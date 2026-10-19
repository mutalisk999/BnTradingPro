//! Binance REST 客户端：请求构建、HMAC-SHA256 签名、时间同步、限速与错误映射

use crate::binance::{endpoints, Endpoints, Market, Side, SPOT_REST_DEFAULT};
use crate::config::ProxyConfig;
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::Sha256;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

/// 现货主网降级镜像（默认域名被运营商/代理误转返回 410 空响应时按序探测）。
/// 均为官方等价镜像，共享同一 API 与限频
const SPOT_REST_MIRRORS: [&str; 5] = [
    "https://api-gcp.binance.com",
    "https://api1.binance.com",
    "https://api2.binance.com",
    "https://api3.binance.com",
    "https://api4.binance.com",
];

#[derive(Debug, Error)]
pub enum BinanceError {
    #[error("未配置 API 密钥")]
    NoCreds,
    #[error("请求超时")]
    Timeout,
    #[error("网络错误: {0}")]
    Network(String),
    #[error("Binance 错误 [{code}]: {msg}")]
    Api { code: i64, msg: String },
    #[error("响应解析失败: {0}")]
    Decode(String),
    #[error("请求过于频繁")]
    RateLimit {
        banned: bool,
        retry_after_secs: Option<u64>,
    },
}

impl BinanceError {
    /// UI 展示用友好信息
    pub fn friendly(&self) -> String {
        let tr = crate::i18n::tr;
        match self {
            BinanceError::NoCreds => tr("未配置 API 密钥，请在设置中配置").into(),
            BinanceError::Timeout => tr("请求超时，请检查网络/代理").into(),
            BinanceError::Network(e) => format!("{}{e}", tr("网络错误: ")),
            BinanceError::Decode(e) => format!("{}{e}", tr("响应解析失败: ")),
            BinanceError::RateLimit {
                banned,
                retry_after_secs,
            } => {
                let base = if *banned {
                    tr("IP 已被限频封禁（418），请降低请求频率后重试")
                } else {
                    tr("请求频率超限（429），请稍后重试")
                };
                match retry_after_secs {
                    // ZH 保留全角括号观感；EN 走半角 + 自然语序
                    Some(s) => {
                        if crate::i18n::lang() == crate::i18n::EN {
                            format!("{base} (retry in {s}s)")
                        } else {
                            format!("{base}（{s}s 后可重试）")
                        }
                    }
                    None => base.to_string(),
                }
            }
            BinanceError::Api { code, msg } => match code {
                -1021 => tr("本地时间与服务器偏差过大，已自动校时并重试，请重试").into(),
                -2015 => {
                    tr("API Key 无效或权限不足（检查 Key、IP 白名单、是否用对测试网/实盘）").into()
                }
                -2014 => tr("API Key 格式错误").into(),
                -1022 => tr("签名错误（Secret 不匹配）").into(),
                -1013 => tr("下单参数不满足过滤器（价格/数量/最小名义金额）").into(),
                -2010 => format!("{}{msg}", tr("下单被拒绝: ")),
                -2019 => tr("保证金不足").into(),
                -2021 => tr("订单会立即触发（止盈止损价过于贴近市价）").into(),
                -2022 => tr("订单方向与持仓方向冲突（ReduceOnly）").into(),
                -4061 => tr("持仓方向与订单冲突（检查双向/单向持仓模式）").into(),
                _ => format!("{} [{code}]: {msg}", tr("Binance 错误")),
            },
        }
    }
}

/// 限价类订单有效期；GTX = 只做 maker（Post-Only，仅合约）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tif {
    #[default]
    Gtc,
    Ioc,
    Fok,
    Gtx,
}

impl Tif {
    pub fn as_str(self) -> &'static str {
        match self {
            Tif::Gtc => "GTC",
            Tif::Ioc => "IOC",
            Tif::Fok => "FOK",
            Tif::Gtx => "GTX",
        }
    }

    /// 各市场可用的 TIF 集合（现货无 GTX）
    pub fn available(market: Market) -> &'static [Tif] {
        match market {
            Market::Spot => &[Tif::Gtc, Tif::Ioc, Tif::Fok],
            Market::UmFutures => &[Tif::Gtc, Tif::Ioc, Tif::Fok, Tif::Gtx],
        }
    }
}

/// 单个请求的上下文（市场 + 环境）
#[derive(Debug, Clone, Copy)]
pub struct RestCtx {
    pub market: Market,
    pub testnet: bool,
    pub recv_window_ms: u64,
}

pub struct BinanceRest {
    http: reqwest::Client,
    api_key: Option<Zeroizing<String>>,
    api_secret: Option<Zeroizing<String>>,
    time_offset_ms: AtomicI64,
    /// 每市场独立的已用权重计数：现货与合约的限额（6000/min vs 2400/min）
    /// 与响应头互不相关，混用一份计数会互相污染导致误限速/漏限速
    weight_used: [AtomicU64; 2],
    weight_window: [Mutex<Option<std::time::Instant>>; 2],
    weight_limit: [u64; 2],
    /// 现货主网镜像降级的粘性游标：上次成功的镜像下标。
    /// 默认域名经代理返回空响应时按 `SPOT_REST_MIRRORS` 轮换重试，
    /// 成功后粘住该镜像，后续请求的镜像探测从它开始
    spot_mirror: Mutex<Option<usize>>,
}

