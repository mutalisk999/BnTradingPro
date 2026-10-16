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
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use state::{ConnState, SharedState, ToastKind};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
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
    /// 撤销挂单：symbol=None 撤销该市场全部挂单
    CancelAll {
        market: Market,
        symbol: Option<String>,
    },
    /// 现货 OCO 下单（限价腿 + 止损限价腿，一腿成交另一腿自动撤销）
    PlaceOco {
        req: crate::binance::client::OcoRequest,
    },
    SetLeverage {
        symbol: String,
        leverage: u32,
    },
    /// 切换合约保证金模式（全仓/逐仓）
    SetMarginType {
        symbol: String,
        isolated: bool,
    },
    SetDualSide {
        dual: bool,
    },
    /// 把当前内存配置落盘（UI 线程不做同步磁盘 IO）
    PersistConfig,
    /// 断线重连后补拉 K 线快照（跨 K 线缺口修复）
    RefreshKlines {
        market: Market,
        symbol: String,
        interval: Interval,
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
    let init_sym = init_cfg.default_symbol.trim().to_uppercase();
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
                    let h_cmd = tokio::spawn(cmd_loop(
                        ctx.clone(),
                        state.clone(),
                        rx,
                        sub_tx.clone(),
                        tx.clone(),
                    ));
                    // 期货行情须拆 public/market 两条连接（Binance 2026-03 起
                    // WS 入口分流，legacy 入口只剩 /public 类推送）；现货仅
                    // 槽位 A 承载，槽位 B 空转。pair 合并两槽状态供 UI 展示
                    let ws_pair = Arc::new(parking_lot::Mutex::new(WsPairState::default()));
                    let h_market = tokio::spawn(market_ws_loop(
                        ctx.clone(),
                        state.clone(),
                        sub_rx.clone(),
                        ws_pair.clone(),
                        MarketWsSlot::A,
                    ));
                    let h_market_b = tokio::spawn(market_ws_loop(
                        ctx.clone(),
                        state.clone(),
                        sub_rx.clone(),
                        ws_pair.clone(),
                        MarketWsSlot::B,
                    ));
                    let h_user_spot = tokio::spawn(user_ws_loop(
                        ctx.clone(),
                        state.clone(),
                        Market::Spot,
                        tx.clone(),
                    ));
                    let h_user_fut = tokio::spawn(user_ws_loop(
                        ctx.clone(),
                        state.clone(),
                        Market::UmFutures,
                        tx.clone(),
                    ));

                    // 任务监督：这些循环本应永不返回，一旦退出（多为 panic）此前
                    // 完全无感知——界面看起来正常但命令/行情静默停摆。每个任务
                    // 配独立监督协程（原先串行 await，先阻塞在永不返回的 cmd
                    // 循环上，WS 任务死亡根本轮不到被观察），保证崩溃可见
                    // （日志 + 错误 toast），WS 循环自动重启（最多 3 次）
                    let supervise =
                        |name: &'static str,
                         h: tokio::task::JoinHandle<()>,
                         state: Arc<SharedState>,
                         ctx: egui::Context,
                         tx: mpsc::UnboundedSender<Command>,
                         sub_rx: watch::Receiver<Option<SubSpec>>,
                         pair: Arc<parking_lot::Mutex<WsPairState>>| {
                            tokio::spawn(async move {
                                let result = h.await;
                                tracing::error!("引擎后台任务退出({name}): {result:?}");
                                state.push_toast(
                                    ToastKind::Err,
                                    format!("{}{name}", crate::i18n::tr("后台任务异常退出: ")),
                                );
                                repaint(&ctx);
                                // 命令循环持有命令接收端，无法重建，只能提示重启
                                if name == "cmd" {
                                    state.push_toast(
                                        ToastKind::Err,
                                        crate::i18n::tr("命令处理任务已停止，请重启应用")
                                            .to_string(),
                                    );
                                    return;
                                }
                                let mut restarts = 0;
                                loop {
                                    let started = tokio::time::Instant::now();
                                    let h2 = match name {
                                        "market" => tokio::spawn(market_ws_loop(
                                            ctx.clone(),
                                            state.clone(),
                                            sub_rx.clone(),
                                            pair.clone(),
                                            MarketWsSlot::A,
                                        )),
                                        "market-b" => tokio::spawn(market_ws_loop(
                                            ctx.clone(),
                                            state.clone(),
                                            sub_rx.clone(),
                                            pair.clone(),
                                            MarketWsSlot::B,
                                        )),
                                        "user-spot" => tokio::spawn(user_ws_loop(
                                            ctx.clone(),
                                            state.clone(),
                                            Market::Spot,
                                            tx.clone(),
                                        )),
                                        _ => tokio::spawn(user_ws_loop(
                                            ctx.clone(),
                                            state.clone(),
                                            Market::UmFutures,
                                            tx.clone(),
                                        )),
                                    };
                                    let res = h2.await;
                                    tracing::error!("引擎后台任务重启后再次退出({name}): {res:?}");
                                    // 稳定运行超过 10min 后再退出视为新的独立故障，
                                    // 重置计数，避免累计 3 次后永久放弃重启
                                    if started.elapsed() > Duration::from_secs(600) {
                                        restarts = 0;
                                    }
                                    restarts += 1;
                                    if restarts >= 3 {
                                        state.push_toast(
                                            ToastKind::Err,
                                            format!(
                                                "{}{name} ×3",
                                                crate::i18n::tr("任务反复退出，请重启应用: ")
                                            ),
                                        );
                                        break;
                                    }
                                    tokio::time::sleep(Duration::from_secs(5)).await;
                                }
                            });
                        };
                    supervise(
                        "cmd",
                        h_cmd,
                        state.clone(),
                        ctx.clone(),
                        tx.clone(),
                        sub_rx.clone(),
                        ws_pair.clone(),
                    );
                    supervise(
                        "market",
                        h_market,
                        state.clone(),
                        ctx.clone(),
                        tx.clone(),
                        sub_rx.clone(),
                        ws_pair.clone(),
                    );
                    supervise(
                        "market-b",
                        h_market_b,
                        state.clone(),
                        ctx.clone(),
                        tx.clone(),
                        sub_rx.clone(),
                        ws_pair.clone(),
                    );
                    supervise(
                        "user-spot",
                        h_user_spot,
                        state.clone(),
                        ctx.clone(),
                        tx.clone(),
                        sub_rx.clone(),
                        ws_pair.clone(),
                    );
                    supervise(
                        "user-futures",
                        h_user_fut,
                        state.clone(),
                        ctx.clone(),
                        tx.clone(),
                        sub_rx.clone(),
                        ws_pair.clone(),
                    );
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
    /// Arc 包装：订单类命令可整体 clone 进独立任务并发执行，不被刷新命令阻塞
    client: Option<Arc<BinanceRest>>,
    built_for: (u64, u64),
    current_sub: Option<SubSpec>,
    /// 上次账户刷新时间（RefreshAccount 节流用）
    last_acct_refresh: Option<Instant>,
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
        last_acct_refresh: None,
    };
    while let Some(cmd) = rx.recv().await {
        ensure_client(&mut c).await;
        // 合并积压命令：成交风暴期间每个 WS 事件都会入队账户/挂单刷新，
        // 逐条执行会刷爆请求权重（下单/撤单共用同一限速闸门被拖住）。
        // 同一批内重复的刷新只保留一条，其余命令保持原有顺序。
        let mut batch = vec![cmd];
        for _ in 0..64 {
            match rx.try_recv() {
                Ok(more) => batch.push(more),
                Err(_) => break,
            }
        }
        let mut acct_seen = false;
        let mut orders_seen = [false; 2];
        batch.retain(|cmd| match cmd {
            Command::RefreshAccount => {
                if acct_seen {
                    false
                } else {
                    acct_seen = true;
                    true
                }
            }
            Command::RefreshOpenOrders { market } => {
                let i = *market as usize;
                if orders_seen[i] {
                    false
                } else {
                    orders_seen[i] = true;
                    true
                }
            }
            _ => true,
        });
        for cmd in batch {
            // 订单类命令并发派发：Subscribe 等刷新命令最坏要等 5 个 REST 超时
            // 才返回，串行会拖住下单/撤单——交易时效优先，直接 spawn 不阻塞循环
            match cmd {
                Command::PlaceOrder { market, req } => {
                    if let Some(client) = c.client.clone() {
                        let (st, cx, etx) = (state.clone(), ctx.clone(), engine_tx.clone());
                        tokio::spawn(async move {
                            let mut req = req;
                            place_order(&client, &st, market, &mut req, &etx).await;
                            st.set_busy("place", false);
                            repaint(&cx);
                        });
                    } else {
                        no_client_err(&state);
                        state.set_busy("place", false);
                    }
                }
                Command::PlaceOco { req } => {
                    if let Some(client) = c.client.clone() {
                        let (st, cx, etx) = (state.clone(), ctx.clone(), engine_tx.clone());
                        tokio::spawn(async move {
                            place_oco(&client, &st, &req, &etx).await;
                            st.set_busy("place", false);
                            repaint(&cx);
                        });
                    } else {
                        no_client_err(&state);
                        state.set_busy("place", false);
                    }
                }
                Command::CancelOrder {
                    market,
                    symbol,
                    order_id,
                } => {
                    if let Some(client) = c.client.clone() {
                        let (st, cx, etx) = (state.clone(), ctx.clone(), engine_tx.clone());
                        tokio::spawn(async move {
                            cancel_order(&client, &st, market, &symbol, order_id, &etx).await;
                            repaint(&cx);
                        });
                    } else {
                        no_client_err(&state);
                        state.set_busy("cancel", false);
                    }
                }
                Command::CancelAll { market, symbol } => {
                    if let Some(client) = c.client.clone() {
                        let (st, cx, etx) = (state.clone(), ctx.clone(), engine_tx.clone());
                        tokio::spawn(async move {
                            cancel_all_orders(&client, &st, market, symbol.as_deref(), &etx)
                                .await;
                            repaint(&cx);
                        });
                    } else {
                        no_client_err(&state);
                        state.set_busy("cancel", false);
                    }
                }
                other => handle_cmd(&ctx, &mut c, other, &sub_tx, &engine_tx).await,
            }
        }
    }
}

