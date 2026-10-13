//! Binance REST 客户端：请求构建、HMAC-SHA256 签名、时间同步、限速与错误映射

use crate::binance::{endpoints, Market, Side};
use crate::config::ProxyConfig;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::Sha256;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

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
                    Some(s) => format!("{base}（{s}s 后可重试）"),
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
                _ => format!("Binance 错误[{code}]: {msg}"),
            },
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
}

impl BinanceRest {
    pub fn new(timeout: Duration, proxy: &ProxyConfig) -> Result<Self, BinanceError> {
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("BnTradingPro/", env!("CARGO_PKG_VERSION")));
        if proxy.enabled && !proxy.url.trim().is_empty() {
            let p = reqwest::Proxy::all(proxy.url.trim())
                .map_err(|e| BinanceError::Network(format!("{}{e}", crate::i18n::tr("代理配置无效: "))))?;
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
        self.resync_time(&ep, ctx.market).await
    }

    /// 按既有端点校时（-1021 自动重试路径复用）
    async fn resync_time(
        &self,
        ep: &crate::binance::Endpoints,
        market: Market,
    ) -> Result<(), BinanceError> {
        // 路径必须带市场前缀：现货 /api/v3/time、合约 /fapi/v1/time
        let path = format!("{}/time", market.api_prefix());
        let v = self.get_public_raw(ep, market, &path, &[]).await?;
        let server = v
            .get("serverTime")
            .and_then(|x| x.as_i64())
            .ok_or_else(|| BinanceError::Decode("serverTime 缺失".into()))?;
        let offset = server - Self::now_ms();
        self.time_offset_ms.store(offset, Ordering::Relaxed);
        tracing::info!("时间同步完成，偏移 {offset}ms");
        Ok(())
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
        self.get_public_raw(&ep, ctx.market, path, params).await
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
        self.signed_raw(method, &ep, ctx, path, params).await
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
        if !self.has_creds() {
            return Err(BinanceError::NoCreds);
        }
        self.wait_for_weight(ctx.market).await?;
        let ep = endpoints(ctx.market, ctx.testnet, overrides);
        let url = format!("{}{}", ep.rest, path);
        let key = self.api_key.as_ref().unwrap();
        let mut req = self.http.request(method, &url);
        if !params.is_empty() {
            req = req.query(params);
        }
        let resp = req
            .header("X-MBX-APIKEY", key.as_str())
            .send()
            .await
            .map_err(map_req_err)?;
        self.track_weight(ctx.market, &resp);
        parse_response(resp).await
    }

    async fn wait_for_weight(&self, market: Market) -> Result<(), BinanceError> {
        let i = market as usize;
        let wait = {
            let mut window = self.weight_window[i].lock().unwrap();
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
            let mut window = self.weight_window[i].lock().unwrap();
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
            let mut window = self.weight_window[i].lock().unwrap();
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
        parse_response(resp).await
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
                other => return other,
            }
        }
        unreachable!("循环恰好两次且第二次必经 other 分支返回")
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
                "HTTP {status}（响应体为空，请检查 REST 端点/代理配置是否正确）"
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
        return Err(BinanceError::Decode(format!("HTTP {status}: {text}")));
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
    /// 合约双向持仓模式下的持仓方向
    pub position_side: crate::binance::FutPosSide,
    pub reduce_only: bool,
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
}

impl OrderKind {
    pub fn label(&self) -> &'static str {
        match self {
            OrderKind::Limit { .. } => "限价",
            OrderKind::Market { .. } => "市价",
            OrderKind::StopLimit {
                take_profit: false, ..
            } => "止损限价",
            OrderKind::StopLimit {
                take_profit: true, ..
            } => "止盈限价",
            OrderKind::StopMarket {
                take_profit: false, ..
            } => "止损市价",
            OrderKind::StopMarket {
                take_profit: true, ..
            } => "止盈市价",
        }
    }

    pub fn qty(&self) -> Decimal {
        match self {
            OrderKind::Limit { qty, .. } | OrderKind::Market { qty } => *qty,
            OrderKind::StopLimit { qty, .. } | OrderKind::StopMarket { qty, .. } => *qty,
        }
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
            p.push(("timeInForce".into(), "GTC".into()));
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
            p.push(("timeInForce".into(), "GTC".into()));
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
            p.push(("quantity".into(), qty.normalize().to_string()));
            p.push(("stopPrice".into(), trigger.normalize().to_string()));
        }
    }
    if market == Market::UmFutures {
        match req.position_side {
            crate::binance::FutPosSide::Both => {
                if req.reduce_only {
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
            position_side: FutPosSide::Both,
            reduce_only: false,
            client_id: Some("c-1".into()),
        };
        let p = build_order_params(Market::Spot, &req);
        assert!(p.contains(&("type".into(), "LIMIT".into())));
        assert!(p.contains(&("price".into(), "30000.12".into())));
        assert!(p.contains(&("quantity".into(), "0.0001".into())));
        assert!(p.contains(&("newClientOrderId".into(), "c-1".into())));
        assert!(!p.iter().any(|(k, _)| k == "reduceOnly"));
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
            position_side: FutPosSide::Long,
            reduce_only: false,
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
            position_side: FutPosSide::Both,
            reduce_only: true,
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
}