impl BinanceRest {
    pub fn new(timeout: Duration, proxy: &ProxyConfig) -> Result<Self, BinanceError> {
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("BnTradingPro/", env!("CARGO_PKG_VERSION")));
        if proxy.enabled && !proxy.url.trim().is_empty() {
            let p = reqwest::Proxy::all(proxy.url.trim()).map_err(|e| {
                BinanceError::Network(format!("{}{e}", crate::i18n::tr("代理配置无效: ")))
            })?;
            builder = builder.proxy(p);
        }
        let http = builder
            .build()
            .map_err(|e| BinanceError::Network(e.to_string()))?;
        Ok(Self {
            http,
            api_key: None,
            api_secret: None,
            time_offset_ms: AtomicI64::new(0),
            weight_used: [AtomicU64::new(0), AtomicU64::new(0)],
            weight_window: [Mutex::new(None), Mutex::new(None)],
            // 各自取限额的 2/3 留余量：现货 6000/min、合约 2400/min
            weight_limit: [4000, 1600],
            spot_mirror: Mutex::new(None),
        })
    }

    pub fn set_creds(&mut self, key: String, secret: String) {
        self.api_key = Some(Zeroizing::new(key));
        self.api_secret = Some(Zeroizing::new(secret));
    }

    pub fn has_creds(&self) -> bool {
        self.api_key.is_some() && self.api_secret.is_some()
    }

    pub fn time_offset_ms(&self) -> i64 {
        self.time_offset_ms.load(Ordering::Relaxed)
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn signed_now_ms(&self) -> i64 {
        Self::now_ms() + self.time_offset_ms()
    }

    /// 与服务器校时（签名请求的 timestamp 需要该偏移）
    pub async fn sync_time(
        &self,
        ctx: RestCtx,
        overrides: &crate::config::AppConfig,
    ) -> Result<(), BinanceError> {
        let ep = endpoints(ctx.market, ctx.testnet, overrides);
        self.with_spot_fallback(ep, ctx, |ep| async move {
            self.resync_time(&ep, ctx.market).await
        })
        .await
    }

    /// 按既有端点校时（-1021 自动重试路径复用）
    async fn resync_time(&self, ep: &Endpoints, market: Market) -> Result<(), BinanceError> {
        // 路径必须带市场前缀：现货 /api/v3/time、合约 /fapi/v1/time
        let path = format!("{}/time", market.api_prefix());
        let v = self.get_public_raw(ep, market, &path, &[]).await?;
        let server = v
            .get("serverTime")
            .and_then(|x| x.as_i64())
            .ok_or_else(|| {
                BinanceError::Decode(crate::i18n::tr("serverTime 缺失").to_string())
            })?;
        let offset = server - Self::now_ms();
        self.time_offset_ms.store(offset, Ordering::Relaxed);
        tracing::info!("时间同步完成，偏移 {offset}ms");
        Ok(())
    }

    /// 现货主网降级重试包装：请求先打当前端点；仅当错误是 Decode（响应体
    /// 非 JSON——代理误转 410 空响应的典型形态）且端点就是内置默认域名
    /// （用户显式覆盖端点时尊重其配置不介入）时，逐个尝试官方镜像。
    /// 真实币安错误（Api/RateLimit）、超时与网络错误一律不降级——能拿到
    /// 结构化错误说明请求已到达币安，跨镜像重试只会浪费限频权重
    async fn with_spot_fallback<T, F, Fut>(
        &self,
        ep: Endpoints,
        ctx: RestCtx,
        run: F,
    ) -> Result<T, BinanceError>
    where
        F: Fn(Endpoints) -> Fut,
        Fut: std::future::Future<Output = Result<T, BinanceError>>,
    {
        match run(ep.clone()).await {
            Ok(v) => Ok(v),
            Err(e) if spot_mirror_eligible(&ep, ctx, &e) => {
                let order = mirror_order(*self.spot_mirror.lock());
                let mut last = e;
                for i in order {
                    let mut m = ep.clone();
                    m.rest = SPOT_REST_MIRRORS[i].into();
                    tracing::warn!("现货 REST 降级重试 {} -> {}", ep.rest, m.rest);
                    match run(m).await {
                        Ok(v) => {
                            tracing::info!(
                                "现货 REST 镜像 {} 可用，后续请求从它开始探测",
                                SPOT_REST_MIRRORS[i]
                            );
                            *self.spot_mirror.lock() = Some(i);
                            return Ok(v);
                        }
                        // 镜像也返回空响应：继续试下一个
                        Err(e2) if is_mirror_trigger(&e2) => last = e2,
                        // 非空响应错误（到达币安/超时/断网）：镜像重试无意义，直接返回
                        Err(e2) => return Err(e2),
                    }
                }
                // 全部镜像都空响应：清掉粘性，下次请求重新从默认域名试起
                *self.spot_mirror.lock() = None;
                Err(last)
            }
            Err(e) => Err(e),
        }
    }

    /// 带端点覆盖的请求包装（由 engine 传入当前配置）
    pub async fn get_public(
        &self,
        ctx: RestCtx,
        overrides: &crate::config::AppConfig,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<Value, BinanceError> {
        let ep = endpoints(ctx.market, ctx.testnet, overrides);
        self.with_spot_fallback(ep, ctx, |ep| async move {
            self.get_public_raw(&ep, ctx.market, path, params).await
        })
        .await
    }

    pub async fn signed(
        &self,
        method: reqwest::Method,
        ctx: RestCtx,
        overrides: &crate::config::AppConfig,
        path: &str,
        params: Vec<(String, String)>,
    ) -> Result<Value, BinanceError> {
        let ep = endpoints(ctx.market, ctx.testnet, overrides);
        // 闭包会被调用多次（默认域名 + 各镜像），每次先克隆再移交 async 块
        self.with_spot_fallback(ep, ctx, move |ep| {
            let method = method.clone();
            let params = params.clone();
            async move { self.signed_raw(method, &ep, ctx, path, params).await }
        })
        .await
    }

    /// 仅需 API Key 头的端点（listenKey 管理），不签名。
    /// 现货 PUT/DELETE /api/v3/userDataStream 必须携带 listenKey 参数。
    pub async fn key_auth(
        &self,
        method: reqwest::Method,
        ctx: RestCtx,
        overrides: &crate::config::AppConfig,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<Value, BinanceError> {
        let ep = endpoints(ctx.market, ctx.testnet, overrides);
        self.with_spot_fallback(ep, ctx, move |ep| {
            let method = method.clone();
            async move {
                if !self.has_creds() {
                    return Err(BinanceError::NoCreds);
                }
                self.wait_for_weight(ctx.market).await?;
                let url = format!("{}{}", ep.rest, path);
                let key = self.api_key.as_ref().unwrap();
                let mut req = self.http.request(method.clone(), &url);
                if !params.is_empty() {
                    req = req.query(params);
                }
                let resp = req
                    .header("X-MBX-APIKEY", key.as_str())
                    .send()
                    .await
                    .map_err(map_req_err)?;
                self.track_weight(ctx.market, &resp);
                parse_response(resp)
                    .await
                    .map_err(|e| with_endpoint_ctx(e, ctx.market, &method, path))
            }
        })
        .await
    }

    async fn wait_for_weight(&self, market: Market) -> Result<(), BinanceError> {
        let i = market as usize;
        let wait = {
            let mut window = self.weight_window[i].lock();
            match *window {
                None => None,
                Some(start) => {
                    if start.elapsed() > Duration::from_secs(60) {
                        *window = None;
                        self.weight_used[i].store(0, Ordering::Relaxed);
                        None
                    } else if self.weight_used[i].load(Ordering::Relaxed) >= self.weight_limit[i] {
                        Some(Duration::from_secs(60) - start.elapsed())
                    } else {
                        None
                    }
                }
            }
        };
        // 在锁外等待，避免 MutexGuard 跨 await 导致 future 非 Send
        if let Some(wait) = wait {
            tracing::warn!(
                "已用权重({market}) {} 过高，等待 {:?}",
                self.weight_used[i].load(Ordering::Relaxed),
                wait
            );
            tokio::time::sleep(wait).await;
            let mut window = self.weight_window[i].lock();
            *window = None;
            self.weight_used[i].store(0, Ordering::Relaxed);
        }
        Ok(())
    }

    fn track_weight(&self, market: Market, resp: &reqwest::Response) {
        let h = resp
            .headers()
            .get("x-mbx-used-weight-1m")
            .or_else(|| resp.headers().get("x-mbx-used-weight"));
        if let Some(w) = h
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
        {
            let i = market as usize;
            self.weight_used[i].store(w, Ordering::Relaxed);
            let mut window = self.weight_window[i].lock();
            if window.is_none() {
                *window = Some(std::time::Instant::now());
            }
        }
    }

    async fn get_public_raw(
        &self,
        ep: &crate::binance::Endpoints,
        market: Market,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<Value, BinanceError> {
        self.wait_for_weight(market).await?;
        let full = format!("{}{}", ep.rest, path);
        let mut req = self.http.get(&full);
        if !params.is_empty() {
            req = req.query(params);
        }
        let resp = req.send().await.map_err(map_req_err)?;
        self.track_weight(market, &resp);
        parse_response(resp)
            .await
            .map_err(|e| with_endpoint_ctx(e, market, &reqwest::Method::GET, path))
    }

    async fn signed_raw(
        &self,
        method: reqwest::Method,
        ep: &crate::binance::Endpoints,
        ctx: RestCtx,
        path: &str,
        params: Vec<(String, String)>,
    ) -> Result<Value, BinanceError> {
        if !self.has_creds() {
            return Err(BinanceError::NoCreds);
        }
        self.wait_for_weight(ctx.market).await?;

        // -1021（本地时间与服务器偏差）时自动校时并用新 timestamp 重签重试一次
        for attempt in 0..2u8 {
            let qs = build_signed_query(
                &params,
                self.signed_now_ms(),
                ctx.recv_window_ms,
                self.api_secret.as_ref().unwrap().as_str(),
            );
            let url = format!("{}{}?{}", ep.rest, path, qs);
            let key = self.api_key.as_ref().unwrap();
            let resp = self
                .http
                .request(method.clone(), &url)
                .header("X-MBX-APIKEY", key.as_str())
                .send()
                .await
                .map_err(map_req_err)?;
            self.track_weight(ctx.market, &resp);
            match parse_response(resp).await {
                Err(BinanceError::Api { code: -1021, .. }) if attempt == 0 => {
                    self.resync_time(ep, ctx.market).await?;
                }
                other => return other.map_err(|e| with_endpoint_ctx(e, ctx.market, &method, path)),
            }
        }
        unreachable!("循环恰好两次且第二次必经 other 分支返回")
    }

    /// 合约 WS-API 下单：连接 ws-fapi 端点，发送逐请求签名的 order.place，
    /// 阻塞等待同 id 响应（整体 8s 超时）。任何失败由调用方回退 REST
    pub async fn place_order_ws(
        &self,
        url: &str,
        proxy: &crate::config::ProxyConfig,
        market: Market,
        req: &OrderRequest,
        recv_window_ms: u64,
    ) -> Result<Value, BinanceError> {
        use futures_util::{SinkExt as _, StreamExt};
        let Some(key) = self.api_key.as_ref() else {
            return Err(BinanceError::NoCreds);
        };
        let Some(secret) = self.api_secret.as_ref() else {
            return Err(BinanceError::NoCreds);
        };
        let ts = self.signed_now_ms();
        let mut params = build_order_params(market, req);
        params.push(("apiKey".into(), key.to_string()));
        let signed = build_signed_query(&params, ts, recv_window_ms, secret.as_str());
        let (_, sig) = signed
            .split_once("&signature=")
            .ok_or_else(|| {
                BinanceError::Decode(crate::i18n::tr("签名构建失败").to_string())
            })?;
        // WS-API 的 params 是对象：键值取原始（未 URL 编码）字符串
        let mut obj = serde_json::Map::new();
        for (k, v) in &params {
            obj.insert(k.clone(), Value::String(v.clone()));
        }
        obj.insert("timestamp".into(), Value::from(ts));
        if recv_window_ms > 0 {
            obj.insert("recvWindow".into(), Value::from(recv_window_ms));
        }
        obj.insert("signature".into(), Value::String(sig.to_string()));
        let id = format!("btp-{ts}");
        let msg = serde_json::json!({ "id": id, "method": "order.place", "params": obj });

        let ws = crate::binance::ws::connect(url, proxy)
            .await
            .map_err(BinanceError::Network)?;
        let (mut write, mut read) = futures_util::StreamExt::split(ws);
        use tokio_tungstenite::tungstenite::Message;
        write
            .send(Message::text(msg.to_string()))
            .await
            .map_err(|e| BinanceError::Network(e.to_string()))?;
        let fut = async {
            loop {
                match read.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                        let d: Value = match serde_json::from_str(&t) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        if d.get("id").and_then(|x| x.as_str()) != Some(id.as_str()) {
                            continue;
                        }
                        let status = d.get("status").and_then(|x| x.as_i64()).unwrap_or(0);
                        if status == 200 {
                            return Ok(d.get("result").cloned().unwrap_or(Value::Null));
                        }
                        let code = d.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
                        let m = d
                            .get("msg")
                            .and_then(|x| x.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        return Err(BinanceError::Api { code, msg: m });
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(BinanceError::Network(e.to_string())),
                    None => {
                        return Err(BinanceError::Network(
                            crate::i18n::tr("WS-API 连接中断").to_string(),
                        ))
                    }
                }
            }
        };
        let r = tokio::time::timeout(Duration::from_secs(8), fut)
            .await
            .map_err(|_| BinanceError::Timeout)?;
        r
    }

    /// 现货 WS API 用户数据流订阅消息（userDataStream.subscribe.signature）。
    /// listenKey REST 端点已下线（2026-02-20 起 410 Gone），现货私有事件改为
    /// 连 WS API 后用此方法订阅——HMAC/RSA/Ed25519 key 均可，无需 Ed25519
    /// session.logon。返回可直接发送的 JSON 消息。
    /// params 是对象：serde_json Map 序列化后按键字典序排列，故签名串同样
    /// 按字典序构建（apiKey < recvWindow < timestamp），服务端无论按收到
    /// 键序还是排序重放，签名都一致
    pub fn spot_user_sub_message(&self, recv_window_ms: u64) -> Result<Value, BinanceError> {
        let key = self.api_key.as_ref().ok_or(BinanceError::NoCreds)?;
        let secret = self.api_secret.as_ref().ok_or(BinanceError::NoCreds)?;
        let ts = self.signed_now_ms();
        let key = key.as_str();
        let mut payload = format!("apiKey={key}");
        if recv_window_ms > 0 {
            payload.push_str(&format!("&recvWindow={recv_window_ms}"));
        }
        payload.push_str(&format!("&timestamp={ts}"));
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC 密钥无长度限制");
        mac.update(payload.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        let mut obj = serde_json::Map::new();
        obj.insert("apiKey".into(), Value::String(key.to_string()));
        if recv_window_ms > 0 {
            obj.insert("recvWindow".into(), Value::from(recv_window_ms));
        }
        obj.insert("signature".into(), Value::String(sig));
        obj.insert("timestamp".into(), Value::from(ts));
        Ok(serde_json::json!({
            "id": format!("btp-ussub-{ts}"),
            "method": "userDataStream.subscribe.signature",
            "params": obj,
        }))
    }
}

/// 组装待签查询串并附 HMAC-SHA256 签名（纯函数，便于测试）。
/// 返回 `query&signature=...`；`recv_window_ms == 0` 时省略 recvWindow 参数。
pub(crate) fn build_signed_query(
    params: &[(String, String)],
    timestamp_ms: i64,
    recv_window_ms: u64,
    secret: &str,
) -> String {
    let mut all: Vec<(String, String)> = params.to_vec();
    all.push(("timestamp".into(), timestamp_ms.to_string()));
    if recv_window_ms > 0 {
        all.push(("recvWindow".into(), recv_window_ms.to_string()));
    }
    let query: String = all
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(query.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());
    format!("{query}&signature={sig}")
}

fn map_req_err(e: reqwest::Error) -> BinanceError {
    // reqwest 的 Display 会附带完整 URL（含签名、listenKey 等敏感查询参数），
    // 错误信息会进入日志文件，落盘前必须剥离
    let e = e.without_url();
    if e.is_timeout() {
        BinanceError::Timeout
    } else {
        BinanceError::Network(e.to_string())
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// 错误是否是「镜像降级触发形」：响应体非 JSON（空响应、代理错误页）。
/// 真实币安错误都是结构化 JSON（Api/RateLimit），超时与网络错误另计——
/// 它们说明要么没到币安但可能只是暂时，要么已到达币安，镜像重试无意义
fn is_mirror_trigger(e: &BinanceError) -> bool {
    matches!(e, BinanceError::Decode(_))
}

/// 镜像轮换顺序：`None`（尚未知任何可用镜像）时按默认顺序探测；
/// `Some(i)` 粘性游标从上次成功的镜像开始。游标越界（配置修改过
/// 镜像表等）自动回到 0
fn mirror_order(start: Option<usize>) -> Vec<usize> {
    let n = SPOT_REST_MIRRORS.len();
    let s = start.map(|i| i % n).unwrap_or(0);
    (0..n).map(|k| (s + k) % n).collect()
}

/// 降级是否适用：仅现货主网、端点为内置默认域名（用户显式覆盖端点时
/// 尊重其配置，不擅自换域名）且错误是代理误转形的 Decode
fn spot_mirror_eligible(ep: &Endpoints, ctx: RestCtx, e: &BinanceError) -> bool {
    ctx.market == Market::Spot
        && !ctx.testnet
        && ep.rest == SPOT_REST_DEFAULT
        && is_mirror_trigger(e)
}

/// 为 HTTP 级失败附加端点上下文（方法 + 路径 + 市场）。
/// 一个光秃秃的「HTTP 410 Gone」无法定位是哪个调用挂了——现货/合约
/// 那么多端点，带上路径用户（和日志）才能一眼看出该查哪个
fn with_endpoint_ctx(
    e: BinanceError,
    market: Market,
    method: &reqwest::Method,
    path: &str,
) -> BinanceError {
    match e {
        BinanceError::Decode(m) => {
            BinanceError::Decode(format!("{method} {path}（{}）: {m}", market.label()))
        }
        BinanceError::Api { code, msg } => BinanceError::Api {
            code,
            msg: format!("{method} {path}（{}）: {msg}", market.label()),
        },
        other => other,
    }
}

async fn parse_response(resp: reqwest::Response) -> Result<Value, BinanceError> {
    let status = resp.status();
    // 429 限频 / 418 IP 封禁：读取 Retry-After 头，返回专用错误（含可重试时间）
    if matches!(status.as_u16(), 429 | 418) {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        return Err(BinanceError::RateLimit {
            banned: status.as_u16() == 418,
            retry_after_secs: retry_after,
        });
    }
    let text = resp
        .text()
        .await
        .map_err(|e| BinanceError::Decode(e.to_string()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| {
        if text.trim().is_empty() {
            BinanceError::Decode(format!(
                "{}{status}{}",
                crate::i18n::tr("HTTP "),
                crate::i18n::tr("（响应体为空，请检查 REST 端点/代理配置是否正确）")
            ))
        } else {
            BinanceError::Decode(format!("HTTP {status}: {e}"))
        }
    })?;
    if !status.is_success() {
        if let (Some(code), Some(msg)) = (
            v.get("code").and_then(|c| c.as_i64()),
            v.get("msg").and_then(|m| m.as_str()),
        ) {
            return Err(BinanceError::Api {
                code,
                msg: msg.to_string(),
            });
        }
        // 代理/拦截层返回的 HTML 错误页可能很长，截断避免撑爆 toast 与日志
        let excerpt: String = text.chars().take(120).collect();
        let suffix = if text.chars().count() > 120 {
            "…"
        } else {
            ""
        };
        return Err(BinanceError::Decode(format!(
            "HTTP {status}: {excerpt}{suffix}"
        )));
    }
    // 部分合约端点（如 positionSide/dual）成功时返回 {"code":200,"msg":"success"}；
    // 错误码为负数，其余正数 code 视为异常
    if let Some(code_v) = v.get("code") {
        let code = code_v
            .as_i64()
            .or_else(|| code_v.as_str().and_then(|s| s.parse().ok()));
        if let Some(code) = code {
            if code == 200 {
                return Ok(v);
            }
            let msg = v
                .get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            return Err(BinanceError::Api { code, msg });
        }
    }
    Ok(v)
}

// ---------- 下单参数构建（纯函数，便于测试） ----------

/// UI 层的订单请求描述
#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub kind: OrderKind,
    /// 限价类订单有效期（市价单忽略）
    pub tif: Tif,
    /// 合约双向持仓模式下的持仓方向
    pub position_side: crate::binance::FutPosSide,
    pub reduce_only: bool,
    /// 合约 STOP_MARKET 触发即全平该持仓方向（与 quantity/reduceOnly 互斥）
    pub close_position: bool,
    pub client_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum OrderKind {
    Limit {
        price: Decimal,
        qty: Decimal,
    },
    Market {
        qty: Decimal,
    },
    /// 现货：止损限价 / 止盈限价（trigger 触发后以 price 挂限价单）
    StopLimit {
        trigger: Decimal,
        price: Decimal,
        qty: Decimal,
        take_profit: bool,
    },
    /// 合约：止损市价 / 止盈市价
    StopMarket {
        trigger: Decimal,
        qty: Decimal,
        take_profit: bool,
    },
    /// 合约：跟踪委托（TRAILING_STOP_MARKET，回调幅度为百分数 0.1-10）
    TrailingStop {
        callback_rate: Decimal,
        qty: Decimal,
    },
}

impl OrderKind {
    pub fn label(&self) -> &'static str {
        // 与 Market/Side/FutPosSide 的 label 同构：内部即走 tr()，
        // 调用方直接拼接展示（外层再包 tr 会因缺键回退成英文）
        match self {
            OrderKind::Limit { .. } => crate::i18n::tr("限价"),
            OrderKind::Market { .. } => crate::i18n::tr("市价"),
            OrderKind::StopLimit {
                take_profit: false, ..
            } => crate::i18n::tr("止损限价"),
            OrderKind::StopLimit {
                take_profit: true, ..
            } => crate::i18n::tr("止盈限价"),
            OrderKind::StopMarket {
                take_profit: false, ..
            } => crate::i18n::tr("止损市价"),
            OrderKind::StopMarket {
                take_profit: true, ..
            } => crate::i18n::tr("止盈市价"),
            OrderKind::TrailingStop { .. } => crate::i18n::tr("跟踪委托"),
        }
    }

    pub fn qty(&self) -> Decimal {
        match self {
            OrderKind::Limit { qty, .. } | OrderKind::Market { qty } => *qty,
            OrderKind::StopLimit { qty, .. } | OrderKind::StopMarket { qty, .. } => *qty,
            OrderKind::TrailingStop { qty, .. } => *qty,
        }
    }

    /// 合约条件单类型（已迁移至 Algo Order 体系，/fapi/v1/order 报 -4120）
    pub fn is_conditional(&self) -> bool {
        matches!(
            self,
            OrderKind::StopLimit { .. }
                | OrderKind::StopMarket { .. }
                | OrderKind::TrailingStop { .. }
        )
    }
}

/// 构建下单请求参数（不含签名与公共字段）
pub fn build_order_params(market: Market, req: &OrderRequest) -> Vec<(String, String)> {
    let mut p: Vec<(String, String)> = vec![
        ("symbol".into(), req.symbol.clone()),
        ("side".into(), req.side.as_str().into()),
    ];
    match &req.kind {
        OrderKind::Limit { price, qty } => {
            p.push(("type".into(), "LIMIT".into()));
            p.push(("timeInForce".into(), req.tif.as_str().into()));
            p.push(("quantity".into(), qty.normalize().to_string()));
            p.push(("price".into(), price.normalize().to_string()));
        }
        OrderKind::Market { qty } => {
            p.push(("type".into(), "MARKET".into()));
            p.push(("quantity".into(), qty.normalize().to_string()));
        }
        OrderKind::StopLimit {
            trigger,
            price,
            qty,
            take_profit,
        } => {
            p.push((
                "type".into(),
                if *take_profit {
                    "TAKE_PROFIT_LIMIT".into()
                } else {
                    "STOP_LOSS_LIMIT".into()
                },
            ));
            p.push(("timeInForce".into(), req.tif.as_str().into()));
            p.push(("quantity".into(), qty.normalize().to_string()));
            p.push(("price".into(), price.normalize().to_string()));
            p.push(("stopPrice".into(), trigger.normalize().to_string()));
        }
        OrderKind::StopMarket {
            trigger,
            qty,
            take_profit,
        } => {
            p.push((
                "type".into(),
                if *take_profit {
                    "TAKE_PROFIT_MARKET".into()
                } else {
                    "STOP_MARKET".into()
                },
            ));
            if req.close_position {
                // closePosition 语义：触发即全平该持仓方向，与 quantity 互斥
                p.push(("closePosition".into(), "true".into()));
            } else {
                p.push(("quantity".into(), qty.normalize().to_string()));
            }
            p.push(("stopPrice".into(), trigger.normalize().to_string()));
        }
        OrderKind::TrailingStop { callback_rate, qty } => {
            p.push(("type".into(), "TRAILING_STOP_MARKET".into()));
            p.push(("quantity".into(), qty.normalize().to_string()));
            p.push(("callbackRate".into(), callback_rate.normalize().to_string()));
        }
    }
    if market == Market::UmFutures {
        match req.position_side {
            crate::binance::FutPosSide::Both => {
                // closePosition 本身即减仓语义，不能再叠加 reduceOnly
                if req.reduce_only && !req.close_position {
                    p.push(("reduceOnly".into(), "true".into()));
                }
            }
            s => {
                p.push(("positionSide".into(), s.as_str().into()));
            }
        }
    }
    if let Some(cid) = &req.client_id {
        p.push(("newClientOrderId".into(), cid.clone()));
    }
    p
}

/// 构建合约条件单（Algo Order）参数（POST /fapi/v1/algoOrder，不含签名
/// 与公共字段）。条件单已从 /fapi/v1/order 迁出（报 -4120），与普通单
/// 的参数名差异：须带 algoType=CONDITIONAL、stopPrice→triggerPrice、
/// newClientOrderId→clientAlgoId；限价版条件单 type 为 STOP/TAKE_PROFIT
pub fn build_algo_order_params(req: &OrderRequest) -> Vec<(String, String)> {
    let mut p: Vec<(String, String)> = vec![
        ("algoType".into(), "CONDITIONAL".into()),
        ("symbol".into(), req.symbol.clone()),
        ("side".into(), req.side.as_str().into()),
    ];
    match &req.kind {
        OrderKind::StopLimit {
            trigger,
            price,
            qty,
            take_profit,
        } => {
            p.push((
                "type".into(),
                if *take_profit {
                    "TAKE_PROFIT".into()
                } else {
                    "STOP".into()
                },
            ));
            p.push(("timeInForce".into(), req.tif.as_str().into()));
            p.push(("quantity".into(), qty.normalize().to_string()));
            p.push(("price".into(), price.normalize().to_string()));
            p.push(("triggerPrice".into(), trigger.normalize().to_string()));
        }
        OrderKind::StopMarket {
            trigger,
            qty,
            take_profit,
        } => {
            p.push((
                "type".into(),
                if *take_profit {
                    "TAKE_PROFIT_MARKET".into()
                } else {
                    "STOP_MARKET".into()
                },
            ));
            if req.close_position {
                // closePosition 语义：触发即全平该持仓方向，与 quantity 互斥
                p.push(("closePosition".into(), "true".into()));
            } else {
                p.push(("quantity".into(), qty.normalize().to_string()));
            }
            p.push(("triggerPrice".into(), trigger.normalize().to_string()));
        }
        OrderKind::TrailingStop { callback_rate, qty } => {
            p.push(("type".into(), "TRAILING_STOP_MARKET".into()));
            p.push(("quantity".into(), qty.normalize().to_string()));
            p.push(("callbackRate".into(), callback_rate.normalize().to_string()));
        }
        _ => unreachable!("仅条件单类型进入 algoOrder 参数构建"),
    }
    // 双向持仓必须带 positionSide；单向模式下 closePosition 本身即减仓
    // 语义，不能再叠加 reduceOnly
    match req.position_side {
        crate::binance::FutPosSide::Both => {
            if req.reduce_only && !req.close_position {
                p.push(("reduceOnly".into(), "true".into()));
            }
        }
        s => {
            p.push(("positionSide".into(), s.as_str().into()));
        }
    }
    if let Some(cid) = &req.client_id {
        p.push(("clientAlgoId".into(), cid.clone()));
    }
    p
}

/// 现货 OCO（one-cancels-other）：限价腿 + 止损限价腿，一腿成交另一腿自动撤销
#[derive(Debug, Clone)]
pub struct OcoRequest {
    pub symbol: String,
    pub side: Side,
    pub qty: Decimal,
    /// 止盈腿限价
    pub price: Decimal,
    /// 止损腿触发价
    pub stop_price: Decimal,
    /// 止损腿限价（触发后挂出的限价）
    pub stop_limit_price: Decimal,
    pub client_id: Option<String>,
}

/// 构建 OCO 下单参数（POST /api/v3/order/oco，不含签名与公共字段）
pub fn build_oco_params(req: &OcoRequest) -> Vec<(String, String)> {
    let mut p: Vec<(String, String)> = vec![
        ("symbol".into(), req.symbol.clone()),
        ("side".into(), req.side.as_str().into()),
        ("quantity".into(), req.qty.normalize().to_string()),
        ("price".into(), req.price.normalize().to_string()),
        ("stopPrice".into(), req.stop_price.normalize().to_string()),
        (
            "stopLimitPrice".into(),
            req.stop_limit_price.normalize().to_string(),
        ),
        ("stopLimitTimeInForce".into(), "GTC".into()),
    ];
    if let Some(cid) = &req.client_id {
        p.push(("listClientOrderId".into(), cid.clone()));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binance::FutPosSide;

    fn hmac_sha256(key: &str, msg: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
        mac.update(msg.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn hmac_known_vector() {
        // RFC 4231 test case 2
        assert_eq!(
            hmac_sha256("Jefe", "what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn build_limit_order_spot() {
        let req = OrderRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Buy,
            kind: OrderKind::Limit {
                price: Decimal::new(3000012, 2), // 30000.12
                qty: Decimal::new(1, 4),         // 0.0001
            },
            tif: Tif::Gtc,
            position_side: FutPosSide::Both,
            reduce_only: false,
            close_position: false,
            client_id: Some("c-1".into()),
        };
        let p = build_order_params(Market::Spot, &req);
        assert!(p.contains(&("type".into(), "LIMIT".into())));
        assert!(p.contains(&("timeInForce".into(), "GTC".into())));
        assert!(p.contains(&("price".into(), "30000.12".into())));
        assert!(p.contains(&("quantity".into(), "0.0001".into())));
        assert!(p.contains(&("newClientOrderId".into(), "c-1".into())));
        assert!(!p.iter().any(|(k, _)| k == "reduceOnly"));
    }

    #[test]
    fn build_limit_order_uses_request_tif() {
        let req = OrderRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Buy,
            kind: OrderKind::Limit {
                price: Decimal::new(3000012, 2),
                qty: Decimal::new(1, 4),
            },
            tif: Tif::Ioc,
            position_side: FutPosSide::Both,
            reduce_only: false,
            close_position: false,
            client_id: None,
        };
        let p = build_order_params(Market::Spot, &req);
        assert!(p.contains(&("timeInForce".into(), "IOC".into())));
    }

    #[test]
    fn build_stop_market_close_position_omits_quantity() {
        let req = OrderRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Sell,
            kind: OrderKind::StopMarket {
                trigger: Decimal::new(900000, 1),
                qty: Decimal::ONE, // closePosition 下省略
                take_profit: false,
            },
            tif: Tif::Gtc,
            position_side: FutPosSide::Both,
            reduce_only: false,
            close_position: true,
            client_id: None,
        };
        let p = build_order_params(Market::UmFutures, &req);
        assert!(p.contains(&("closePosition".into(), "true".into())));
        assert!(!p.iter().any(|(k, _)| k == "quantity"));
        // closePosition 本身即减仓语义，不能再叠加 reduceOnly
        assert!(!p.iter().any(|(k, _)| k == "reduceOnly"));
    }

    #[test]
    fn build_algo_order_params_conditional_names() {
        // 整仓止盈：条件单迁至 Algo 体系后参数名差异——algoType=CONDITIONAL、
        // triggerPrice（非 stopPrice）、clientAlgoId（非 newClientOrderId）
        let req = OrderRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Sell,
            kind: OrderKind::StopMarket {
                trigger: Decimal::new(872000, 1),
                qty: Decimal::ONE,
                take_profit: true,
            },
            tif: Tif::Gtc,
            position_side: FutPosSide::Both,
            reduce_only: false,
            close_position: true,
            client_id: Some("btp-tpsl-1".into()),
        };
        let p = build_algo_order_params(&req);
        assert!(p.contains(&("algoType".into(), "CONDITIONAL".into())));
        assert!(p.contains(&("type".into(), "TAKE_PROFIT_MARKET".into())));
        assert!(p.contains(&("triggerPrice".into(), "87200".into())));
        assert!(p.contains(&("closePosition".into(), "true".into())));
        assert!(p.contains(&("clientAlgoId".into(), "btp-tpsl-1".into())));
        assert!(!p.iter().any(|(k, _)| k == "stopPrice"));
        assert!(!p.iter().any(|(k, _)| k == "newClientOrderId"));
        assert!(!p.iter().any(|(k, _)| k == "reduceOnly"));

        // 限价止损：algo 体系 type 为 STOP（非 STOP_LOSS_LIMIT）
        let req = OrderRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Sell,
            kind: OrderKind::StopLimit {
                trigger: Decimal::new(850000, 1),
                price: Decimal::new(849000, 1),
                qty: Decimal::new(1, 2),
                take_profit: false,
            },
            tif: Tif::Gtc,
            position_side: FutPosSide::Long,
            reduce_only: false,
            close_position: false,
            client_id: None,
        };
        let p = build_algo_order_params(&req);
        assert!(p.contains(&("type".into(), "STOP".into())));
        assert!(p.contains(&("positionSide".into(), "LONG".into())));
        assert!(p.contains(&("quantity".into(), "0.01".into())));
    }

    #[test]
    fn build_trailing_stop_params() {
        let req = OrderRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Sell,
            kind: OrderKind::TrailingStop {
                callback_rate: Decimal::new(10, 1), // 1.0
                qty: Decimal::new(1, 3),
            },
            tif: Tif::Gtc,
            position_side: FutPosSide::Both,
            reduce_only: false,
            close_position: false,
            client_id: None,
        };
        let p = build_order_params(Market::UmFutures, &req);
        assert!(p.contains(&("type".into(), "TRAILING_STOP_MARKET".into())));
        assert!(p.contains(&("callbackRate".into(), "1".into())));
    }

    #[test]
    fn build_oco_params_fields() {
        let req = crate::binance::client::OcoRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Sell,
            qty: Decimal::new(1, 3),
            price: Decimal::new(3100000, 2),
            stop_price: Decimal::new(2900000, 2),
            stop_limit_price: Decimal::new(2899000, 2),
            client_id: Some("l-1".into()),
        };
        let p = build_oco_params(&req);
        assert!(p.contains(&("stopLimitTimeInForce".into(), "GTC".into())));
        assert!(p.contains(&("price".into(), "31000".into())));
        assert!(p.contains(&("stopPrice".into(), "29000".into())));
        assert!(p.contains(&("stopLimitPrice".into(), "28990".into())));
        assert!(p.contains(&("listClientOrderId".into(), "l-1".into())));
    }

    #[test]
    fn build_stop_market_futures_hedge() {
        let req = OrderRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Sell,
            kind: OrderKind::StopMarket {
                trigger: Decimal::new(900000, 1),
                qty: Decimal::new(2, 3),
                take_profit: false,
            },
            tif: Tif::Gtc,
            position_side: FutPosSide::Long,
            reduce_only: false,
            close_position: false,
            client_id: None,
        };
        let p = build_order_params(Market::UmFutures, &req);
        assert!(p.contains(&("type".into(), "STOP_MARKET".into())));
        assert!(p.contains(&("positionSide".into(), "LONG".into())));
        assert!(p.contains(&("stopPrice".into(), "90000".into())));
    }

    #[test]
    fn build_market_futures_one_way_reduce_only() {
        let req = OrderRequest {
            symbol: "BTCUSDT".into(),
            side: Side::Sell,
            kind: OrderKind::Market {
                qty: Decimal::new(1, 3),
            },
            tif: Tif::Gtc,
            position_side: FutPosSide::Both,
            reduce_only: true,
            close_position: false,
            client_id: None,
        };
        let p = build_order_params(Market::UmFutures, &req);
        assert!(p.contains(&("reduceOnly".into(), "true".into())));
        assert!(!p.iter().any(|(k, _)| k == "positionSide"));
    }

    #[test]
    fn signed_query_omits_recv_window_when_zero() {
        let qs = build_signed_query(&[("symbol".into(), "BTCUSDT".into())], 1, 0, "secret");
        assert_eq!(
            qs,
            format!(
                "symbol=BTCUSDT&timestamp=1&signature={}",
                hmac_sha256("secret", "symbol=BTCUSDT&timestamp=1")
            )
        );
    }

    #[test]
    fn signed_query_includes_recv_window() {
        let qs = build_signed_query(&[], 42, 5000, "k");
        assert!(qs.contains("timestamp=42"));
        assert!(qs.contains("recvWindow=5000"));
        let query_part = qs.split("&signature=").next().unwrap();
        let sig = qs.split("&signature=").nth(1).unwrap();
        assert_eq!(*sig, hmac_sha256("k", query_part));
    }

    #[tokio::test]
    async fn parse_response_maps_binance_error() {
        let resp = reqwest::Response::from(
            http::Response::builder()
                .status(400)
                .body(r#"{"code":-2010,"msg":"order would immediately match"}"#)
                .unwrap(),
        );
        match parse_response(resp).await {
            Err(BinanceError::Api { code: -2010, msg }) => {
                assert_eq!(msg, "order would immediately match")
            }
            other => panic!("期望 Api(-2010)，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn parse_response_code_200_success_is_ok() {
        // positionSide/dual 等端点成功时返回 {"code":200,"msg":"success"}
        let resp = reqwest::Response::from(
            http::Response::builder()
                .status(200)
                .body(r#"{"code":200,"msg":"success"}"#)
                .unwrap(),
        );
        assert!(parse_response(resp).await.is_ok());
    }

    #[tokio::test]
    async fn parse_response_positive_code_is_error() {
        let resp = reqwest::Response::from(
            http::Response::builder()
                .status(200)
                .body(r#"{"code":1,"msg":"weird"}"#)
                .unwrap(),
        );
        match parse_response(resp).await {
            Err(BinanceError::Api { code: 1, .. }) => {}
            other => panic!("期望 Api(1)，得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn parse_response_empty_body_is_decode_error() {
        // 空响应体常见于端点覆盖填错（打到了非 Binance 服务）
        let resp = reqwest::Response::from(http::Response::builder().status(200).body("").unwrap());
        match parse_response(resp).await {
            Err(e) => assert!(e.to_string().contains("HTTP 200")),
            Ok(v) => panic!("期望 Decode 错误，得到 {v}"),
        }
    }

    #[test]
    fn signed_query_urlencodes_values() {
        let qs = build_signed_query(&[("k".into(), "a b&c=d".into())], 1, 0, "secret");
        assert!(qs.starts_with("k=a+b%26c%3Dd&timestamp=1&signature="));
    }

    #[test]
    fn mirror_order_rotation() {
        // 无粘性：从镜像表头（api-gcp）开始
        let order = mirror_order(None);
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
        // 粘性 i：从 i 开始轮换，回到表头
        let order = mirror_order(Some(3));
        assert_eq!(order, vec![3, 4, 0, 1, 2]);
        // 游标越界按取模回到有效范围（99 % 5 = 4）
        assert_eq!(mirror_order(Some(99)), vec![4, 0, 1, 2, 3]);
    }

    #[test]
    fn spot_mirror_eligibility() {
        let spot_ep = Endpoints {
            rest: SPOT_REST_DEFAULT.into(),
            ws: "wss://stream.binance.com:9443".into(),
        };
        let decode = BinanceError::Decode("HTTP 410（响应体为空）".into());
        let spot_mainnet = RestCtx {
            market: Market::Spot,
            testnet: false,
            recv_window_ms: 0,
        };
        // 现货主网 + 默认端点 + Decode → 可降级
        assert!(spot_mirror_eligible(&spot_ep, spot_mainnet, &decode));
        // 用户显式覆盖了端点 → 尊重配置，不降级
        let overridden = Endpoints {
            rest: "https://my-relay.example".into(),
            ws: spot_ep.ws.clone(),
        };
        assert!(!spot_mirror_eligible(&overridden, spot_mainnet, &decode));
        // 测试网不降级（镜像不服务测试网）
        let testnet = RestCtx {
            testnet: true,
            ..spot_mainnet
        };
        assert!(!spot_mirror_eligible(&spot_ep, testnet, &decode));
        // 合约不降级（镜像仅覆盖现货 api 域名）
        let fut = RestCtx {
            market: Market::UmFutures,
            ..spot_mainnet
        };
        assert!(!spot_mirror_eligible(&spot_ep, fut, &decode));
        // 结构化错误（到达币安后的真实错误）不降级
        let api = BinanceError::Api {
            code: -1021,
            msg: "timestamp".into(),
        };
        assert!(!spot_mirror_eligible(&spot_ep, spot_mainnet, &api));
        assert!(!spot_mirror_eligible(
            &spot_ep,
            spot_mainnet,
            &BinanceError::Timeout
        ));
        // 到达镜像后的非空响应错误同样不应继续轮换
        assert!(!is_mirror_trigger(&api));
    }
}