fn no_client_err(state: &SharedState) {
    state.push_toast(
        ToastKind::Err,
        crate::i18n::tr("REST 客户端初始化失败，请检查网络/代理设置"),
    );
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
                client.set_creds(creds.api_key.clone(), creds.api_secret.clone());
            }
            c.client = Some(Arc::new(client));
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
        no_client_err(&state);
        // busy 在命令入队时由 UI 置位，未执行的命令也必须释放；
        // 但只释放携带 busy 的命令——其他命令在此提前释放会把仍在
        // 排队的下单/撤单的忙标志清掉
        match cmd {
            Command::PlaceOrder { .. } => state.set_busy("place", false),
            Command::CancelOrder { .. } | Command::CancelAll { .. } => {
                state.set_busy("cancel", false);
            }
            _ => {}
        }
        return;
    };
    match cmd {
        Command::SyncTime => {
            // 周期校时（30min）：只校时，不重发订阅、不刷规则——
            // 旧实现复用 ReloadAll 分支且末尾无条件重发订阅，导致每 30min
            // 强制行情重连一次
            let cfg = state.cfg_snapshot();
            let mut all_ok = true;
            for m in Market::all() {
                let rc = RestCtx {
                    market: m,
                    testnet: cfg.testnet,
                    recv_window_ms: cfg.recv_window_ms,
                };
                if let Err(e) = client.sync_time(rc, &cfg).await {
                    tracing::warn!("校时失败({m}): {e}");
                    all_ok = false;
                }
            }
            // 校时成功即 REST 可达；失败可能是瞬时抖动，不翻转既有状态
            if all_ok {
                state.rest_ok.store(true, Ordering::Relaxed);
            }
        }
        Command::ReloadAll => {
            let cfg = state.cfg_snapshot();
            // 两个市场各校时一次（忽略失败）；rest_ok 取所有市场的合取，
            // 避免单一市场失败被另一市场成功覆盖
            let mut all_ok = true;
            for m in Market::all() {
                let rc = RestCtx {
                    market: m,
                    testnet: cfg.testnet,
                    recv_window_ms: cfg.recv_window_ms,
                };
                if let Err(e) = client.sync_time(rc, &cfg).await {
                    tracing::warn!("校时失败({m}): {e}");
                    all_ok = false;
                    state.push_toast(
                        ToastKind::Err,
                        format!("{}{}", crate::i18n::tr("连接 Binance 失败: "), e.friendly()),
                    );
                }
            }
            state.rest_ok.store(all_ok, Ordering::Relaxed);
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
                        state.rules_version.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        tracing::warn!("exchangeInfo 失败({m}): {e}");
                        let key = if m == Market::Spot {
                            "获取现货交易规则失败: "
                        } else {
                            "获取合约交易规则失败: "
                        };
                        state.push_toast(
                            ToastKind::Err,
                            format!("{}{}", crate::i18n::tr(key), e.friendly()),
                        );
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
            // 快照按 symbol 累积只增不删（每个 symbol 数百 KB）；同一时刻
            // 每市场只有一条活跃订阅，切换交易对时清掉同市场旧 symbol 的快照。
            // 先更新活跃订阅标记：with_symbol_md 只允许写入活跃 (market,symbol)，
            // 旧连接的在途帧不会复活已清空的槽位
            state.set_active_sub(market, &symbol);
            state.md.lock().slot(market).retain(|k, _| k == &symbol);
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
            // 先读磁盘缓存立即渲染（秒开）：REST 快照返回后再整体覆盖。
            // 缓存只在本市场本周期有效，切换交易对/周期时上面已清空旧数据
            load_kline_cache(&state, market, &symbol, interval.as_str());
            // 初始快照并发拉取（原先 5 个串行 await，网络差时切换交易对要等
            // 最坏 5×超时才出第一份数据）
            let kl_path = format!("{}/klines", market.api_prefix());
            let tk_path = format!("{}/ticker/24hr", market.api_prefix());
            let dp_path = format!("{}/depth", market.api_prefix());
            let at_path = format!("{}/aggTrades", market.api_prefix());
            let pi_path = format!("{}/premiumIndex", market.api_prefix());
            // 参数表先落局部变量：并发 future 会借用它们，临时值活不过语句结尾
            let kl_params = [
                ("symbol", symbol.as_str()),
                ("interval", interval.as_str()),
                ("limit", "500"),
            ];
            let one_params = [("symbol", symbol.as_str())];
            let dp_params = [("symbol", symbol.as_str()), ("limit", "20")];
            let at_params = [("symbol", symbol.as_str()), ("limit", "50")];
            let kl_f = client.get_public(rc, &cfg, &kl_path, &kl_params);
            let tk_f = client.get_public(rc, &cfg, &tk_path, &one_params);
            let dp_f = client.get_public(rc, &cfg, &dp_path, &dp_params);
            let at_f = client.get_public(rc, &cfg, &at_path, &at_params);
            let pi_f = async {
                if market == Market::UmFutures {
                    client
                        .get_public(rc, &cfg, &pi_path, &one_params)
                        .await
                        .ok()
                } else {
                    None
                }
            };
            let (kl, tk, dp, at, pi) = tokio::join!(kl_f, tk_f, dp_f, at_f, pi_f);
            match kl {
                Ok(v) => {
                    let rows = v.as_array().cloned().unwrap_or_default();
                    let candles: Vec<Candle> = rows
                        .iter()
                        .filter_map(|r| r.as_array().and_then(|a| parse_kline_row(a)))
                        .collect();
                    save_kline_cache(market, &symbol, interval.as_str(), &candles);
                    state.with_symbol_md(market, &symbol, |md| {
                        apply_kline_snapshot(md, candles);
                    });
                }
                Err(e) => {
                    state.push_toast(
                        ToastKind::Err,
                        format!("{}{}", crate::i18n::tr("加载K线失败: "), e.friendly()),
                    );
                }
            }
            if let Ok(v) = tk {
                if let Ok(t) = serde_json::from_value::<Ticker24>(v) {
                    state.with_symbol_md(market, &symbol, |md| md.ticker = Some(t));
                }
            }
            if let Ok(v) = dp {
                if let Ok(d) = serde_json::from_value::<Depth>(v) {
                    state.with_symbol_md(market, &symbol, |md| md.depth = d);
                }
            }
            if let Ok(v) = at {
                if let Ok(list) = serde_json::from_value::<Vec<TradeTick>>(v) {
                    state.with_symbol_md(market, &symbol, |md| {
                        md.trades = list.into_iter().collect();
                    });
                }
            }
            if let Some(v) = pi {
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
            }
            repaint(ctx);
        }
        Command::RefreshAccount => {
            // 账户 REST ≈11 权重/次；ACCOUNT_UPDATE 事件风暴期间逐条刷新会
            // 打满限速——距上次刷新不足 2s 直接丢弃（WS 推送本身保证最终一致）
            if let Some(t) = c.last_acct_refresh {
                if t.elapsed() < Duration::from_secs(2) {
                    return;
                }
            }
            c.last_acct_refresh = Some(Instant::now());
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
                            acct.spot_err = None;
                            repaint(ctx);
                        }
                        Err(e) => tracing::warn!("解析现货账户失败: {e}"),
                    },
                    Err(e) => {
                        // 注意：REST 失败不能标记 user_ws 断开——它只代表本轮
                        // REST 快照失败，用户数据流可能仍然健康
                        state.acct.lock().spot_err = Some(e.friendly());
                        if !matches!(e, BinanceError::NoCreds) {
                            state.push_toast(
                                ToastKind::Err,
                                format!("{}{}", crate::i18n::tr("现货账户: "), e.friendly()),
                            );
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
                    // v3 相比 v2 多返回损益两平价（breakEvenPrice），
                    // 且只返回有持仓/挂单的 symbol（UI 本就过滤零持仓）；
                    // 注意 v3 已移除 leverage 等配置字段，杠杆另查 symbolConfig
                    "/fapi/v3/positionRisk",
                    vec![],
                );
                // 杠杆/保证金模式等配置（v3 positionRisk 不再返回）
                let sc_f = client.signed(
                    reqwest::Method::GET,
                    rc,
                    &cfg,
                    "/fapi/v1/symbolConfig",
                    vec![],
                );
                let dual_f = client.signed(
                    reqwest::Method::GET,
                    rc,
                    &cfg,
                    "/fapi/v1/positionSide/dual",
                    vec![],
                );
                let (a, p, sc, d) = tokio::join!(acct_f, pr_f, sc_f, dual_f);
                let mut ok = true;
                let mut fut_err: Option<String> = None;
                match a {
                    Ok(v) => match serde_json::from_value::<FutAccount>(v) {
                        Ok(acc) => {
                            let mut acct = state.acct.lock();
                            acct.fut = Some(acc);
                        }
                        Err(e) => {
                            tracing::warn!("解析合约账户失败: {e}");
                            ok = false;
                        }
                    },
                    Err(e) => {
                        ok = false;
                        fut_err = Some(e.friendly());
                        if !matches!(e, BinanceError::NoCreds) {
                            state.push_toast(
                                ToastKind::Err,
                                format!("{}{}", crate::i18n::tr("合约账户: "), e.friendly()),
                            );
                        }
                    }
                }
                if let Ok(v) = sc {
                    // 杠杆/保证金模式配置整体替换：与服务器状态保持一致
                    // （SetLeverage/SetMarginType 本地写入的值会被下一次
                    // RefreshAccount 的服务器值校正）
                    if let Ok(list) = serde_json::from_value::<Vec<SymbolConfig>>(v) {
                        let mut acct = state.acct.lock();
                        acct.fut_leverage = list
                            .iter()
                            .map(|x| (x.symbol.clone(), x.leverage))
                            .collect();
                        acct.fut_margin_type = list
                            .into_iter()
                            .map(|x| (x.symbol, x.margin_type))
                            .collect();
                    }
                }
                if let Ok(v) = p {
                    if let Ok(mut list) = serde_json::from_value::<Vec<PositionRisk>>(v) {
                        // v3 响应无 leverage 字段（默认 0）：从 fut_leverage 补齐，
                        // 供持仓表杠杆列展示
                        let mut acct = state.acct.lock();
                        for r in &mut list {
                            if let Some(l) = acct.fut_leverage.get(&r.symbol) {
                                r.leverage = *l;
                            }
                        }
                        acct.fut_positions = list;
                    }
                }
                if let Ok(v) = d {
                    if let Ok(m) = serde_json::from_value::<DualSideMode>(v) {
                        state.acct.lock().fut_dual_side = m.dual;
                    }
                }
                state.acct.lock().fut_err = if ok { None } else { fut_err };
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
                        // 挂单获取失败若静默，UI 会显示"暂无挂单"空态，
                        // 让用户误以为没有在挂的保护单——必须提示
                        tracing::warn!("挂单获取失败({market}): {e}");
                        state.push_toast(
                            ToastKind::Err,
                            format!(
                                "{}{}: {}",
                                crate::i18n::tr("挂单获取失败"),
                                market.label(),
                                e.friendly()
                            ),
                        );
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
                let (po_ok, pt_ok) = (po.is_ok(), pt.is_ok());
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
                o.history_symbol = if po_ok || pt_ok {
                    // 至少一个请求成功才更新归属标记：双双失败时保留旧数据 +
                    // 旧标记，避免表头显示新交易对而内容仍是旧交易对
                    Some((market, symbol))
                } else {
                    o.history_symbol.take()
                };
            }
            repaint(ctx);
        }
        // PlaceOrder/CancelOrder/CancelAll 已在 cmd_loop 中并发派发，
        // 不会走到 handle_cmd（缺省分支由编译器穷尽性兜底）
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
                    state.push_toast(
                        ToastKind::Ok,
                        format!("{symbol} {} {leverage}x", crate::i18n::tr("杠杆已设为")),
                    );
                    engine_tx.send(Command::RefreshAccount).ok();
                }
                Err(e) => toast_err(&state, &e),
            }
            repaint(ctx);
        }
        Command::SetMarginType { symbol, isolated } => {
            if !state.has_creds() {
                return;
            }
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market: Market::UmFutures,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let target = if isolated { "ISOLATED" } else { "CROSSED" };
            let params = vec![
                ("symbol".into(), symbol.clone()),
                ("marginType".into(), target.to_string()),
            ];
            let res = client
                .signed(reqwest::Method::POST, rc, &cfg, "/fapi/v1/marginType", params)
                .await;
            match res {
                Ok(_) => {
                    state.acct.lock().fut_margin_type.insert(symbol.clone(), target.into());
                    state.push_toast(
                        ToastKind::Ok,
                        format!(
                            "{symbol} {}{target}",
                            crate::i18n::tr("保证金模式已切为 ")
                        ),
                    );
                    engine_tx.send(Command::RefreshAccount).ok();
                }
                // -4046 = 无需修改（已是目标模式），按成功提示
                Err(BinanceError::Api {
                    code: -4046,
                    ..
                }) => {
                    state.push_toast(
                        ToastKind::Info,
                        crate::i18n::tr("保证金模式无需修改").to_string(),
                    );
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
        Command::PersistConfig => {
            // 配置落盘统一在引擎侧执行，UI 线程只改内存
            let cfg = state.cfg_snapshot();
            cfg.save(&crate::config::data_dir());
        }
        Command::RefreshKlines {
            market,
            symbol,
            interval,
        } => {
            // 断线重连后补拉 K 线快照：断线期间可能跨了多根 K 线，
            // WS 只推当前根，补快照消除图表缺口与 MA 失真。
            // 用户可能已切走交易对（Subscribe 清掉了旧槽位）：
            // 槽位不存在直接跳过，避免 entry().or_default() 复活旧槽
            if state.md.lock().get(market, &symbol).is_none() {
                return;
            }
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let kl_path = format!("{}/klines", market.api_prefix());
            let kl_params = [
                ("symbol", symbol.as_str()),
                ("interval", interval.as_str()),
                ("limit", "500"),
            ];
            match client.get_public(rc, &cfg, &kl_path, &kl_params).await {
                Ok(v) => {
                    let rows = v.as_array().cloned().unwrap_or_default();
                    let candles: Vec<Candle> = rows
                        .iter()
                        .filter_map(|r| r.as_array().and_then(|a| parse_kline_row(a)))
                        .collect();
                    save_kline_cache(market, &symbol, interval.as_str(), &candles);
                    state.with_symbol_md(market, &symbol, |md| {
                        apply_kline_snapshot(md, candles);
                    });
                }
                Err(e) => tracing::warn!("补拉K线快照失败({market} {symbol}): {e}"),
            }
        }
        // 订单类命令已在 cmd_loop 中并发派发，不会进入 handle_cmd
        Command::PlaceOrder { .. }
        | Command::CancelOrder { .. }
        | Command::CancelAll { .. }
        | Command::PlaceOco { .. } => {}
    }
}

/// 下单：引擎侧兜底量化与校验 + POST。busy 的置位/释放由调用方（命令入口）负责。
async fn place_order(
    client: &BinanceRest,
    state: &Arc<SharedState>,
    market: Market,
    req: &mut OrderRequest,
    engine_tx: &mpsc::UnboundedSender<Command>,
) {
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
            format!(
                "{} {}{}",
                crate::i18n::tr("缺少"),
                req.symbol,
                crate::i18n::tr(" 交易规则（等待行情初始化）")
            ),
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
            OrderKind::TrailingStop { callback_rate, qty } => {
                // 回调幅度一位小数（0.1 步进），只需量化数量
                *callback_rate = callback_rate.round_dp(1);
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
        state.push_toast(
            ToastKind::Warn,
            format!("{}: {}", crate::i18n::tr("数量过小"), r.step.normalize()),
        );
        return;
    }
    let params = build_order_params(market, req);
    let rc = RestCtx {
        market,
        testnet: cfg.testnet,
        recv_window_ms: cfg.recv_window_ms,
    };
    // WS-API 通道（可选，仅合约）：任何失败自动回退 REST，资金路径不赌单通道
    if market == Market::UmFutures && cfg.ws_order {
        let url = crate::binance::ws_api_order(cfg.testnet);
        let proxy = cfg.proxy.clone();
        match client
            .place_order_ws(url, &proxy, market, req, cfg.recv_window_ms)
            .await
        {
            Ok(v) => {
                let id = v.get("orderId").and_then(|x| x.as_i64()).unwrap_or(0);
                tracing::info!("WS 下单成功 {} {:?} id={id}", req.symbol, req.side);
                if !note.is_empty() {
                    state.push_toast(ToastKind::Warn, note);
                }
                state.push_toast(
                    ToastKind::Ok,
                    format!(
                        "{} {} {}{id})",
                        req.symbol,
                        req.side.label(),
                        crate::i18n::tr("委托已提交 (#")
                    ),
                );
                engine_tx.send(Command::RefreshOpenOrders { market }).ok();
                engine_tx.send(Command::RefreshAccount).ok();
                return;
            }
            Err(e) => {
                tracing::warn!("WS 下单失败，回退 REST: {e}");
                state.push_toast(
                    ToastKind::Warn,
                    format!(
                        "{}{e}",
                        crate::i18n::tr("WS 下单失败，已回退 REST: ")
                    ),
                );
            }
        }
    }
    let path = format!("{}/order", market.api_prefix());
    let res = client
        .signed(reqwest::Method::POST, rc, &cfg, &path, params)
        .await;
    match res {
        Ok(v) => {
            let id = v.get("orderId").and_then(|x| x.as_i64()).unwrap_or(0);
            tracing::info!("下单成功 {} {:?} id={id}", req.symbol, req.side);
            if !note.is_empty() {
                state.push_toast(ToastKind::Warn, note);
            }
            state.push_toast(
                ToastKind::Ok,
                format!(
                    "{} {} {}{id})",
                    req.symbol,
                    req.side.label(),
                    crate::i18n::tr("委托已提交 (#")
                ),
            );
            engine_tx.send(Command::RefreshOpenOrders { market }).ok();
            engine_tx.send(Command::RefreshAccount).ok();
        }
        Err(e) => {
            toast_err(state, &e);
            // 超时等场景下订单可能已被交易所接受：立即对账，
            // 避免用户看不到第一笔委托而重复下单
            engine_tx.send(Command::RefreshOpenOrders { market }).ok();
            engine_tx.send(Command::RefreshAccount).ok();
        }
    }
}

/// 撤销单笔委托（busy 置位/释放由本函数负责；命令循环可并发派发）
async fn cancel_order(
    client: &BinanceRest,
    state: &Arc<SharedState>,
    market: Market,
    symbol: &str,
    order_id: i64,
    engine_tx: &mpsc::UnboundedSender<Command>,
) {
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
        ("symbol".into(), symbol.to_string()),
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
        Err(e) => toast_err(state, &e),
    }
}

