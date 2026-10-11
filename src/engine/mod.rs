//! 后台引擎：命令处理（REST）、行情 WebSocket、用户数据流 WebSocket
//!
//! UI 线程通过 `Engine::tx` 发送命令；后台任务把结果写入 `SharedState` 快照并请求重绘。

use eframe::egui;
pub mod state;

use crate::binance::client::{
    build_order_params, BinanceError, BinanceRest, OrderKind, OrderRequest, RestCtx,
};
use crate::binance::model::*;
use crate::binance::symbols::{parse_exchange_info, quantize_price, quantize_qty, RuleBook};
use crate::binance::{endpoints, Interval, Market, Side};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use state::{ConnState, SharedState, ToastKind};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone)]
pub struct SubSpec {
    pub market: Market,
    pub symbol: String,
    pub interval: Interval,
}

#[derive(Debug, Clone)]
pub enum Command {
    /// 配置/凭据变更：校时 + 规则 + 账户全量刷新
    ReloadAll,
    SyncTime,
    Subscribe {
        market: Market,
        symbol: String,
        interval: Interval,
    },
    RefreshAccount,
    RefreshOpenOrders {
        market: Market,
    },
    RefreshHistory {
        market: Market,
        symbol: String,
    },
    PlaceOrder {
        market: Market,
        req: OrderRequest,
    },
    CancelOrder {
        market: Market,
        symbol: String,
        order_id: i64,
    },
    CancelAll {
        market: Market,
        symbol: String,
    },
    SetLeverage {
        symbol: String,
        leverage: u32,
    },
    SetDualSide {
        dual: bool,
    },
}

#[derive(Clone)]
pub struct Engine {
    pub tx: mpsc::UnboundedSender<Command>,
}