/// 现货 OCO 下单：限价腿 + 止损限价腿（busy 置位/释放由调用方负责）
async fn place_oco(
    client: &BinanceRest,
    state: &Arc<SharedState>,
    req: &crate::binance::client::OcoRequest,
    engine_tx: &mpsc::UnboundedSender<Command>,
) {
    if !state.has_creds() {
        state.push_toast(ToastKind::Err, crate::i18n::tr("未配置 API 密钥"));
        return;
    }
    let cfg = state.cfg_snapshot();
    let rc = RestCtx {
        market: Market::Spot,
        testnet: cfg.testnet,
        recv_window_ms: cfg.recv_window_ms,
    };
    // 作用域内取规则：parking_lot 读守卫非 Send，不能活到下方 await
    let r = {
        let rules = state.rules.read();
        rules[Market::Spot as usize].get(&req.symbol).cloned()
    };
    let Some(r) = r else {
        state.push_toast(
            ToastKind::Err,
            format!(
                "{} {}{}",
                crate::i18n::tr("缺少"),
                req.symbol,
                crate::i18n::tr(" 交易规则（等待行情初始化）")
            ),
        );
        return;
    };
    let mut note = String::new();
    let mut req = req.clone();
    let quantized: Option<()> = (|| {
        req.price = quantize_with_note(req.price, r.tick, true, &mut note)?;
        req.stop_price = quantize_with_note(req.stop_price, r.tick, true, &mut note)?;
        req.stop_limit_price =
            quantize_with_note(req.stop_limit_price, r.tick, true, &mut note)?;
        req.qty = quantize_with_note(req.qty, r.step, false, &mut note)?;
        Some(())
    })();
    if quantized.is_none() {
        state.push_toast(
            ToastKind::Err,
            crate::i18n::tr("价格/数量超出可表示范围，请检查输入"),
        );
        return;
    }
    state.set_busy("place", true);
    let res = client
        .signed(
            reqwest::Method::POST,
            rc,
            &cfg,
            "/api/v3/order/oco",
            crate::binance::client::build_oco_params(&req),
        )
        .await;
    state.set_busy("place", false);
    match res {
        Ok(v) => {
            let id = v.get("orderListId").and_then(|x| x.as_i64()).unwrap_or(0);
            tracing::info!("OCO 已提交 {} {:?} listId={id}", req.symbol, req.side);
            if !note.is_empty() {
                state.push_toast(ToastKind::Warn, note);
            }
            state.push_toast(
                ToastKind::Ok,
                format!(
                    "{} OCO {}{id})",
                    req.symbol,
                    crate::i18n::tr("委托已提交 (#")
                ),
            );
            engine_tx
                .send(Command::RefreshOpenOrders {
                    market: Market::Spot,
                })
                .ok();
        }
        Err(e) => {
            toast_err(state, &e);
            engine_tx
                .send(Command::RefreshOpenOrders {
                    market: Market::Spot,
                })
                .ok();
        }
    }
}

/// 撤销挂单（symbol=None 撤销该市场全部；busy 置位/释放由本函数负责，
/// 命令循环可并发派发）
async fn cancel_all_orders(
    client: &BinanceRest,
    state: &Arc<SharedState>,
    market: Market,
    symbol: Option<&str>,
    engine_tx: &mpsc::UnboundedSender<Command>,
) {
    if !state.has_creds() {
        return;
    }
    let cfg = state.cfg_snapshot();
    let rc = RestCtx {
        market,
        testnet: cfg.testnet,
        recv_window_ms: cfg.recv_window_ms,
    };
    let (path, mut params) = match market {
        Market::Spot => (
            format!("{}/openOrders", market.api_prefix()),
            vec![],
        ),
        Market::UmFutures => (
            format!("{}/allOpenOrders", market.api_prefix()),
            vec![],
        ),
    };
    if let Some(sym) = symbol {
        params.push(("symbol".into(), sym.to_string()));
    }
    state.set_busy("cancel", true);
    let res = client
        .signed(reqwest::Method::DELETE, rc, &cfg, &path, params)
        .await;
    state.set_busy("cancel", false);
    match res {
        Ok(_) => {
            let msg = match symbol {
                Some(s) => format!("{} {s}", crate::i18n::tr("已撤销")),
                None => crate::i18n::tr("已撤销本市场全部挂单").to_string(),
            };
            state.push_toast(ToastKind::Ok, msg);
            engine_tx.send(Command::RefreshOpenOrders { market }).ok();
        }
        Err(e) => toast_err(state, &e),
    }
}