impl Engine {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

/// 启动后台运行时与全部任务
pub fn spawn(ctx: egui::Context, state: Arc<SharedState>) -> Engine {
    let (tx, rx) = mpsc::unbounded_channel::<Command>();
    *state.cmd_tx.write() = Some(tx.clone());

    // 初始订阅与全量刷新
    let init_cfg = state.cfg_snapshot();
    let init_sym = init_cfg.default_symbol.clone();
    let init_iv = Interval::from_str(&init_cfg.default_interval);
    let _ = tx.send(Command::Subscribe {
        market: Market::Spot,
        symbol: init_sym,
        interval: init_iv,
    });
    let _ = tx.send(Command::ReloadAll);

    std::thread::Builder::new()
        .name("engine-rt".into())
        .spawn({
            let tx = tx.clone();
            move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("构建 tokio 运行时失败");
                rt.block_on(async move {
                    // 周期校时（每 30 分钟）
                    {
                        let sync_tx = tx.clone();
                        tokio::spawn(async move {
                            loop {
                                tokio::time::sleep(Duration::from_secs(1800)).await;
                                let _ = sync_tx.send(Command::SyncTime);
                            }
                        });
                    }
                    let (sub_tx, sub_rx) = watch::channel::<Option<SubSpec>>(None);
                    tokio::spawn(cmd_loop(
                        ctx.clone(),
                        state.clone(),
                        rx,
                        sub_tx.clone(),
                        tx.clone(),
                    ));
                    tokio::spawn(market_ws_loop(ctx.clone(), state.clone(), sub_rx));
                    tokio::spawn(user_ws_loop(
                        ctx.clone(),
                        state.clone(),
                        Market::Spot,
                        tx.clone(),
                    ));
                    tokio::spawn(user_ws_loop(
                        ctx.clone(),
                        state.clone(),
                        Market::UmFutures,
                        tx,
                    ));
                    // 常驻
                    loop {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                });
            }
        })
        .expect("启动引擎线程失败");

    Engine { tx }
}

fn repaint(ctx: &egui::Context) {
    ctx.request_repaint();
}

fn toast_err(state: &SharedState, e: &BinanceError) {
    tracing::warn!("REST 错误: {e}");
    state.push_toast(ToastKind::Err, e.friendly());
}

// ---------------------------------------------------------------- 命令循环

struct CmdCtx {
    state: Arc<SharedState>,
    client: Option<BinanceRest>,
    built_for: (u64, u64),
    current_sub: Option<SubSpec>,
}

async fn cmd_loop(
    ctx: egui::Context,
    state: Arc<SharedState>,
    mut rx: mpsc::UnboundedReceiver<Command>,
    sub_tx: watch::Sender<Option<SubSpec>>,
    engine_tx: mpsc::UnboundedSender<Command>,
) {
    let mut c = CmdCtx {
        state: state.clone(),
        client: None,
        built_for: (0, 0),
        current_sub: None,
    };
    while let Some(cmd) = rx.recv().await {
        ensure_client(&mut c).await;
        handle_cmd(&ctx, &mut c, cmd, &sub_tx, &engine_tx).await;
    }
}

async fn ensure_client(c: &mut CmdCtx) {
    let cfgv = c.state.cfg_version.load(Ordering::Relaxed);
    let credsv = c.state.creds_version.load(Ordering::Relaxed);
    if c.client.is_some() && c.built_for == (cfgv, credsv) {
        return;
    }
    let cfg = c.state.cfg_snapshot();
    match BinanceRest::new(
        Duration::from_secs(cfg.http_timeout_secs.max(3)),
        &cfg.proxy,
    ) {
        Ok(mut client) => {
            if let Some(creds) = c.state.creds.read().clone() {
                client.set_creds(creds.api_key, creds.api_secret);
            }
            c.client = Some(client);
            c.built_for = (cfgv, credsv);
        }
        Err(e) => {
            toast_err(&c.state, &e);
        }
    }
}

async fn handle_cmd(
    ctx: &egui::Context,
    c: &mut CmdCtx,
    cmd: Command,
    sub_tx: &watch::Sender<Option<SubSpec>>,
    engine_tx: &mpsc::UnboundedSender<Command>,
) {
    let state = c.state.clone();
    let Some(client) = c.client.as_ref() else {
        state.push_toast(ToastKind::Err, "REST 客户端初始化失败，请检查网络/代理设置");
        return;
    };
    match cmd {
        Command::SyncTime | Command::ReloadAll => {
            let cfg = state.cfg_snapshot();
            // 两个市场各校时一次（忽略失败）
            for m in Market::all() {
                let rc = RestCtx {
                    market: m,
                    testnet: cfg.testnet,
                    recv_window_ms: cfg.recv_window_ms,
                };
                if let Err(e) = client.sync_time(rc, &cfg).await {
                    tracing::warn!("校时失败({m}): {e}");
                    state.rest_ok.store(false, Ordering::Relaxed);
                    if let Command::ReloadAll = cmd {
                        state.push_toast(
                            ToastKind::Err,
                            format!("连接 Binance 失败: {}", e.friendly()),
                        );
                    }
                } else {
                    state.rest_ok.store(true, Ordering::Relaxed);
                }
            }
            // 交易规则
            for m in Market::all() {
                let rc = RestCtx {
                    market: m,
                    testnet: cfg.testnet,
                    recv_window_ms: cfg.recv_window_ms,
                };
                let path = format!("{}/exchangeInfo", m.api_prefix());
                match client.get_public(rc, &cfg, &path, &[]).await {
                    Ok(v) => {
                        let book = RuleBook::from_rules(parse_exchange_info(&v));
                        tracing::info!("{}: {} 个交易对", m.label(), book.symbols.len());
                        state.rules.write()[m as usize] = book;
                    }
                    Err(e) => {
                        tracing::warn!("exchangeInfo 失败({m}): {e}");
                        if let Command::ReloadAll = cmd {
                            state.push_toast(
                                ToastKind::Err,
                                format!("获取{}交易规则失败: {}", m.label(), e.friendly()),
                            );
                        }
                    }
                }
            }
            if state.has_creds() {
                engine_tx.send(Command::RefreshAccount).ok();
                for m in Market::all() {
                    engine_tx
                        .send(Command::RefreshOpenOrders { market: m })
                        .ok();
                }
            }
            // 重发当前订阅，确保行情流配置一致
            if let Some(sub) = c.current_sub.clone() {
                let _ = sub_tx.send(Some(sub));
            }
            repaint(ctx);
        }
        Command::Subscribe {
            market,
            symbol,
            interval,
        } => {
            let spec = SubSpec {
                market,
                symbol: symbol.clone(),
                interval,
            };
            c.current_sub = Some(spec.clone());
            let _ = sub_tx.send(Some(spec));
            // 初始快照（公共接口，无需密钥）
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            state.with_symbol_md(market, &symbol, |md| {
                if md.interval != interval {
                    md.klines.clear();
                    md.interval = interval;
                }
                md.ws = ConnState::Connecting;
            });
            let kl = format!("{}/klines", market.api_prefix());
            match client
                .get_public(
                    rc,
                    &cfg,
                    &kl,
                    &[
                        ("symbol", &symbol),
                        ("interval", interval.as_str()),
                        ("limit", "500"),
                    ],
                )
                .await
            {
                Ok(v) => {
                    let rows = v.as_array().cloned().unwrap_or_default();
                    let candles: Vec<Candle> = rows
                        .iter()
                        .filter_map(|r| r.as_array().and_then(|a| parse_kline_row(a)))
                        .collect();
                    state.with_symbol_md(market, &symbol, |md| md.klines = candles);
                    repaint(ctx);
                }
                Err(e) => {
                    state.push_toast(ToastKind::Err, format!("加载K线失败: {}", e.friendly()));
                }
            }
            let tk = format!("{}/ticker/24hr", market.api_prefix());
            if let Ok(v) = client
                .get_public(rc, &cfg, &tk, &[("symbol", &symbol)])
                .await
            {
                if let Ok(t) = serde_json::from_value::<Ticker24>(v) {
                    state.with_symbol_md(market, &symbol, |md| md.ticker = Some(t));
                    repaint(ctx);
                }
            }
            let dp = format!("{}/depth", market.api_prefix());
            if let Ok(v) = client
                .get_public(rc, &cfg, &dp, &[("symbol", &symbol), ("limit", "20")])
                .await
            {
                if let Ok(d) = serde_json::from_value::<Depth>(v) {
                    state.with_symbol_md(market, &symbol, |md| md.depth = d);
                    repaint(ctx);
                }
            }
            let at = format!("{}/aggTrades", market.api_prefix());
            if let Ok(v) = client
                .get_public(rc, &cfg, &at, &[("symbol", &symbol), ("limit", "50")])
                .await
            {
                if let Ok(list) = serde_json::from_value::<Vec<TradeTick>>(v) {
                    state.with_symbol_md(market, &symbol, |md| {
                        md.trades = list.into_iter().collect();
                    });
                    repaint(ctx);
                }
            }
            if market == Market::UmFutures {
                let pi = format!("{}/premiumIndex", market.api_prefix());
                if let Ok(v) = client
                    .get_public(rc, &cfg, &pi, &[("symbol", &symbol)])
                    .await
                {
                    let mark = MarkPriceUpdate {
                        mark_price: v
                            .get("markPrice")
                            .and_then(crate::binance::de::value_to_dec)
                            .unwrap_or_default(),
                        funding_rate: v
                            .get("lastFundingRate")
                            .and_then(crate::binance::de::value_to_dec),
                        next_funding_time: v
                            .get("nextFundingTime")
                            .and_then(|x| x.as_i64())
                            .unwrap_or(0),
                    };
                    state.with_symbol_md(market, &symbol, |md| md.mark = Some(mark));
                    repaint(ctx);
                }
            }
        }
        Command::RefreshAccount => {
            if !state.has_creds() {
                return;
            }
            let cfg = state.cfg_snapshot();
            // 现货
            {
                let rc = RestCtx {
                    market: Market::Spot,
                    testnet: cfg.testnet,
                    recv_window_ms: cfg.recv_window_ms,
                };
                match client
                    .signed(reqwest::Method::GET, rc, &cfg, "/api/v3/account", vec![])
                    .await
                {
                    Ok(v) => match serde_json::from_value::<SpotAccount>(v) {
                        Ok(a) => {
                            let mut acct = state.acct.lock();
                            acct.spot = Some(a);
                            acct.spot_loaded = true;
                            repaint(ctx);
                        }
                        Err(e) => tracing::warn!("解析现货账户失败: {e}"),
                    },
                    Err(e) => {
                        state.set_user_ws(Market::Spot, ConnState::Disconnected);
                        if !matches!(e, BinanceError::NoCreds) {
                            state.push_toast(ToastKind::Err, format!("现货账户: {}", e.friendly()));
                        }
                    }
                }
            }
            // 合约
            {
                let rc = RestCtx {
                    market: Market::UmFutures,
                    testnet: cfg.testnet,
                    recv_window_ms: cfg.recv_window_ms,
                };
                let acct_f =
                    client.signed(reqwest::Method::GET, rc, &cfg, "/fapi/v2/account", vec![]);
                let pr_f = client.signed(
                    reqwest::Method::GET,
                    rc,
                    &cfg,
                    "/fapi/v2/positionRisk",
                    vec![],
                );
                let dual_f = client.signed(
                    reqwest::Method::GET,
                    rc,
                    &cfg,
                    "/fapi/v1/positionSide/dual",
                    vec![],
                );
                let (a, p, d) = tokio::join!(acct_f, pr_f, dual_f);
                let mut ok = true;
                match a {
                    Ok(v) => match serde_json::from_value::<FutAccount>(v) {
                        Ok(acc) => {
                            let mut acct = state.acct.lock();
                            acct.fut = Some(acc);
                            acct.fut_loaded = true;
                        }
                        Err(e) => {
                            tracing::warn!("解析合约账户失败: {e}");
                            ok = false;
                        }
                    },
                    Err(e) => {
                        ok = false;
                        if !matches!(e, BinanceError::NoCreds) {
                            state.push_toast(ToastKind::Err, format!("合约账户: {}", e.friendly()));
                        }
                    }
                }
                if let Ok(v) = p {
                    if let Ok(list) = serde_json::from_value::<Vec<PositionRisk>>(v) {
                        let mut acct = state.acct.lock();
                        acct.fut_leverage
                            .extend(list.iter().map(|x| (x.symbol.clone(), x.leverage)));
                        acct.fut_positions = list;
                    }
                }
                if let Ok(v) = d {
                    if let Ok(m) = serde_json::from_value::<DualSideMode>(v) {
                        state.acct.lock().fut_dual_side = m.dual;
                    }
                }
                if ok {
                    repaint(ctx);
                }
            }
        }
        Command::RefreshOpenOrders { market } => {
            if !state.has_creds() {
                return;
            }
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let path = format!("{}/openOrders", market.api_prefix());
            match client
                .signed(reqwest::Method::GET, rc, &cfg, &path, vec![])
                .await
            {
                Ok(v) => match serde_json::from_value::<Vec<OrderInfo>>(v) {
                    Ok(list) => {
                        state.orders.lock().open[market as usize] = list;
                        repaint(ctx);
                    }
                    Err(e) => tracing::warn!("解析挂单失败: {e}"),
                },
                Err(e) => {
                    if !matches!(e, BinanceError::NoCreds) {
                        tracing::warn!("挂单获取失败({market}): {e}");
                    }
                }
            }
        }
        Command::RefreshHistory { market, symbol } => {
            if !state.has_creds() {
                return;
            }
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let orders_path = format!("{}/allOrders", market.api_prefix());
            let trades_path = match market {
                Market::Spot => "/api/v3/myTrades".to_string(),
                Market::UmFutures => "/fapi/v1/userTrades".to_string(),
            };
            let po = client
                .signed(
                    reqwest::Method::GET,
                    rc,
                    &cfg,
                    &orders_path,
                    vec![
                        ("symbol".into(), symbol.clone()),
                        ("limit".into(), "100".into()),
                    ],
                )
                .await;
            let pt = client
                .signed(
                    reqwest::Method::GET,
                    rc,
                    &cfg,
                    &trades_path,
                    vec![
                        ("symbol".into(), symbol.clone()),
                        ("limit".into(), "100".into()),
                    ],
                )
                .await;
            {
                let mut o = state.orders.lock();
                if let Ok(v) = po {
                    if let Ok(mut list) = serde_json::from_value::<Vec<OrderInfo>>(v) {
                        list.sort_by_key(|x| std::cmp::Reverse(x.time));
                        o.history = list;
                    }
                }
                if let Ok(v) = pt {
                    if let Ok(mut list) = serde_json::from_value::<Vec<MyTrade>>(v) {
                        list.sort_by_key(|x| std::cmp::Reverse(x.time));
                        o.trades = list;
                    }
                }
                o.history_symbol = Some((market, symbol));
            }
            repaint(ctx);
        }
        Command::PlaceOrder { market, mut req } => {
            if !state.has_creds() {
                state.push_toast(ToastKind::Err, crate::i18n::tr("未配置 API 密钥"));
                return;
            }
            let cfg = state.cfg_snapshot();
            let rules = state.rules_of(market);
            // 引擎侧兜底量化与校验
            let Some(r) = rules.get(&req.symbol) else {
                state.push_toast(
                    ToastKind::Err,
                    format!("缺少 {} 交易规则（等待行情初始化）", req.symbol),
                );
                return;
            };
            let mut note = String::new();
            // 量化失败（数值溢出）时拒绝下单，避免向交易所发送错误数量
            let quantized: Option<()> = (|| {
                match &mut req.kind {
                    OrderKind::Limit { price, qty } => {
                        *price = quantize_with_note(*price, r.tick, true, &mut note)?;
                        *qty = quantize_with_note(*qty, r.step, false, &mut note)?;
                    }
                    OrderKind::Market { qty } => {
                        *qty = quantize_with_note(*qty, r.step, false, &mut note)?;
                    }
                    OrderKind::StopLimit {
                        trigger,
                        price,
                        qty,
                        ..
                    } => {
                        *trigger = quantize_with_note(*trigger, r.tick, true, &mut note)?;
                        *price = quantize_with_note(*price, r.tick, true, &mut note)?;
                        *qty = quantize_with_note(*qty, r.step, false, &mut note)?;
                    }
                    OrderKind::StopMarket { trigger, qty, .. } => {
                        *trigger = quantize_with_note(*trigger, r.tick, true, &mut note)?;
                        *qty = quantize_with_note(*qty, r.step, false, &mut note)?;
                    }
                }
                Some(())
            })();
            if quantized.is_none() {
                state.push_toast(
                    ToastKind::Err,
                    crate::i18n::tr("价格/数量超出可表示范围，请检查输入"),
                );
                return;
            }
            if req.kind.qty().is_zero() {
                state.push_toast(ToastKind::Warn, format!("数量过小（步进 {}）", r.step));
                return;
            }
            let params = build_order_params(market, &req);
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let path = format!("{}/order", market.api_prefix());
            state.set_busy("place", true);
            let res = client
                .signed(reqwest::Method::POST, rc, &cfg, &path, params)
                .await;
            state.set_busy("place", false);
            match res {
                Ok(v) => {
                    let id = v.get("orderId").and_then(|x| x.as_i64()).unwrap_or(0);
                    tracing::info!("下单成功 {} {:?} id={id}", req.symbol, req.side);
                    if !note.is_empty() {
                        state.push_toast(ToastKind::Warn, note);
                    }
                    state.push_toast(
                        ToastKind::Ok,
                        format!("{} {} 委托已提交 (#{id})", req.symbol, req.side.label()),
                    );
                    engine_tx.send(Command::RefreshOpenOrders { market }).ok();
                    engine_tx.send(Command::RefreshAccount).ok();
                }
                Err(e) => toast_err(&state, &e),
            }
            repaint(ctx);
        }
        Command::CancelOrder {
            market,
            symbol,
            order_id,
        } => {
            if !state.has_creds() {
                return;
            }
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let path = format!("{}/order", market.api_prefix());
            let params = vec![
                ("symbol".into(), symbol.clone()),
                ("orderId".into(), order_id.to_string()),
            ];
            state.set_busy("cancel", true);
            let res = client
                .signed(reqwest::Method::DELETE, rc, &cfg, &path, params)
                .await;
            state.set_busy("cancel", false);
            match res {
                Ok(_) => {
                    state.push_toast(
                        ToastKind::Ok,
                        format!("{}{order_id}", crate::i18n::tr("已撤销委托 #")),
                    );
                    engine_tx.send(Command::RefreshOpenOrders { market }).ok();
                }
                Err(e) => toast_err(&state, &e),
            }
            repaint(ctx);
        }
        Command::CancelAll { market, symbol } => {
            if !state.has_creds() {
                return;
            }
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let (path, params) = match market {
                Market::Spot => (
                    format!("{}/openOrders", market.api_prefix()),
                    vec![("symbol".into(), symbol.clone())],
                ),
                Market::UmFutures => (
                    format!("{}/allOpenOrders", market.api_prefix()),
                    vec![("symbol".into(), symbol.clone())],
                ),
            };
            state.set_busy("cancel", true);
            let res = client
                .signed(reqwest::Method::DELETE, rc, &cfg, &path, params)
                .await;
            state.set_busy("cancel", false);
            match res {
                Ok(_) => {
                    state.push_toast(
                        ToastKind::Ok,
                        format!("{}{symbol}", crate::i18n::tr("已撤销 ")),
                    );
                    engine_tx.send(Command::RefreshOpenOrders { market }).ok();
                }
                Err(e) => toast_err(&state, &e),
            }
            repaint(ctx);
        }
        Command::SetLeverage { symbol, leverage } => {
            if !state.has_creds() {
                return;
            }
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market: Market::UmFutures,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let params = vec![
                ("symbol".into(), symbol.clone()),
                ("leverage".into(), leverage.to_string()),
            ];
            state.set_busy("lev", true);
            let res = client
                .signed(reqwest::Method::POST, rc, &cfg, "/fapi/v1/leverage", params)
                .await;
            state.set_busy("lev", false);
            match res {
                Ok(_) => {
                    state
                        .acct
                        .lock()
                        .fut_leverage
                        .insert(symbol.clone(), Decimal::from(leverage));
                    state.push_toast(ToastKind::Ok, format!("{symbol} 杠杆已设为 {leverage}x"));
                    engine_tx.send(Command::RefreshAccount).ok();
                }
                Err(e) => toast_err(&state, &e),
            }
            repaint(ctx);
        }
        Command::SetDualSide { dual } => {
            if !state.has_creds() {
                return;
            }
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market: Market::UmFutures,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let params = vec![("dualSidePosition".into(), dual.to_string())];
            state.set_busy("lev", true);
            let res = client
                .signed(
                    reqwest::Method::POST,
                    rc,
                    &cfg,
                    "/fapi/v1/positionSide/dual",
                    params,
                )
                .await;
            state.set_busy("lev", false);
            match res {
                Ok(_) => {
                    state.acct.lock().fut_dual_side = dual;
                    state.push_toast(ToastKind::Ok, crate::i18n::tr("持仓模式已切换").to_string());
                    engine_tx.send(Command::RefreshAccount).ok();
                }
                Err(e) => toast_err(&state, &e),
            }
            repaint(ctx);
        }
    }
}

// ---------------------------------------------------------------- 行情 WebSocket

async fn market_ws_loop(
    ctx: egui::Context,
    state: Arc<SharedState>,
    mut rx_sub: watch::Receiver<Option<SubSpec>>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        // 等待首个订阅
        let spec = loop {
            let cur = rx_sub.borrow_and_update().clone();
            match cur {
                Some(s) => break s,
                None => {
                    if rx_sub.changed().await.is_err() {
                        return;
                    }
                }
            }
        };
        let cfg = state.cfg_snapshot();
        let ep = endpoints(spec.market, cfg.testnet, &cfg);
        let lower = spec.symbol.to_lowercase();
        let mut streams = vec![
            format!("{lower}@ticker"),
            format!("{lower}@depth20@100ms"),
            format!("{lower}@aggTrade"),
            format!("{lower}@kline_{}", spec.interval.as_str()),
        ];
        if spec.market == Market::UmFutures {
            streams.push(format!("{lower}@markPrice@1s"));
        }
        let url = format!("{}/stream?streams={}", ep.ws, streams.join("/"));

        state.set_market_ws(spec.market, ConnState::Connecting);
        state.with_symbol_md(spec.market, &spec.symbol, |md| {
            md.ws = ConnState::Connecting
        });
        repaint(&ctx);

        match crate::binance::ws::connect(&url, &cfg.proxy).await {
            Err(e) => {
                tracing::warn!("行情WS连接失败({}): {e}", spec.market);
                state.set_market_ws(spec.market, ConnState::Failed(short_err(&e)));
                state.with_symbol_md(spec.market, &spec.symbol, |md| {
                    md.ws = ConnState::Failed(short_err(&e))
                });
                repaint(&ctx);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            Ok(ws) => {
                tracing::info!("行情WS已连接: {}", spec.market);
                backoff = Duration::from_secs(1);
                state.set_market_ws(spec.market, ConnState::Connected);
                state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = ConnState::Connected);
                repaint(&ctx);

                let (_, mut read) = futures_util::StreamExt::split(ws);
                let version = last_version(&state);
                // 记录读循环退出原因：出错时保留 Failed 状态（不被下方 Disconnected 覆盖）
                let mut read_fail: Option<String> = None;
                loop {
                    // 循环顶部比对配置版本，避免被高频行情消息饿死
                    if last_version(&state) != version {
                        break;
                    }
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                None => break,
                                Some(Err(e)) => {
                                    tracing::warn!("行情WS错误: {e}");
                                    let msg = short_err(&e.to_string());
                                    read_fail = Some(msg.clone());
                                    state.with_symbol_md(spec.market, &spec.symbol, |md| {
                                        md.ws = ConnState::Failed(msg)
                                    });
                                    break;
                                }
                                Some(Ok(m)) => {
                                    if let tokio_tungstenite::tungstenite::Message::Text(txt) = m {
                                        handle_market_msg_text(&state, &spec, &txt);
                                        repaint(&ctx);
                                    }
                                }
                            }
                        }
                        _ = rx_sub.changed() => break,
                    }
                }
                if let Some(msg) = read_fail {
                    state.set_market_ws(spec.market, ConnState::Failed(msg));
                } else {
                    state.with_symbol_md(spec.market, &spec.symbol, |md| {
                        md.ws = ConnState::Disconnected
                    });
                }
            }
        }
        if !matches!(state.md.lock().market_ws(spec.market), ConnState::Failed(_)) {
            state.set_market_ws(spec.market, ConnState::Disconnected);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn last_version(state: &SharedState) -> (u64, u64) {
    (
        state.cfg_version.load(Ordering::Relaxed),
        state.creds_version.load(Ordering::Relaxed),
    )
}

fn short_err(e: &str) -> String {
    let e = e.lines().next().unwrap_or("");
    if e.chars().count() > 80 {
        let s: String = e.chars().take(80).collect();
        format!("{s}…")
    } else {
        e.to_string()
    }
}

/// 按规则量化并记录变化说明；溢出返回 None
fn quantize_with_note(
    v: Decimal,
    unit: Decimal,
    is_price: bool,
    note: &mut String,
) -> Option<Decimal> {
    let q = if is_price {
        quantize_price(v, unit)?
    } else {
        quantize_qty(v, unit)?
    };
    if q != v {
        if !note.is_empty() {
            note.push('；');
        }
        note.push_str(&format!("{v} → {q}"));
    }
    Some(q)
}

fn handle_market_msg_text(state: &SharedState, spec: &SubSpec, text: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let stream = v.get("stream").and_then(|s| s.as_str()).unwrap_or("");
    let data = match v.get("data") {
        Some(d) => d,
        None => &v,
    };
    if stream.ends_with("@ticker") {
        if let Ok(t) = serde_json::from_value::<Ticker24>(data.clone()) {
            state.with_symbol_md(spec.market, &spec.symbol, |md| md.ticker = Some(t));
        }
    } else if stream.contains("@depth20") {
        if let Ok(d) = serde_json::from_value::<Depth>(data.clone()) {
            state.with_symbol_md(spec.market, &spec.symbol, |md| md.depth = d);
        }
    } else if stream.ends_with("@aggTrade") {
        if let Ok(t) = serde_json::from_value::<TradeTick>(data.clone()) {
            state.apply_trades(spec.market, &spec.symbol, t);
        }
    } else if stream.contains("@kline_") {
        if let Ok(k) = serde_json::from_value::<WsKline>(data.clone()) {
            state.apply_kline(spec.market, &spec.symbol, spec.interval, &k.k);
        }
    } else if stream.contains("@markPrice") {
        if let Ok(m) = serde_json::from_value::<MarkPriceUpdate>(data.clone()) {
            state.with_symbol_md(spec.market, &spec.symbol, |md| md.mark = Some(m));
        }
    }
}

// ---------------------------------------------------------------- 用户数据流

async fn user_ws_loop(
    ctx: egui::Context,
    state: Arc<SharedState>,
    market: Market,
    engine_tx: mpsc::UnboundedSender<Command>,
) {
    loop {
        // 无凭据则等待
        if !state.has_creds() {
            tokio::time::sleep(Duration::from_millis(800)).await;
            continue;
        }
        let cfg = state.cfg_snapshot();
        let ep = endpoints(market, cfg.testnet, &cfg);
        let rc = RestCtx {
            market,
            testnet: cfg.testnet,
            recv_window_ms: cfg.recv_window_ms,
        };

        // 构建 listenKey 专用客户端
        let client = match BinanceRest::new(
            Duration::from_secs(cfg.http_timeout_secs.max(3)),
            &cfg.proxy,
        ) {
            Ok(mut c) => {
                if let Some(creds) = state.creds.read().clone() {
                    c.set_creds(creds.api_key, creds.api_secret);
                }
                c
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let lk_path = market.listen_key_path();
        let key = match client
            .key_auth(reqwest::Method::POST, rc, &cfg, lk_path, &[])
            .await
        {
            Ok(v) => v
                .get("listenKey")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            Err(e) => {
                tracing::warn!("listenKey 获取失败({market}): {e}");
                state.set_user_ws(market, ConnState::Failed(e.friendly()));
                repaint(&ctx);
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };
        let Some(key) = key else {
            state.set_user_ws(market, ConnState::Failed("listenKey 响应异常".into()));
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        };
        let url = format!("{}/ws/{}", ep.ws, key);
        // 建连前采样版本号：循环顶部比对，避免被高频事件饿死
        let last_seen = last_version(&state);
        state.set_user_ws(market, ConnState::Connecting);
        repaint(&ctx);

        match crate::binance::ws::connect(&url, &cfg.proxy).await {
            Err(e) => {
                tracing::warn!("用户数据流连接失败({market}): {e}");
                state.set_user_ws(market, ConnState::Failed(short_err(&e)));
                repaint(&ctx);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Ok(ws) => {
                tracing::info!("用户数据流已连接({market})");
                state.set_user_ws(market, ConnState::Connected);
                repaint(&ctx);
                // 重连后立即对账
                engine_tx.send(Command::RefreshAccount).ok();
                engine_tx.send(Command::RefreshOpenOrders { market }).ok();

                let (_, mut read) = futures_util::StreamExt::split(ws);
                let mut keepalive = tokio::time::interval(Duration::from_secs(25 * 60));
                keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // interval 首个 tick 立即完成，先消费掉，避免建连即触发保活
                keepalive.tick().await;
                // 现货 PUT/DELETE 需携带 listenKey 参数，合约端点无参
                let keepalive_params: Vec<(&str, &str)> = match market {
                    Market::Spot => vec![("listenKey", key.as_str())],
                    Market::UmFutures => vec![],
                };
                let mut alive = true;
                while alive {
                    if last_version(&state) != last_seen {
                        break;
                    }
                    tokio::select! {
                        msg = read.next() => match msg {
                            None => alive = false,
                            Some(Err(e)) => {
                                tracing::warn!("用户数据流错误({market}): {e}");
                                alive = false;
                            }
                            Some(Ok(m)) => {
                                if let tokio_tungstenite::tungstenite::Message::Text(txt) = m {
                                    handle_user_event(&state, market, &txt, &engine_tx);
                                    repaint(&ctx);
                                }
                            }
                        },
                        _ = keepalive.tick() => {
                            if let Err(e) = client.key_auth(reqwest::Method::PUT, rc, &cfg, lk_path, &keepalive_params).await {
                                tracing::warn!("listenKey 保活失败({market}): {e}，将重建");
                                alive = false;
                            }
                        }
                    }
                }
                // 尽力关闭 listenKey
                let close_params: Vec<(&str, &str)> = match market {
                    Market::Spot => vec![("listenKey", key.as_str())],
                    Market::UmFutures => vec![],
                };
                let _ = client
                    .key_auth(reqwest::Method::DELETE, rc, &cfg, lk_path, &close_params)
                    .await;
            }
        }
        state.set_user_ws(market, ConnState::Disconnected);
        repaint(&ctx);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// 处理用户数据流事件（现货 executionReport/outboundAccountPosition；合约 ORDER_TRADE_UPDATE/ACCOUNT_UPDATE）
fn handle_user_event(
    state: &SharedState,
    market: Market,
    text: &str,
    engine_tx: &mpsc::UnboundedSender<Command>,
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let et = v.get("e").and_then(|x| x.as_str()).unwrap_or("");
    match (market, et) {
        (Market::Spot, "executionReport") => {
            let Ok(r) = serde_json::from_value::<ExecReport>(v) else {
                return;
            };
            let side = order_side_of(&r.side);
            let o = OrderInfo {
                symbol: r.symbol.clone(),
                order_id: r.order_id,
                client_order_id: r.client_order_id.clone(),
                price: Some(r.price),
                orig_qty: r.orig_qty,
                executed_qty: r.cum_qty,
                status: r.status.clone(),
                side: r.side.clone(),
                order_type: r.order_type.clone(),
                stop_price: None,
                time: r.trade_time,
                update_time: r.event_time,
                position_side: crate::binance::FutPosSide::Both.as_str().to_string(),
                reduce_only: false,
            };
            apply_order_update(state, market, o);
            if r.exec_type == "TRADE" && r.last_qty > Decimal::ZERO {
                fill_toast(
                    state,
                    &r.symbol,
                    side,
                    &r.last_qty,
                    r.last_price.as_ref(),
                    &r.status,
                );
                engine_tx.send(Command::RefreshAccount).ok();
                engine_tx.send(Command::RefreshOpenOrders { market }).ok();
            }
        }
        (Market::Spot, "outboundAccountPosition") => {
            let Ok(p) = serde_json::from_value::<OutboundAccountPosition>(v) else {
                return;
            };
            let mut acct = state.acct.lock();
            if let Some(spot) = acct.spot.as_mut() {
                spot.balances = p.balances;
            }
        }
        (Market::UmFutures, "ORDER_TRADE_UPDATE") => {
            let Ok(f) = serde_json::from_value::<FutExecInner>(v) else {
                return;
            };
            let side = order_side_of(&f.o.side);
            let o = OrderInfo {
                symbol: f.o.symbol.clone(),
                order_id: f.o.order_id,
                client_order_id: f.o.client_order_id.clone(),
                price: f.o.price,
                orig_qty: f.o.orig_qty,
                executed_qty: f.o.cum_qty,
                status: f.o.status.clone(),
                side: f.o.side.clone(),
                order_type: f.o.order_type.clone(),
                stop_price: f.o.stop_price,
                time: f.o.trade_time,
                update_time: f.o.trade_time,
                position_side: f.o.position_side.clone(),
                reduce_only: f.o.reduce_only,
            };
            apply_order_update(state, market, o);
            let filled_now = f.o.last_qty.map(|q| q > Decimal::ZERO).unwrap_or(false);
            if filled_now {
                fill_toast(
                    state,
                    &f.o.symbol,
                    side,
                    f.o.last_qty.as_ref().unwrap(),
                    f.o.last_price.as_ref(),
                    &f.o.status,
                );
                engine_tx.send(Command::RefreshAccount).ok();
                engine_tx.send(Command::RefreshOpenOrders { market }).ok();
            }
            if let Some(rp) = f.o.realized_profit {
                if rp != Decimal::ZERO
                    && matches!(f.o.status.as_str(), "FILLED" | "PARTIALLY_FILLED")
                {
                    state.push_toast(
                        if rp > Decimal::ZERO {
                            ToastKind::Ok
                        } else {
                            ToastKind::Warn
                        },
                        format!("已实现盈亏: {rp}"),
                    );
                }
            }
        }
        (Market::UmFutures, "ACCOUNT_UPDATE") => {
            // 账户/持仓变更 → 触发轻量对账（REST 拉全量，保证标记价等字段准确）
            engine_tx.send(Command::RefreshAccount).ok();
        }
        _ => {}
    }
}

#[derive(serde::Deserialize)]
struct FutExecInner {
    #[serde(rename = "o")]
    o: FutExec,
}

fn order_side_of(s: &str) -> Side {
    if s == "SELL" {
        Side::Sell
    } else {
        Side::Buy
    }
}

/// 统一更新挂单快照
fn apply_order_update(state: &SharedState, market: Market, o: OrderInfo) {
    let done = matches!(
        o.status.as_str(),
        "FILLED" | "CANCELED" | "EXPIRED" | "REJECTED" | "EXPIRED_IN_MATCH"
    );
    let mut snap = state.orders.lock();
    let list = &mut snap.open[market as usize];
    list.retain(|x| !(x.order_id == o.order_id && x.symbol == o.symbol));
    if !done {
        list.insert(0, o.clone());
    }
}

fn fill_toast(
    state: &SharedState,
    symbol: &str,
    side: Side,
    qty: &Decimal,
    price: Option<&Decimal>,
    status: &str,
) {
    let px = price
        .map(|p| p.to_string())
        .unwrap_or_else(|| "市价".into());
    let word = if status == "FILLED" {
        "已成交"
    } else {
        "部分成交"
    };
    state.push_toast(
        if status == "FILLED" {
            ToastKind::Ok
        } else {
            ToastKind::Info
        },
        format!("{word}: {symbol} {} {qty} @ {px}", side.label()),
    );
}