// ---------------------------------------------------------------- 行情 WebSocket

/// 期货行情连接槽位：Binance 自 2026-03 起把 USDⓈ-M WebSocket 拆为
/// /public /market /private 三类入口，legacy 入口只剩 public 类
/// （depth/bookTicker）继续推送，成交/K线/ticker/标记价必须走 /market。
/// 现货无此拆分：槽位 A 单连接承载全部流，槽位 B 空转
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarketWsSlot {
    A,
    B,
}

impl MarketWsSlot {
    fn idx(self) -> usize {
        match self {
            MarketWsSlot::A => 0,
            MarketWsSlot::B => 1,
        }
    }

    /// 本槽位的符号作用域流（随交易对/周期变化）。
    /// 独立于 `streams_and_url` 拆出：原地改订（UNSUBSCRIBE/SUBSCRIBE）只动
    /// 这些，`!miniTicker@arr` 这类市场级流随初始连接建立后永不重订
    fn scoped_streams(&self, lower: &str, interval: &str, fut: bool) -> Vec<String> {
        match (self, fut) {
            // 现货槽位 A 承载全部符号流。深度取 20 档（@depth20@100ms）：
            // 现货部分深度流只支持 5/10/20 档——50 档为合约（fapi）专属，
            // 现货订阅 @depth50 会被服务端忽略，盘口静默停止推送
            (MarketWsSlot::A, false) => vec![
                format!("{lower}@ticker"),
                format!("{lower}@depth20@100ms"),
                format!("{lower}@aggTrade"),
                format!("{lower}@kline_{interval}"),
            ],
            // 期货 public 类：部分深度（bookTicker 未用，不订阅）。
            // 深度同样取 20 档（@depth20@100ms）：50 档在本环境下与现货
            // 一样被服务端静默忽略——连接成功、状态正常，但深度帧永不到达
            // （盘口定格）。代价仅是开启大倍数聚合档位时桶密度变稀，
            // 数据可用性优先于聚合密度
            (MarketWsSlot::A, true) => vec![format!("{lower}@depth20@100ms")],
            // 期货 market 类：成交/K线/ticker/标记价
            (MarketWsSlot::B, true) => vec![
                format!("{lower}@ticker"),
                format!("{lower}@aggTrade"),
                format!("{lower}@kline_{interval}"),
                format!("{lower}@markPrice@1s"),
            ],
            // 现货 B 槽位不建连接（调用方跳过该订阅周期）
            (MarketWsSlot::B, false) => Vec::new(),
        }
    }

    /// 本槽位的流列表与组合流 URL（base 为市场 WS 根端点，期货为
    /// fstream.binance.com[.testnet]，可被 override_fut_ws 覆盖）
    fn streams_and_url(
        self,
        base: &str,
        lower: &str,
        interval: &str,
        fut: bool,
    ) -> (Vec<String>, String) {
        let mut s = self.scoped_streams(lower, interval, fut);
        // 全市场迷你 ticker（侧栏实时价格）只随初始连接订阅一次，
        // 交易对切换时不重订——它本来就不属于任何单个交易对
        if matches!((self, fut), (MarketWsSlot::A, false) | (MarketWsSlot::B, true)) {
            s.push("!miniTicker@arr".to_string());
        }
        if s.is_empty() {
            return (Vec::new(), String::new());
        }
        // 期货 2026-03 拆流后端点路径按槽位不同：A 走 /public/stream（深度），
        // B 走 /market/stream（成交/K线/ticker/标记价）；现货 legacy 单入口 /stream
        let path = match (self, fut) {
            (MarketWsSlot::A, false) => "/stream",
            (MarketWsSlot::A, true) => "/public/stream",
            (MarketWsSlot::B, true) => "/market/stream",
            (MarketWsSlot::B, false) => return (Vec::new(), String::new()),
        };
        let url = format!("{base}{path}?streams={}", s.join("/"));
        (s, url)
    }
}

/// 双槽行情连接状态合并（UI/状态栏仍按单一市场状态展示）：
/// 任一 Failed → Failed；任一 Connecting → Connecting；其余（含现货
/// B 槽未使用的 None）视为已连接；兜底 Disconnected
#[derive(Default)]
struct WsPairState {
    st: [Option<ConnState>; 2],
}

impl WsPairState {
    fn set(&mut self, slot: usize, market: Market, s: ConnState) -> ConnState {
        // 现货只走槽位 A：一旦为现货写入状态，立即清掉槽位 B 残留的上一次
        // 合约会话状态。否则切换回现货时，B 槽位的陈旧 Disconnected/Failed
        // 会参与合并（seen=2），把「槽位 A 已连接」的现货判成未连接——
        // 数据正常流入但状态栏恒显示未连接，正是由此而来
        if market == Market::Spot {
            self.st[MarketWsSlot::B.idx()] = None;
        }
        self.st[slot] = Some(s);
        // None = 该槽位对当前市场不适用（现货 B 槽），不参与合并；
        // 失败优先展示，其次连接中，全部已连接才算已连接
        let mut failed = None;
        let mut connecting = false;
        let mut connected = 0usize;
        let mut seen = 0usize;
        for st in self.st.iter().flatten() {
            seen += 1;
            match st {
                ConnState::Failed(m) => failed = Some(m.clone()),
                ConnState::Connecting => connecting = true,
                ConnState::Connected => connected += 1,
                ConnState::Disconnected => {}
            }
        }
        if seen == 0 {
            return ConnState::Disconnected;
        }
        if let Some(m) = failed {
            return ConnState::Failed(m);
        }
        if connecting {
            return ConnState::Connecting;
        }
        if connected == seen {
            return ConnState::Connected;
        }
        ConnState::Disconnected
    }

    /// 清空槽位状态（现货下槽位 B 空转前自我清场，避免陈旧状态泄漏）
    fn clear(&mut self, slot: usize) {
        self.st[slot] = None;
    }
}

async fn market_ws_loop(
    ctx: egui::Context,
    state: Arc<SharedState>,
    mut rx_sub: watch::Receiver<Option<SubSpec>>,
    pair: Arc<parking_lot::Mutex<WsPairState>>,
    slot: MarketWsSlot,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        // 等待首个订阅
        let mut spec = loop {
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
        // 现货只需槽位 A 一条连接；B 槽静候下一次订阅变化。
        // 等待前先清掉自己的陈旧状态：上一次合约会话的 Disconnected/Failed
        // 若留着，下一次槽位 A 为现货写状态时仍会被合并进来
        if slot == MarketWsSlot::B && spec.market != Market::UmFutures {
            pair.lock().clear(slot.idx());
            if rx_sub.changed().await.is_err() {
                return;
            }
            continue;
        }
        let cfg = state.cfg_snapshot();
        let ep = endpoints(spec.market, cfg.testnet, &cfg);
        let lower = spec.symbol.to_lowercase();
        let fut = spec.market == Market::UmFutures;
        let (_, url) = slot.streams_and_url(&ep.ws, &lower, spec.interval.as_str(), fut);

        let st = pair.lock().set(slot.idx(), spec.market, ConnState::Connecting);
        state.set_market_ws(spec.market, st.clone());
        state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
        repaint(&ctx);

        match crate::binance::ws::connect(&url, &cfg.proxy).await {
            Err(e) => {
                tracing::warn!("行情WS连接失败({}): {e}", spec.market);
                let msg = short_err(&e);
                let st = pair.lock().set(slot.idx(), spec.market, ConnState::Failed(msg.clone()));
                state.set_market_ws(spec.market, st.clone());
                state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
                repaint(&ctx);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            Ok(ws) => {
                tracing::info!("行情WS已连接: {}", spec.market);
                let connected_at = std::time::Instant::now();
                let st = pair.lock().set(slot.idx(), spec.market, ConnState::Connected);
                state.set_market_ws(spec.market, st.clone());
                state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
                repaint(&ctx);

                let (mut write, mut read) = futures_util::StreamExt::split(ws);
                // 服务端当下已订阅的流（含随连接建立的 !miniTicker@arr）。
                // 原地改订按差集计算增量：市场级流（'!' 前缀）永远不退订
                let mut server_streams =
                    slot.streams_and_url(&ep.ws, &lower, spec.interval.as_str(), fut).0;
                let version = last_version(&state);
                // 记录读循环退出原因：出错时保留 Failed 状态（不被下方 Disconnected 覆盖）
                let mut read_fail: Option<String> = None;
                let mut eof = false;
                // 主动断开（跨市场切换/取消订阅）：不计退避
                let mut intentional = false;
                // 原地改订的活性看门狗：到期未收到新符号的符号级帧即判定
                // 服务端忽略了 SUBSCRIBE（组合流端点理论支持原地改订，但
                // 无法离线验证），退回整连兜底
                let mut resub_dl: Option<Instant> = None;
                // 控制帧 id：同一连接内单调递增，便于对照服务端应答
                let mut req_id: u64 = 0;
                loop {
                    // 循环顶部比对配置版本，避免被高频行情消息饿死
                    if last_version(&state) != version {
                        intentional = true;
                        break;
                    }
                    // 看门狗截止时间本帧快照（Copy）：分支 body 要写 resub_dl，
                    // async 分支须 move 捕获避免跨分支借用冲突
                    let dl = resub_dl;
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                None => {
                                    eof = true;
                                    break;
                                }
                                Some(Err(e)) => {
                                    tracing::warn!("行情WS错误: {e}");
                                    let msg = short_err(&e.to_string());
                                    read_fail = Some(msg.clone());
                                    let st = pair
                                        .lock()
                                        .set(slot.idx(), spec.market, ConnState::Failed(msg.clone()));
                                    state.with_symbol_md(spec.market, &spec.symbol, |md| {
                                        md.ws = st
                                    });
                                    break;
                                }
                                Some(Ok(m)) => {
                                    // 组合流载荷按帧类型统一取 UTF-8 文本
                                    // （实测均为 text 帧；Binary 分支留作
                                    // 服务端行为变化的兜底）
                                    let txt = match m {
                                        tokio_tungstenite::tungstenite::Message::Text(t) => {
                                            Some(t)
                                        }
                                        tokio_tungstenite::tungstenite::Message::Binary(b) => {
                                            String::from_utf8(b).ok()
                                        }
                                        tokio_tungstenite::tungstenite::Message::Ping(p) => {
                                            // 主动回 Pong：不依赖服务端单向心跳，
                                            // 部分代理仅在有客户端 Pong 时保活连接。
                                            // 与改订控制帧同等待遇：sink 阻塞 3s
                                            // 即判写通道已死，read_fail 置位后由
                                            // 循环底部跳出去走重连
                                            let pong = tokio::time::timeout(
                                                Duration::from_secs(3),
                                                write.send(
                                                    tokio_tungstenite::tungstenite::Message::Pong(p),
                                                ),
                                            )
                                            .await;
                                            if !matches!(pong, Ok(Ok(()))) {
                                                read_fail = Some(
                                                    crate::i18n::tr("Pong 发送失败").to_string(),
                                                );
                                            }
                                            None
                                        }
                                        _ => None,
                                    };
                                    // Ping 分支置位 read_fail 后立即退出循环
                                    if read_fail.is_some() {
                                        break;
                                    }
                                    if let Some(txt) = txt {
                                        if handle_market_msg_text(&state, &spec, &txt) {
                                            // 新符号的符号级帧到达 → 改订已生效
                                            resub_dl = None;
                                        }
                                        // 行情帧可达每秒数百条（depth20@100ms +
                                        // aggTrade），逐帧立即重绘会让 UI 在无输入
                                        // 时也持续满帧率空转；合帧到 ~10fps，
                                        // 状态变迁/用户输入仍走立即重绘
                                        ctx.request_repaint_after(Duration::from_millis(100));
                                    }
                                }
                            }
                        }
                        _ = rx_sub.changed() => {
                            // 订阅变化：同市场 → 原地改订（UNSUBSCRIBE/SUBSCRIBE
                            // 控制帧，连接复用，切换免一次 TCP+TLS+WSS 握手）；
                            // 跨市场（端点主机不同）或槽位不适用 → 整连
                            let Some(ns) = rx_sub.borrow_and_update().clone() else {
                                intentional = true;
                                break;
                            };
                            if ns.market != spec.market
                                || (slot == MarketWsSlot::B && ns.market != Market::UmFutures)
                            {
                                intentional = true;
                                break;
                            }
                            // 改订期间 UI 侧已把新交易对 ws 置 Connecting，
                            // 控制帧发出后连接本身仍健康，拉回 Connected
                            let st = pair.lock().set(slot.idx(), ns.market, ConnState::Connected);
                            state.set_market_ws(ns.market, st.clone());
                            state.with_symbol_md(ns.market, &ns.symbol, |md| md.ws = st);
                            let nl = ns.symbol.to_lowercase();
                            let new_scoped = slot.scoped_streams(&nl, ns.interval.as_str(), fut);
                            let drop_s: Vec<String> = server_streams
                                .iter()
                                .filter(|s| !s.starts_with('!') && !new_scoped.contains(s))
                                .cloned()
                                .collect();
                            let add_s: Vec<String> = new_scoped
                                .iter()
                                .filter(|s| !server_streams.contains(s))
                                .cloned()
                                .collect();
                            if drop_s.is_empty() && add_s.is_empty() {
                                // 同交易对同周期重复下发（如 ReloadAll 重发订阅）：
                                // 流无变化，只更新本地规格
                                spec = ns;
                                repaint(&ctx);
                                continue;
                            }
                            req_id += 1;
                            let unsub = serde_json::json!({
                                "method": "UNSUBSCRIBE",
                                "params": drop_s,
                                "id": req_id,
                            })
                            .to_string();
                            req_id += 1;
                            let sub = serde_json::json!({
                                "method": "SUBSCRIBE",
                                "params": add_s,
                                "id": req_id,
                            })
                            .to_string();
                            // 发送超时即判定连接已死： sink 阻塞会把整个读循环卡住
                            let sent = tokio::time::timeout(
                                Duration::from_secs(3),
                                async {
                                    write
                                        .send(tokio_tungstenite::tungstenite::Message::Text(unsub))
                                        .await?;
                                    write
                                        .send(tokio_tungstenite::tungstenite::Message::Text(sub))
                                        .await
                                },
                            )
                            .await;
                            if !matches!(sent, Ok(Ok(()))) {
                                read_fail =
                                    Some(crate::i18n::tr("改订命令发送失败").to_string());
                                break;
                            }
                            server_streams.retain(|s| !drop_s.contains(s));
                            server_streams.extend(add_s.iter().cloned());
                            tracing::info!(
                                "行情WS原地改订({}): {} -> {}",
                                spec.market,
                                spec.symbol,
                                ns.symbol
                            );
                            spec = ns;
                            repaint(&ctx);
                            resub_dl = Some(Instant::now() + Duration::from_secs(6));
                        }
                        // 改订活性看门狗：None 时永久挂起，不参与 select 竞争
                        _ = async move {
                            match dl {
                                Some(d) => {
                                    tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await
                                }
                                None => std::future::pending::<()>().await,
                            }
                        } => {
                            read_fail = Some(
                                crate::i18n::tr("切换订阅后未收到数据，重连").to_string(),
                            );
                            break;
                        }
                        // 空闲看门狗：行情流常有 100ms 深度推送，90s 无任何帧
                        // （含协议 ping）基本可判定为半开连接，强制走重连
                        _ = tokio::time::sleep(Duration::from_secs(90)) => {
                            read_fail = Some(crate::i18n::tr("读超时（连接无数据）").to_string());
                            break;
                        }
                    }
                }
                // 会话异常结束（错误/EOF/读超时）后补拉一次 K 线快照：
                // 断线期间可能跨了多根 K 线，WS 只推当前根，补快照消除缺口
                if read_fail.is_some() || eof {
                    state.send_cmd(Command::RefreshKlines {
                        market: spec.market,
                        symbol: spec.symbol.clone(),
                        interval: spec.interval,
                    });
                }
                if let Some(msg) = read_fail {
                    let st = pair.lock().set(slot.idx(), spec.market, ConnState::Failed(msg));
                    state.set_market_ws(spec.market, st.clone());
                    state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
                } else if !intentional {
                    let st = pair.lock().set(slot.idx(), spec.market, ConnState::Disconnected);
                    state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
                } else {
                    // 主动断开：状态由下一轮连接（或订阅方）接管，这里不写
                    // Disconnected 覆盖即将到来的 Connecting
                }
                // 连接未稳定（<2min）即断开 → 指数退避，防止对端抖动时重连风暴；
                // 稳定运行过的连接、以及主动改订/配置变更引起的断开则重置退避
                if intentional || connected_at.elapsed() >= Duration::from_secs(120) {
                    backoff = Duration::from_secs(1);
                } else {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
        if !matches!(state.md.lock().market_ws(spec.market), ConnState::Failed(_)) {
            let st = pair.lock().set(slot.idx(), spec.market, ConnState::Disconnected);
            state.set_market_ws(spec.market, st);
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

/// 写入 REST K 线快照并递增版本号。
/// 若本地最后一根（WS 持续推送的在收蜡烛）与快照最后一根同开盘时间，则保留
/// 本地版本——REST 快照发出到返回之间存在延迟，直接整体替换会把图表回滚到
/// 快照发出时刻（缺口短暂重现）。
fn apply_kline_snapshot(md: &mut crate::engine::state::SymbolMd, candles: Vec<Candle>) {
    let old_last = md.klines.last().cloned();
    let new_last_t = candles.last().map(|c| c.t);
    md.klines = candles;
    md.kline_version = md.kline_version.wrapping_add(1);
    if let (Some(o), Some(t)) = (old_last, new_last_t) {
        if o.t == t {
            if let Some(last) = md.klines.last_mut() {
                *last = o;
            }
        }
    }
}

// ---------------------------------------------------------------- K 线磁盘缓存

/// 缓存文件路径：cache/klines/{市场}_{交易对}_{周期}.json。
/// 切换交易对时先读缓存立即渲染（秒开），REST 快照随后覆盖
fn kline_cache_path(market: Market, symbol: &str, interval: &str) -> Option<std::path::PathBuf> {
    let dir = crate::config::data_dir().join("cache").join("klines");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("{}_{}_{}.json", market as u8, symbol, interval)))
}

/// 写缓存（临时文件 + 原子改名），保留最近 1000 根
fn save_kline_cache(market: Market, symbol: &str, interval: &str, candles: &[Candle]) {
    const CAP: usize = 1000;
    let Some(path) = kline_cache_path(market, symbol, interval) else {
        return;
    };
    let start = candles.len().saturating_sub(CAP);
    let body = match serde_json::to_string(&candles[start..]) {
        Ok(s) => s,
        Err(_) => return,
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// 读缓存并应用到活跃订阅槽位（须在 active_sub 设置之后调用）
fn load_kline_cache(state: &SharedState, market: Market, symbol: &str, interval: &str) {
    let Some(path) = kline_cache_path(market, symbol, interval) else {
        return;
    };
    let candles: Vec<Candle> = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Candle>>(&s).ok())
    {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };
    state.with_symbol_md(market, symbol, |md| apply_kline_snapshot(md, candles));
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

/// 组合流帧：`{"stream":"<名称>","data":{...}}`。
/// data 用借用的 RawValue——整条消息只解析一次，按流名分发后对 data
/// 做第二段定点解析，替代旧的 Value 全树解析 + 子树深克隆。
#[derive(serde::Deserialize)]
struct StreamFrame<'a> {
    #[serde(borrow)]
    stream: &'a str,
    #[serde(borrow)]
    data: &'a serde_json::value::RawValue,
}

/// 分发一条组合流文本帧。
///
/// 返回值 = 是否为**当前订阅符号**的符号级数据帧（市场级帧如
/// `!miniTicker@arr` 返回 false）。调用方据此清原地改订的活性看门狗：
/// 市场级帧每秒照推，不能证明新符号流已被服务端接受。
///
/// 符号作用域过滤：组合流帧名形如 `btcusdt@depth20@100ms`，`@` 前缀即
/// 所属符号；`!` 前缀为市场级流。原地改订（UNSUBSCRIBE/SUBSCRIBE）后
/// 旧符号的在途帧仍可能到达，仅与当前 spec 一致的符号级帧才落地——
/// 与 `with_symbol_md` 的活跃订阅守卫互为兜底
fn handle_market_msg_text(state: &SharedState, spec: &SubSpec, text: &str) -> bool {
    let Ok(frame) = serde_json::from_str::<StreamFrame>(text) else {
        // SUBSCRIBE/UNSUBSCRIBE 的服务端应答帧（{"result":null,"id":N}）
        // 不是 StreamFrame 结构，反序列化失败即静默忽略
        return false;
    };
    let stream = frame.stream;
    let data = frame.data.get();
    // 帧的符号作用域：'!' 开头 = 市场级流（不受交易对切换影响）
    let scope = stream.split_once('@').map(|(p, _)| p).unwrap_or("");
    let scoped = !scope.starts_with('!');
    if scoped && scope != spec.symbol.to_lowercase() {
        return false;
    }
    if stream.ends_with("@ticker") {
        if let Ok(t) = serde_json::from_str::<Ticker24>(data) {
            state.with_symbol_md(spec.market, &spec.symbol, |md| md.ticker = Some(t));
        }
    } else if stream.ends_with("!miniTicker@arr") {
        // 全市场迷你 ticker（侧栏实时价格），与活跃订阅无关
        if let Ok(list) = serde_json::from_str::<Vec<MiniTicker>>(data) {
            state.apply_mini_tickers(spec.market, list);
        }
    } else if stream.contains("@depth") {
        // 部分深度流（@depth20@100ms）。用 contains("@depth") 而非精确
        // 档数串，后续调整订阅档数不必改这里
        if let Ok(d) = serde_json::from_str::<Depth>(data) {
            state.with_symbol_md(spec.market, &spec.symbol, |md| md.depth = d);
        }
    } else if stream.ends_with("@aggTrade") {
        if let Ok(t) = serde_json::from_str::<TradeTick>(data) {
            state.apply_trade_to_kline(spec.market, &spec.symbol, &t);
            state.apply_trades(spec.market, &spec.symbol, t);
        }
    } else if stream.contains("@kline_") {
        if let Ok(k) = serde_json::from_str::<WsKline>(data) {
            state.apply_kline(spec.market, &spec.symbol, spec.interval, &k.k);
        }
    } else if stream.contains("@markPrice") {
        if let Ok(m) = serde_json::from_str::<MarkPriceUpdate>(data) {
            state.with_symbol_md(spec.market, &spec.symbol, |md| md.mark = Some(m));
        }
    }
    scoped
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
                    c.set_creds(creds.api_key.clone(), creds.api_secret.clone());
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
        // 用户数据流 URL：现货走 legacy /ws/<listenKey>；期货自 2026-03 起
        // 私有事件迁至 /private/ws/<listenKey>（legacy 入口已停止推送）
        let url = match market {
            Market::UmFutures => format!("{}/private/ws/{}", ep.ws, key),
            Market::Spot => format!("{}/ws/{}", ep.ws, key),
        };
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
                let connected_at = std::time::Instant::now();
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
                // 空闲看门狗基准：每收到一帧（含协议 ping/pong）更新一次。
                // 旧实现在 select! 里同时挂 300s sleep 与 5s sleep——每次迭代
                // 重建 future 后 5s 永远先触发，300s 看门狗是死代码，半开连接
                // 时 listenKey REST 保活仍成功但数据流永久静默
                let mut last_frame = Instant::now();
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
                                last_frame = Instant::now();
                                // 组合流载荷按帧类型统一取 UTF-8 文本（Binary
                                // 分支留作服务端行为变化的兜底）
                                let txt = match m {
                                    tokio_tungstenite::tungstenite::Message::Text(t) => Some(t),
                                    tokio_tungstenite::tungstenite::Message::Binary(b) => {
                                        String::from_utf8(b).ok()
                                    }
                                    _ => None,
                                };
                                if let Some(txt) = txt {
                                    handle_user_event(&state, market, &txt, &engine_tx);
                                    // 用户事件多为对账推送，成交风暴时可能高频；
                                    // 合帧 100ms 与行情流一致，避免满帧率空转
                                    ctx.request_repaint_after(Duration::from_millis(100));
                                }
                            }
                        },
                        _ = keepalive.tick() => {
                            if let Err(e) = client.key_auth(reqwest::Method::PUT, rc, &cfg, lk_path, &keepalive_params).await {
                                tracing::warn!("listenKey 保活失败({market}): {e}，将重建");
                                alive = false;
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {
                            if last_version(&state) != last_seen {
                                break;
                            }
                            // 300s 无任何帧（含协议 ping）判定半开连接，强制重建；
                            // 不能只依赖 REST 保活结果（见上）
                            if last_frame.elapsed() > Duration::from_secs(300) {
                                tracing::warn!("用户数据流空闲超时({market})，强制重连");
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
                // 连接未稳定（<2min）即断开 → 额外退避，避免重连风暴；正常 2s 即可
                if connected_at.elapsed() < Duration::from_secs(120) {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
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
                // 成交后订单/成交历史页需要对账：市价单成交即从挂单消失，
                // 不刷新的话"订单历史/成交历史"子页仍显示旧数据
                engine_tx
                    .send(Command::RefreshHistory {
                        market,
                        symbol: r.symbol.clone(),
                    })
                    .ok();
            }
        }
        (Market::Spot, "outboundAccountPosition") => {
            let Ok(p) = serde_json::from_value::<OutboundAccountPosition>(v) else {
                return;
            };
            // 事件只携带"可能已变化"的资产子集，必须按 asset 合并而非整体替换，
            // 否则一次事件会抹掉其余币种余额（比例填充/资产页随之显示为 0）
            let mut acct = state.acct.lock();
            if let Some(spot) = acct.spot.as_mut() {
                for b in p.balances {
                    match spot.balances.iter_mut().find(|x| x.asset == b.asset) {
                        Some(x) => *x = b,
                        None => spot.balances.push(b),
                    }
                }
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
                // 同现货：成交事件驱动历史子页对账
                engine_tx
                    .send(Command::RefreshHistory {
                        market,
                        symbol: f.o.symbol.clone(),
                    })
                    .ok();
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
                        format!("{}{rp}", crate::i18n::tr("已实现盈亏: ")),
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
        .unwrap_or_else(|| crate::i18n::tr("市价").to_string());
    let word = if status == "FILLED" {
        crate::i18n::tr("已成交")
    } else {
        crate::i18n::tr("部分成交")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn order(id: i64, symbol: &str, status: &str) -> OrderInfo {
        OrderInfo {
            symbol: symbol.into(),
            order_id: id,
            status: status.into(),
            ..Default::default()
        }
    }

    #[test]
    fn order_update_upsert_and_terminal_removal() {
        let st = SharedState::default();
        apply_order_update(&st, Market::Spot, order(1, "BTCUSDT", "NEW"));
        apply_order_update(&st, Market::Spot, order(1, "BTCUSDT", "PARTIALLY_FILLED"));
        apply_order_update(&st, Market::Spot, order(2, "BTCUSDT", "NEW"));
        {
            let o = st.orders.lock();
            let list = &o.open[Market::Spot as usize];
            assert_eq!(list.len(), 2);
            // 更新后的单插到最前（最新优先展示）
            assert_eq!(list[0].order_id, 2);
        }
        // 终态移除（订单号 + 交易对都匹配才移除）
        apply_order_update(&st, Market::Spot, order(1, "BTCUSDT", "FILLED"));
        {
            let o = st.orders.lock();
            let list = &o.open[Market::Spot as usize];
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].order_id, 2);
        }
        // 同 id 不同交易对不受影响
        apply_order_update(&st, Market::Spot, order(2, "ETHUSDT", "CANCELED"));
        {
            let o = st.orders.lock();
            assert_eq!(o.open[Market::Spot as usize].len(), 1);
        }
        // 市场之间互相隔离
        apply_order_update(&st, Market::UmFutures, order(2, "BTCUSDT", "FILLED"));
        {
            let o = st.orders.lock();
            assert_eq!(o.open[Market::UmFutures as usize].len(), 0);
            assert_eq!(o.open[Market::Spot as usize].len(), 1);
        }
    }

    #[test]
    fn stream_scope_filter() {
        let st = SharedState::default();
        st.set_active_sub(Market::Spot, "BTCUSDT");
        let spec = SubSpec {
            market: Market::Spot,
            symbol: "BTCUSDT".into(),
            interval: Interval::M1,
        };
        let ticker_frame = |sym: &str| {
            format!(
                r#"{{"stream":"{sym}@ticker","data":{{"symbol":"{S}","c":"68000","o":"67000","h":"69000","l":"66500","P":"1.5","q":"1000000000","v":"20000"}}}}"#,
                sym = sym.to_lowercase(),
                S = sym
            )
        };

        // 旧符号在途帧（原地改订后服务端仍可能推几帧）：帧名符号与当前订阅
        // 不一致 → 丢弃，返回 false，且不得创建/复活 ETHUSDT 槽位
        assert!(!handle_market_msg_text(&st, &spec, &ticker_frame("ETHUSDT")));
        assert!(!st.md.lock().spot.contains_key("ETHUSDT"));

        // 当前符号帧 → 落地并返回 true（读循环据此清改订活性看门狗）
        assert!(handle_market_msg_text(&st, &spec, &ticker_frame("BTCUSDT")));
        {
            let md = st.md.lock();
            let slot = md.spot.get("BTCUSDT").expect("活跃符号槽位已创建");
            assert_eq!(
                slot.ticker.as_ref().unwrap().last_price,
                Decimal::from(68000)
            );
        }

        // 市场级流：照常分发（侧栏 tickers 更新），但返回 false——
        // 它每秒都推，不能证明新符号符号级流已被服务端接受
        let mini = r#"{"stream":"!miniTicker@arr","data":[{"s":"BTCUSDT","c":"68100","o":"67000","P":"1.6"},{"s":"ETHUSDT","c":"3000","o":"2900","P":"3.4"}]}"#;
        assert!(!handle_market_msg_text(&st, &spec, mini));
        assert_eq!(
            st.ticker_of(Market::Spot, "BTCUSDT").unwrap().close,
            Decimal::from(68100)
        );

        // 深度流的符号作用域同样过滤（帧名含多段 @depth20@100ms）
        let depth_stale = r#"{"stream":"ethusdt@depth20@100ms","data":{"lastUpdateId":1,"bids":[["3000","1"]],"asks":[["3001","2"]]}}"#;
        assert!(!handle_market_msg_text(&st, &spec, depth_stale));
        // 非组合流的控制帧应答（result(null)）也不是 StreamFrame → 静默 false
        assert!(!handle_market_msg_text(&st, &spec, r#"{"result":null,"id":7}"#));
    }
}
