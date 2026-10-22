//! 后台引擎：命令处理（REST）、行情 WebSocket、用户数据流 WebSocket
//!
//! UI 线程通过 `SharedState::send_cmd` 发送命令；后台任务把结果写入 `SharedState` 快照并请求重绘。

use eframe::egui;
pub mod state;

use crate::binance::client::{
    build_algo_order_params, build_order_params, BinanceError, BinanceRest, OrderKind,
    OrderRequest, RestCtx,
};
use crate::binance::model::*;
use crate::binance::symbols::{parse_exchange_info, quantize_price, quantize_qty, RuleBook};
use crate::binance::{endpoints, Interval, Market, Side};
use crate::config::AppConfig;
use futures_util::{future::join_all, SinkExt, StreamExt};
use rust_decimal::Decimal;
use state::{ConnState, SharedState, ToastKind};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
    /// 网络恢复巡检专用：校时 + 仅补拉规则缺失市场的 exchangeInfo。
    /// 不重发订阅、不刷账户历史——巡检每 5-10s 一次，若用 ReloadAll
    /// 其"重发订阅"会反复触发行情 WS 重连与历史重拉风暴
    ReloadRules,
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
    /// 资金流水：合约拉 /fapi/v1/income，现货拉充值+提现历史
    RefreshFlow {
        market: Market,
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
    /// 撤销合约条件单（Algo Order 体系：仓位止盈止损/追踪委托等，
    /// DELETE /fapi/v1/algoOrder 按 algoId 撤销）
    CancelAlgoOrder {
        algo_id: i64,
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

/// 启动后台运行时与全部任务。
/// 命令通道的唯一入口是 `SharedState::send_cmd`（返回入队是否成功）；
/// UI 不再持有独立 sender，避免失败提示/回滚策略按入口分裂
pub fn spawn(ctx: egui::Context, state: Arc<SharedState>) {
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
                    // 网络恢复巡检：ReloadAll 只在启动发一次，断网启动时
                    // exchangeInfo/校时失败后侧栏将永久停在"交易规则加载中"，
                    // REST 指示灯也一直红（rest_ok 仅靠 30 分钟一次的 SyncTime
                    // 才可能翻回）。行情 WS 有自愈重连，REST 侧补上恢复路径。
                    // 巡检只发轻量的 ReloadRules（校时+缺规则市场补拉）：
                    // 发 ReloadAll 会"重发订阅"，每轮（5-10s）触发行情 WS
                    // 重连+历史重拉的风暴；从不健康转健康时才补发一次完整
                    // ReloadAll 恢复账户/挂单/订阅
                    {
                        let h_tx = tx.clone();
                        let h_state = state.clone();
                        tokio::spawn(async move {
                            fn healthy(st: &SharedState) -> bool {
                                !st.rules.read().iter().any(|b| b.symbols.is_empty())
                                    && st.rest_ok.load(Ordering::Relaxed)
                            }
                            // 启动宽限：避开启动期 ReloadAll 的在途重复。不睡固定
                            // 时长而是盯首个就绪信号——健康/部分健康时启动 ReloadAll
                            // 已见效，无重复投递风险即退出宽限；全程断网（fail-fast）
                            // 时没有任何信号，最长等 60s（≥ 启动 ReloadAll 在慢网下
                            // 的最坏耗时 4×http_timeout），届时必然已不在途
                            let grace = tokio::time::Instant::now() + Duration::from_secs(60);
                            while tokio::time::Instant::now() < grace && !healthy(&h_state) {
                                tokio::time::sleep(Duration::from_secs(2)).await;
                            }
                            let mut backoff = Duration::from_secs(5);
                            // 是否曾观测到不健康：转回健康时据此补发一次完整 ReloadAll；
                            // 启动即健康的正常路径不多发任何命令
                            let mut was_unhealthy = false;
                            let mut full_sent = false;
                            loop {
                                if healthy(&h_state) {
                                    if was_unhealthy && !full_sent {
                                        tracing::info!("恢复巡检：网络已恢复，补发 ReloadAll");
                                        let _ = h_tx.send(Command::ReloadAll);
                                        full_sent = true;
                                    }
                                    backoff = Duration::from_secs(5);
                                    tokio::time::sleep(Duration::from_secs(15)).await;
                                    continue;
                                }
                                was_unhealthy = true;
                                // 每轮不健康都以"恢复后一次完整 reload"收尾：
                                // 若在断网期间改过配置，那次 ReloadAll 也会失败并
                                // 置 rest_ok=false，恢复后必须重补一次才能刷新账户
                                full_sent = false;
                                tracing::info!(?backoff, "恢复巡检：重发 ReloadRules");
                                let _ = h_tx.send(Command::ReloadRules);
                                // 等本轮执行完或判超时再投下一轮：命令串行执行，
                                // 慢请求（连接超时）下持续投递会让队列无界堆积。
                                // 等待上限按 HTTP 超时推导（一条 ReloadRules 最坏
                                // 2 校时 + 2 exchangeInfo ≈ 4×超时），保证发送速率
                                // 不超过执行速率
                                let to = h_state.cfg_snapshot().http_timeout_secs.max(3);
                                let deadline =
                                    tokio::time::Instant::now() + Duration::from_secs(to * 4 + 15);
                                while tokio::time::Instant::now() < deadline && !healthy(&h_state) {
                                    tokio::time::sleep(Duration::from_secs(2)).await;
                                }
                                if healthy(&h_state) {
                                    backoff = Duration::from_secs(5);
                                } else {
                                    tokio::time::sleep(backoff).await;
                                    backoff = (backoff * 2).min(Duration::from_secs(60));
                                }
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
}

fn repaint(ctx: &egui::Context) {
    ctx.request_repaint();
}

fn toast_err(state: &SharedState, e: &BinanceError) {
    tracing::warn!("REST 错误: {e}");
    state.push_toast(ToastKind::Err, e.friendly());
}

// ---------------- 资金流水归一化助手（纯函数，便于测试） ----------------
// 注意：这里统一返回「中文原键」，由 UI 展示时经 i18n::tr 现场翻译，
// 保证运行中切换语言时已存数据不会固化成旧语言文案

/// 合约 income 类型 → 中文类型键
fn flow_kind(t: &str) -> &'static str {
    match t {
        "TRANSFER" => "划转",
        "FUNDING_FEE" => "资金费",
        "REALIZED_PNL" => "已实现盈亏",
        "COMMISSION" => "手续费",
        "WELCOME_BONUS" => "体验金",
        _ => "其他",
    }
}

/// 划转方向（income 条目 transferType：IN/OUT）；未知返回空
fn transfer_dir(s: &str) -> &'static str {
    match s {
        "IN" => "转入",
        "OUT" => "转出",
        _ => "",
    }
}

/// 充值状态：0=处理中 1=成功 6=已入账（锁定）
fn deposit_status(s: i64) -> &'static str {
    match s {
        0 => "处理中",
        1 => "成功",
        6 => "已入账",
        _ => "其他",
    }
}

/// 提现状态：0=审核中 1=已取消 2=待审核 3=已拒绝 4=处理中 5=失败 6=完成
fn withdraw_status(s: i64) -> &'static str {
    match s {
        0 => "审核中",
        1 => "已取消",
        2 => "待审核",
        3 => "已拒绝",
        4 => "处理中",
        5 => "失败",
        6 => "完成",
        _ => "其他",
    }
}

/// 提现 applyTime（"yyyy-MM-dd HH:mm:ss" UTC 文本）→ 毫秒；解析失败归 0
fn apply_time_ms(s: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|t| t.and_utc().timestamp_millis())
        .unwrap_or(0)
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
    /// 上次订单历史刷新时间（按 市场+交易对 节流）：历史 REST 每份数秒且
    /// 串行执行，手动按钮被连续点击时积压命令会逐条以每秒一条的速率
    /// 执行（实测日志），3s 内同键重复直接丢弃，切换交易对不受影响
    history_debounce: HashMap<(Market, String), Instant>,
    /// 上次资金流水刷新时间（RefreshFlow 节流，按市场）
    flow_debounce: HashMap<Market, Instant>,
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
        history_debounce: HashMap::new(),
        flow_debounce: HashMap::new(),
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
        // 历史按 (市场, 交易对) 合并、流水按市场合并：串行 REST 命令积压时
        // 同键重复只保留最后一条（与 RefreshAccount 同理）
        let mut hist_seen: Vec<(Market, String)> = Vec::new();
        let mut flow_seen = [false; 2];
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
            Command::RefreshHistory { market, symbol } => {
                let k = (*market, symbol.clone());
                if hist_seen.contains(&k) {
                    false
                } else {
                    hist_seen.push(k);
                    true
                }
            }
            Command::RefreshFlow { market } => {
                let i = *market as usize;
                if flow_seen[i] {
                    false
                } else {
                    flow_seen[i] = true;
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
                Command::CancelAlgoOrder { algo_id } => {
                    if let Some(client) = c.client.clone() {
                        let (st, cx, etx) = (state.clone(), ctx.clone(), engine_tx.clone());
                        tokio::spawn(async move {
                            cancel_algo_order(&client, &st, algo_id, &etx).await;
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
                            cancel_all_orders(&client, &st, market, symbol.as_deref(), &etx).await;
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

/// 拉取某市场的 exchangeInfo 并写入规则簿（成功时递增规则版本号，
/// UI 的符号列表缓存按版本失效）。ReloadAll 与恢复巡检共用
async fn fetch_rules(
    client: &BinanceRest,
    state: &Arc<SharedState>,
    cfg: &AppConfig,
    market: Market,
) -> Result<(), BinanceError> {
    let rc = RestCtx {
        market,
        testnet: cfg.testnet,
        recv_window_ms: cfg.recv_window_ms,
    };
    let path = format!("{}/exchangeInfo", market.api_prefix());
    let v = client.get_public(rc, cfg, &path, &[]).await?;
    let book = RuleBook::from_rules(parse_exchange_info(&v));
    tracing::info!("{}: {} 个交易对", market.label(), book.symbols.len());
    state.rules.write()[market as usize] = book;
    state.rules_version.fetch_add(1, Ordering::Relaxed);
    Ok(())
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
            Command::CancelOrder { .. }
            | Command::CancelAll { .. }
            | Command::CancelAlgoOrder { .. } => {
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
                match fetch_rules(client, &state, &cfg, m).await {
                    Ok(()) => {}
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
        Command::ReloadRules => {
            // 巡检轻量恢复：校时（刷 rest_ok——失败也翻假；巡检每 5s+重试，
            // 不像 30min 的 SyncTime 那样需要防瞬时抖动误报）+ 仅补拉规则
            // 缺失的市场（已加载市场不重复拉 1300+ 个现货交易对）。
            // 失败只记日志：启动期 ReloadAll 已向用户提示过一次，巡检重试
            // 的 warn 噪声不必要再弹 toast
            let cfg = state.cfg_snapshot();
            let mut all_ok = true;
            for m in Market::all() {
                let rc = RestCtx {
                    market: m,
                    testnet: cfg.testnet,
                    recv_window_ms: cfg.recv_window_ms,
                };
                if let Err(e) = client.sync_time(rc, &cfg).await {
                    tracing::warn!("巡检校时失败({m}): {e}");
                    all_ok = false;
                }
            }
            state.rest_ok.store(all_ok, Ordering::Relaxed);
            for m in Market::all() {
                if !state.rules.read()[m as usize].symbols.is_empty() {
                    continue;
                }
                if let Err(e) = fetch_rules(client, &state, &cfg, m).await {
                    tracing::warn!("巡检规则加载失败({m}): {e}");
                }
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
                    match serde_json::from_value::<Vec<PositionRisk>>(v) {
                        Ok(mut list) => {
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
                        Err(e) => tracing::warn!("解析 positionRisk 失败: {e}"),
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
            // 合约条件单（仓位止盈止损/追踪委托等）已迁移至 Algo Order
            // 体系，openOrders 不再包含——单独拉取存快照，供持仓止盈止损
            // 列与挂单表展示
            if market == Market::UmFutures {
                let algo_path = format!("{}/openAlgoOrders", market.api_prefix());
                match client
                    .signed(reqwest::Method::GET, rc, &cfg, &algo_path, vec![])
                    .await
                {
                    Ok(v) => match serde_json::from_value::<Vec<AlgoOrderInfo>>(v) {
                        Ok(list) => {
                            state.orders.lock().algo = list;
                            repaint(ctx);
                        }
                        Err(e) => tracing::warn!("解析条件单失败: {e}"),
                    },
                    Err(e) => {
                        // 获取失败只记日志不打断：条件单列表为空会让止盈止损
                        // 列显示"—"，但普通挂单/持仓不受影响
                        if !matches!(e, BinanceError::NoCreds) {
                            tracing::warn!("条件单获取失败: {e}");
                        }
                    }
                }
            }
        }
        Command::RefreshHistory { market, symbol } => {
            // 节流（3s 键 = 市场+交易对）：历史 REST 串行且每份数秒，手动按钮
            // 被连续点击时积压命令会逐条以每秒一条的速率执行（实测日志）。
            // 同交易对 3s 内重复直接丢弃；切换交易对换键不受影响。
            // 登记放在 has_creds 之后：无凭证的尝试不应占用节流窗口，
            // 否则"无凭证点一下→填凭证 3 秒内再点"的第二次会被吞
            let hkey = (market, symbol.clone());
            if let Some(t) = c.history_debounce.get(&hkey) {
                if t.elapsed() < Duration::from_secs(3) {
                    return;
                }
            }
            if !state.has_creds() {
                return;
            }
            c.history_debounce.insert(hkey, Instant::now());
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let orders_path = format!("{}/allOrders", market.api_prefix());
            let trades_path = match market {
                Market::Spot => "/api/v3/myTrades".to_string(),
                Market::UmFutures => format!("{}/userTrades", market.api_prefix()),
            };
            // 现货维持单次全量（limit=100，现状可用）；合约历史端点不传
            // startTime 时默认窗口很窄（实测 8 天前的订单已查不到），改按
            // 7 天分段回填 30 天并发拉取（见 history_segments）
            let po = match market {
                Market::Spot => client
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
                    .await
                    .map(|v| vec![v]),
                Market::UmFutures => {
                    fetch_history_segments(client, rc, &cfg, &orders_path, &symbol, now_ms(), true)
                        .await
                }
            };
            let pt = match market {
                Market::Spot => client
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
                    .await
                    .map(|v| vec![v]),
                Market::UmFutures => {
                    fetch_history_segments(client, rc, &cfg, &trades_path, &symbol, now_ms(), true)
                        .await
                }
            };
            // 条件单历史（/fapi/v1/allAlgoOrders，与 openAlgoOrders 同族）：
            // 参数集无官方文档可核对（文档站反爬无法抓取），先按 allOrders
            // 同款参数请求；交易所若拒绝参数（-1102/-1104）则退化为仅
            // symbol+limit 重试一遍，自愈式兼容
            let pa = if market == Market::UmFutures {
                let algo_path = format!("{}/allAlgoOrders", market.api_prefix());
                let mut r =
                    fetch_history_segments(client, rc, &cfg, &algo_path, &symbol, now_ms(), true)
                        .await;
                if matches!(
                    &r,
                    Err(BinanceError::Api {
                        code: -1102 | -1104,
                        ..
                    })
                ) {
                    r = fetch_history_segments(
                        client,
                        rc,
                        &cfg,
                        &algo_path,
                        &symbol,
                        now_ms(),
                        false,
                    )
                    .await;
                }
                Some(r)
            } else {
                None
            };
            {
                let (po_ok, pt_ok) = (po.is_ok(), pt.is_ok());
                let pa_ok = pa.as_ref().map(|r| r.is_ok()).unwrap_or(false);
                // 失败必须可见：静默吞掉时 UI 恒为空态，用户无法区分
                // "没有历史"与"请求失败"（期货历史查不到即此形态）
                if let Err(e) = &po {
                    if !matches!(e, BinanceError::NoCreds) {
                        tracing::warn!("订单历史获取失败({market}): {e}");
                        state.push_toast(
                            ToastKind::Err,
                            format!(
                                "{}{}: {}",
                                crate::i18n::tr("订单历史获取失败"),
                                market.label(),
                                e.friendly()
                            ),
                        );
                    }
                }
                if let Err(e) = &pt {
                    if !matches!(e, BinanceError::NoCreds) {
                        tracing::warn!("成交历史获取失败({market}): {e}");
                        state.push_toast(
                            ToastKind::Err,
                            format!(
                                "{}{}: {}",
                                crate::i18n::tr("成交历史获取失败"),
                                market.label(),
                                e.friendly()
                            ),
                        );
                    }
                }
                let mut o = state.orders.lock();
                if let Ok(vals) = po {
                    match flatten_history::<OrderInfo>(vals) {
                        Ok(mut list) => {
                            // 合并条件单历史：自 2025-12 条件单迁至 Algo 体系
                            // 后不会出现在 allOrders 里，不合并则持仓止盈止损
                            // 单在订单历史中永远查不到
                            if let Some(Ok(avals)) = pa {
                                match flatten_history::<AlgoOrderInfo>(avals) {
                                    Ok(algos) => list.extend(algos.iter().map(algo_to_order_info)),
                                    Err(e) => tracing::warn!("解析条件单历史失败: {e}"),
                                }
                            }
                            list.sort_by_key(|x| std::cmp::Reverse(x.time));
                            tracing::info!("订单历史 {market} {symbol}: {} 条", list.len());
                            // 整体替换 Arc：UI 侧持有旧 Arc 的帧不受影响，
                            // 新帧命中新内容——无版本号契约需要双方维护
                            o.history = std::sync::Arc::new(list);
                        }
                        Err(e) => tracing::warn!("解析订单历史失败: {e}"),
                    }
                }
                if let Ok(vals) = pt {
                    match flatten_history::<MyTrade>(vals) {
                        Ok(mut list) => {
                            list.sort_by_key(|x| std::cmp::Reverse(x.time));
                            tracing::info!("成交历史 {market} {symbol}: {} 条", list.len());
                            o.trades = std::sync::Arc::new(list);
                        }
                        Err(e) => tracing::warn!("解析成交历史失败: {e}"),
                    }
                }
                o.history_symbol = if po_ok || pt_ok || pa_ok {
                    // 至少一个请求成功才更新归属标记：全部失败时保留旧数据 +
                    // 旧标记，避免表头显示新交易对而内容仍是旧交易对
                    Some((market, symbol))
                } else {
                    o.history_symbol.take()
                };
            }
            repaint(ctx);
        }
        Command::RefreshFlow { market } => {
            // 节流（3s 键 = 市场）：与 RefreshHistory 同因——手动按钮可被
            // 连续点击，串行 REST 期间积压命令逐条执行。按市场防抖，
            // 切换市场换键不受影响。登记放在 has_creds 之后
            // （RefreshHistory 同理：无凭证尝试不占节流窗口）
            if let Some(t) = c.flow_debounce.get(&market) {
                if t.elapsed() < Duration::from_secs(3) {
                    return;
                }
            }
            if !state.has_creds() {
                return;
            }
            c.flow_debounce.insert(market, Instant::now());
            let cfg = state.cfg_snapshot();
            let rc = RestCtx {
                market,
                testnet: cfg.testnet,
                recv_window_ms: cfg.recv_window_ms,
            };
            let limit = vec![("limit".into(), "100".into())];
            let mut flows: Vec<state::FundFlow> = Vec::new();
            // 单侧失败仍展示另一侧；仅全部失败才 toast（现货两源并发语义）
            let mut first_err: Option<BinanceError> = None;
            match market {
                Market::UmFutures => {
                    // income 不带 symbol：返回全部交易对 + 划转/资金费等全局条目
                    match client
                        .signed(reqwest::Method::GET, rc, &cfg, "/fapi/v1/income", limit)
                        .await
                    {
                        Ok(v) => match serde_json::from_value::<Vec<FutIncome>>(v) {
                            Ok(list) => {
                                flows.extend(list.into_iter().map(|x| {
                                    let note = match x.income_type.as_str() {
                                        "TRANSFER" => transfer_dir(&x.transfer_type).to_string(),
                                        "FUNDING_FEE" | "REALIZED_PNL" | "COMMISSION" => {
                                            String::new()
                                        }
                                        // 未知类型原文入备注，避免信息丢失
                                        other => other.to_string(),
                                    };
                                    state::FundFlow {
                                        time: x.time,
                                        kind: flow_kind(&x.income_type),
                                        symbol: x.symbol,
                                        asset: x.asset,
                                        amount: x.income,
                                        note,
                                    }
                                }));
                            }
                            Err(e) => tracing::warn!("解析资金流水失败: {e}"),
                        },
                        Err(e) => first_err = Some(e),
                    }
                }
                Market::Spot => {
                    // 现货无统一流水端点：充值 + 提现两源合并
                    match client
                        .signed(
                            reqwest::Method::GET,
                            rc,
                            &cfg,
                            "/sapi/v1/capital/deposit/hisrec",
                            limit.clone(),
                        )
                        .await
                    {
                        Ok(v) => match serde_json::from_value::<Vec<SpotDeposit>>(v) {
                            Ok(list) => {
                                flows.extend(list.into_iter().map(|d| state::FundFlow {
                                    time: d.insert_time,
                                    kind: "充值",
                                    symbol: String::new(),
                                    asset: d.coin,
                                    amount: d.amount,
                                    note: format!(
                                        "{}{} · {}",
                                        crate::i18n::tr("网络 "),
                                        if d.network.is_empty() {
                                            "—"
                                        } else {
                                            &d.network
                                        },
                                        deposit_status(d.status)
                                    ),
                                }));
                            }
                            Err(e) => tracing::warn!("解析充值历史失败: {e}"),
                        },
                        Err(e) => first_err = Some(e),
                    }
                    match client
                        .signed(
                            reqwest::Method::GET,
                            rc,
                            &cfg,
                            "/sapi/v1/capital/withdraw/history",
                            limit,
                        )
                        .await
                    {
                        Ok(v) => match serde_json::from_value::<Vec<SpotWithdraw>>(v) {
                            Ok(list) => {
                                flows.extend(list.into_iter().map(|w| {
                                    // 流出取负号；手续费为 0 时不出现在备注
                                    let mut note = format!(
                                        "{}{} · {}",
                                        crate::i18n::tr("网络 "),
                                        if w.network.is_empty() {
                                            "—"
                                        } else {
                                            &w.network
                                        },
                                        withdraw_status(w.status)
                                    );
                                    if !w.transaction_fee.is_zero() {
                                        note.push_str(&format!(
                                            " · {}{}",
                                            crate::i18n::tr("手续费 "),
                                            w.transaction_fee.normalize()
                                        ));
                                    }
                                    state::FundFlow {
                                        time: apply_time_ms(&w.apply_time),
                                        kind: "提现",
                                        symbol: String::new(),
                                        asset: w.coin,
                                        amount: -w.amount,
                                        note,
                                    }
                                }));
                            }
                            Err(e) => tracing::warn!("解析提现历史失败: {e}"),
                        },
                        Err(e) => first_err = Some(e),
                    }
                }
            }
            if let Some(e) = &first_err {
                if flows.is_empty() {
                    tracing::warn!("资金流水获取失败({market}): {e}");
                    state.push_toast(
                        ToastKind::Err,
                        format!(
                            "{}{}: {}",
                            crate::i18n::tr("资金流水获取失败"),
                            market.label(),
                            e.friendly()
                        ),
                    );
                } else {
                    tracing::warn!("资金流水部分获取失败({market}): {e}");
                }
            }
            flows.sort_by_key(|x| std::cmp::Reverse(x.time));
            tracing::info!("资金流水 {market}: {} 条", flows.len());
            {
                let mut o = state.orders.lock();
                o.flow = std::sync::Arc::new(flows);
                o.flow_market = Some(market);
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
                .signed(
                    reqwest::Method::POST,
                    rc,
                    &cfg,
                    "/fapi/v1/marginType",
                    params,
                )
                .await;
            match res {
                Ok(_) => {
                    state
                        .acct
                        .lock()
                        .fut_margin_type
                        .insert(symbol.clone(), target.into());
                    state.push_toast(
                        ToastKind::Ok,
                        format!("{symbol} {}{target}", crate::i18n::tr("保证金模式已切为 ")),
                    );
                    engine_tx.send(Command::RefreshAccount).ok();
                }
                // -4046 = 无需修改（已是目标模式），按成功提示
                Err(BinanceError::Api { code: -4046, .. }) => {
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
            if let Err(e) = cfg.save(crate::config::data_dir()) {
                tracing::error!("配置保存失败: {e}");
                // 仅在失败时打扰用户：保存成功是后台常态（切自选也会触发），
                // 成功 toast 会刷屏；失败必须明确告知，否则用户直到重启后
                // 才发现自选/预警/端点配置从未写入
                state.push_toast(
                    ToastKind::Err,
                    format!("{}{e}", crate::i18n::tr("配置保存失败: ")),
                );
            }
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
        | Command::CancelAlgoOrder { .. }
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
    // WS-API 通道（可选，仅合约）：任何失败自动回退 REST，资金路径不赌单通道；
    // 条件单已迁至 Algo 体系且 WS 通道不覆盖，直接走 REST algoOrder
    if market == Market::UmFutures && cfg.ws_order && !req.kind.is_conditional() {
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
                    format!("{}{e}", crate::i18n::tr("WS 下单失败，已回退 REST: ")),
                );
            }
        }
    }
    // 条件单已迁至 Algo Order 体系（/fapi/v1/order 报 -4120）：
    // 参数构建与端点都换 algoOrder 版
    let (path, params) = if market == Market::UmFutures && req.kind.is_conditional() {
        (
            format!("{}/algoOrder", market.api_prefix()),
            build_algo_order_params(req),
        )
    } else {
        (format!("{}/order", market.api_prefix()), params)
    };
    let res = client
        .signed(reqwest::Method::POST, rc, &cfg, &path, params)
        .await;
    match res {
        Ok(v) => {
            // 条件单响应的主键是 algoId（普通单为 orderId）
            let id = v
                .get("orderId")
                .or_else(|| v.get("algoId"))
                .and_then(|x| x.as_i64())
                .unwrap_or(0);
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

/// 撤销合约条件单（Algo Order）：DELETE /fapi/v1/algoOrder 按 algoId
/// 撤销（busy 置位/释放由本函数负责；命令循环可并发派发）
async fn cancel_algo_order(
    client: &BinanceRest,
    state: &Arc<SharedState>,
    algo_id: i64,
    engine_tx: &mpsc::UnboundedSender<Command>,
) {
    if !state.has_creds() {
        return;
    }
    let cfg = state.cfg_snapshot();
    let rc = RestCtx {
        market: Market::UmFutures,
        testnet: cfg.testnet,
        recv_window_ms: cfg.recv_window_ms,
    };
    let path = format!("{}/algoOrder", Market::UmFutures.api_prefix());
    let params = vec![("algoId".into(), algo_id.to_string())];
    state.set_busy("cancel", true);
    let res = client
        .signed(reqwest::Method::DELETE, rc, &cfg, &path, params)
        .await;
    state.set_busy("cancel", false);
    match res {
        Ok(_) => {
            state.push_toast(
                ToastKind::Ok,
                format!("{}{algo_id}", crate::i18n::tr("已撤销条件单 #")),
            );
            engine_tx
                .send(Command::RefreshOpenOrders {
                    market: Market::UmFutures,
                })
                .ok();
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
        req.stop_limit_price = quantize_with_note(req.stop_limit_price, r.tick, true, &mut note)?;
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

/// 历史查询覆盖的天数（期货）：allOrders/userTrades 不传 startTime 时
/// 默认窗口很窄（实测 8 天前的订单已查不到），故回填一段历史
const HISTORY_DAYS_BACK: u32 = 30;

/// 本地毫秒时间戳（历史窗口边界用，±几百 ms 偏差对 30 天窗口无意义）
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 把 [now-days_back, now] 切成若干 ≤7 天的区间（新→旧）。分段同时规避
/// 两件事：默认窗口太窄查不到旧数据，且各端点单次时间跨度上限不一致
fn history_segments(now_ms: i64, days_back: u32) -> Vec<(i64, i64)> {
    const DAY_MS: i64 = 86_400_000;
    const CHUNK_DAYS: i64 = 7;
    let floor = now_ms - days_back as i64 * DAY_MS;
    let mut segs = Vec::new();
    let mut end = now_ms;
    while end > floor {
        let start = (end - CHUNK_DAYS * DAY_MS).max(floor);
        segs.push((start, end));
        end = start;
    }
    segs
}

/// 拉取用的分段列表：带时间参数时按 30 天 / 7 天分段；回退模式
/// （with_time=false）只返回一段占位区间——调用方不使用边界，
/// 拼接结果也不会因多发相同请求而重复
fn fetch_segments(now_ms: i64, with_time: bool) -> Vec<(i64, i64)> {
    if with_time {
        history_segments(now_ms, HISTORY_DAYS_BACK)
    } else {
        vec![(0, 0)]
    }
}

/// 分段并发拉取历史端点：各段独立成败，部分失败时返回已成功的响应、
/// 全部失败才返回错误（首段错误优先）。段间无重叠故结果不去重。
/// with_time=false（端点拒绝时间参数时的回退）只发一次：此时段边界
/// 无意义，仍按分段发出会是 N 个参数完全相同的请求，拼接后每条记录
/// 重复 N 次
async fn fetch_history_segments(
    client: &BinanceRest,
    rc: RestCtx,
    cfg: &AppConfig,
    path: &str,
    symbol: &str,
    now_ms: i64,
    with_time: bool,
) -> Result<Vec<serde_json::Value>, BinanceError> {
    let segs = fetch_segments(now_ms, with_time);
    let futs: Vec<_> = segs
        .into_iter()
        .map(|(start, end)| {
            let mut params = vec![
                ("symbol".into(), symbol.to_string()),
                ("limit".into(), "1000".into()),
            ];
            if with_time {
                params.push(("startTime".into(), start.to_string()));
                params.push(("endTime".into(), end.to_string()));
            }
            let cfg = cfg.clone();
            async move {
                client
                    .signed(reqwest::Method::GET, rc, &cfg, path, params)
                    .await
            }
        })
        .collect();
    let results = join_all(futs).await;
    let mut ok = Vec::new();
    let mut first_err = None;
    for r in results {
        match r {
            Ok(v) => ok.push(v),
            Err(e) => {
                tracing::warn!("历史分段请求失败({path}): {e}");
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    if ok.is_empty() {
        Err(first_err.unwrap_or_else(|| {
            BinanceError::Decode(crate::i18n::tr("历史分段全部失败").to_string())
        }))
    } else {
        Ok(ok)
    }
}

/// 各段响应合并为列表：段为 JSON 数组直接拼接；个别端点把数组包在
/// {orders:[...]} 里（SAPI 风格），解包后拼接。非数组段记录原文后跳过
fn flatten_history<T: serde::de::DeserializeOwned>(
    vals: Vec<serde_json::Value>,
) -> Result<Vec<T>, serde_json::Error> {
    let mut out = Vec::new();
    for v in vals {
        let arr = if v.is_array() {
            v
        } else {
            match v.get("orders").filter(|x| x.is_array()) {
                Some(a) => a.clone(),
                None => {
                    let raw = serde_json::to_string(&v).unwrap_or_default();
                    tracing::warn!("历史响应非数组: {}", &raw[..raw.len().min(400)]);
                    continue;
                }
            }
        };
        out.extend(serde_json::from_value::<Vec<T>>(arr)?);
    }
    Ok(out)
}

/// 条件单历史行 → 订单历史表行：algoId 入 orderId 列，触发价同时入
/// 价格/触发价列；未触发条件单 actualQty 为 0，algoStatus 为空按 NEW 兜底
fn algo_to_order_info(a: &AlgoOrderInfo) -> OrderInfo {
    OrderInfo {
        symbol: a.symbol.clone(),
        order_id: a.algo_id,
        client_order_id: a.client_algo_id.clone(),
        price: a.trigger_price,
        orig_qty: a.quantity,
        executed_qty: a.actual_qty,
        status: if a.algo_status.is_empty() {
            "NEW".to_string()
        } else {
            a.algo_status.clone()
        },
        side: a.side.clone(),
        order_type: a.order_type.clone(),
        stop_price: a.trigger_price,
        time: a.create_time,
        update_time: a.update_time,
        position_side: a.position_side.clone(),
        reduce_only: a.reduce_only,
        is_conditional: true,
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
        Market::Spot => (format!("{}/openOrders", market.api_prefix()), vec![]),
        Market::UmFutures => (format!("{}/allOpenOrders", market.api_prefix()), vec![]),
    };
    if let Some(sym) = symbol {
        params.push(("symbol".into(), sym.to_string()));
    }
    state.set_busy("cancel", true);
    // 现货 DELETE /openOrders 与合约 DELETE /allOpenOrders 均实测强制要求
    // symbol（不带报 -1102，2026 API 收紧且文档滞后）——"本市场全部"退化为
    // 按当前挂单的去重交易对逐个撤；全空视为成功
    let res = if symbol.is_none() {
        let syms: Vec<String> = {
            let mut v: Vec<String> = state.orders.lock().open[market as usize]
                .iter()
                .map(|o| o.symbol.clone())
                .collect();
            v.sort();
            v.dedup();
            v
        };
        let mut last_err = None;
        for s in syms {
            let ps = vec![("symbol".into(), s.clone())];
            if let Err(e) = client
                .signed(reqwest::Method::DELETE, rc, &cfg, &path, ps)
                .await
            {
                tracing::warn!("撤销挂单失败({s}): {e}");
                last_err = Some(e);
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(serde_json::Value::Null),
        }
    } else {
        client
            .signed(reqwest::Method::DELETE, rc, &cfg, &path, params)
            .await
    };
    // 条件单（仓位止盈止损/追踪委托）在 Algo 体系，allOpenOrders 撤不到：
    // 指定交易对用批量端点 algoOpenOrders；全市场无批量端点，逐交易对撤
    if market == Market::UmFutures {
        let syms: Vec<String> = match symbol {
            Some(s) => vec![s.to_string()],
            None => {
                let mut v: Vec<String> = state
                    .orders
                    .lock()
                    .algo
                    .iter()
                    .map(|a| a.symbol.clone())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
        };
        let algo_path = format!("{}/algoOpenOrders", market.api_prefix());
        for s in syms {
            let ps = vec![("symbol".into(), s.clone())];
            if let Err(e) = client
                .signed(reqwest::Method::DELETE, rc, &cfg, &algo_path, ps)
                .await
            {
                tracing::warn!("撤销条件单失败({s}): {e}");
            }
        }
    }
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
        if matches!(
            (self, fut),
            (MarketWsSlot::A, false) | (MarketWsSlot::B, true)
        ) {
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

        let st = pair
            .lock()
            .set(slot.idx(), spec.market, ConnState::Connecting);
        state.set_market_ws(spec.market, st.clone());
        state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
        repaint(&ctx);

        match crate::binance::ws::connect(&url, &cfg.proxy).await {
            Err(e) => {
                tracing::warn!("行情WS连接失败({}): {e}", spec.market);
                let msg = short_err(&e);
                let st = pair
                    .lock()
                    .set(slot.idx(), spec.market, ConnState::Failed(msg.clone()));
                state.set_market_ws(spec.market, st.clone());
                state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
                repaint(&ctx);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            Ok(ws) => {
                tracing::info!("行情WS已连接: {}", spec.market);
                let connected_at = std::time::Instant::now();
                let st = pair
                    .lock()
                    .set(slot.idx(), spec.market, ConnState::Connected);
                state.set_market_ws(spec.market, st.clone());
                state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
                repaint(&ctx);

                let (mut write, mut read) = futures_util::StreamExt::split(ws);
                // 服务端当下已订阅的流（含随连接建立的 !miniTicker@arr）。
                // 原地改订按差集计算增量：市场级流（'!' 前缀）永远不退订
                let mut server_streams = slot
                    .streams_and_url(&ep.ws, &lower, spec.interval.as_str(), fut)
                    .0;
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
                            let (drop_s, add_s) = resub_diff(&server_streams, &new_scoped);
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
                    let st = pair
                        .lock()
                        .set(slot.idx(), spec.market, ConnState::Failed(msg));
                    state.set_market_ws(spec.market, st.clone());
                    state.with_symbol_md(spec.market, &spec.symbol, |md| md.ws = st);
                } else if !intentional {
                    let st = pair
                        .lock()
                        .set(slot.idx(), spec.market, ConnState::Disconnected);
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
            let st = pair
                .lock()
                .set(slot.idx(), spec.market, ConnState::Disconnected);
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

/// 原地改订的流差集：服务端当前已订阅 vs 新规格需要的符号级流。
///
/// `drop` = 服务端有但新规格不再需要的（市场级 '!' 流永不退订——它
/// 随初始连接建立，与单个交易对无关）；`add` = 新规格需要但服务端
/// 尚未订阅的（本轮符号级流全换或周期变化时常见）。两者皆空 = 重复
/// 下发同一订阅，改订本身可跳过
fn resub_diff(server: &[String], new_scoped: &[String]) -> (Vec<String>, Vec<String>) {
    let drop_s = server
        .iter()
        .filter(|s| !s.starts_with('!') && !new_scoped.contains(s))
        .cloned()
        .collect();
    let add_s = new_scoped
        .iter()
        .filter(|s| !server.contains(s))
        .cloned()
        .collect();
    (drop_s, add_s)
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
        // 用户数据流连接：现货 listenKey REST 端点已下线（2026-02-20 起
        // 410 Gone），改为连 WS API 后以 userDataStream.subscribe.signature
        // 订阅私有事件；期货仍走 listenKey（2026-03 起私有事件在
        // /private/ws/<listenKey>，legacy 入口已停止推送）
        let (url, mut sub_msg) = match market {
            Market::Spot => {
                let msg = match client.spot_user_sub_message(cfg.recv_window_ms) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("用户流订阅消息构建失败({market}): {e}");
                        state.set_user_ws(market, ConnState::Failed(e.friendly()));
                        repaint(&ctx);
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        continue;
                    }
                };
                (
                    crate::binance::ws_api_spot(cfg.testnet).to_string(),
                    Some(msg),
                )
            }
            Market::UmFutures => {
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
                    state.set_user_ws(
                        market,
                        ConnState::Failed(crate::i18n::tr("listenKey 响应异常").to_string()),
                    );
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                };
                (format!("{}/private/ws/{}", ep.ws, key), None)
            }
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
                let connected_at = std::time::Instant::now();
                let (mut write, mut read) = futures_util::StreamExt::split(ws);
                // 现货：建连后发送订阅请求并等待同 id 确认（status 200）；
                // 期货无订阅握手，连上即已生效
                if let Some(msg) = sub_msg.take() {
                    use tokio_tungstenite::tungstenite::Message;
                    let sub_id = msg
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let handshake = async {
                        write
                            .send(Message::text(msg.to_string()))
                            .await
                            .map_err(|e| e.to_string())?;
                        loop {
                            match read.next().await {
                                None => return Err(crate::i18n::tr("连接中断").to_string()),
                                Some(Err(e)) => return Err(e.to_string()),
                                Some(Ok(Message::Text(t))) => {
                                    let Ok(d) = serde_json::from_str::<serde_json::Value>(&t)
                                    else {
                                        continue;
                                    };
                                    if d.get("id").and_then(|x| x.as_str()) != Some(sub_id.as_str())
                                    {
                                        continue;
                                    }
                                    let status =
                                        d.get("status").and_then(|x| x.as_i64()).unwrap_or(0);
                                    if status == 200 {
                                        tracing::info!(
                                            "现货用户流订阅确认 {}",
                                            d.pointer("/result/subscriptionId")
                                                .map(|x| x.to_string())
                                                .unwrap_or_default()
                                        );
                                        return Ok(());
                                    }
                                    // 错误体顶层或 error 子对象均有出现（不同版本行为）
                                    let err = d.get("error").cloned().unwrap_or_else(|| d.clone());
                                    let code =
                                        err.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
                                    let m = err
                                        .get("msg")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or(crate::i18n::tr("未知错误"))
                                        .to_string();
                                    return Err(format!("({code}) {m}"));
                                }
                                Some(Ok(_)) => continue,
                            }
                        }
                    };
                    match tokio::time::timeout(Duration::from_secs(15), handshake).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::warn!("现货用户流订阅失败: {e}");
                            state.set_user_ws(market, ConnState::Failed(short_err(&e)));
                            repaint(&ctx);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                        Err(_) => {
                            tracing::warn!("现货用户流订阅确认超时");
                            state.set_user_ws(
                                market,
                                ConnState::Failed(crate::i18n::tr("订阅确认超时").to_string()),
                            );
                            repaint(&ctx);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                    }
                }
                tracing::info!("用户数据流已连接({market})");
                state.set_user_ws(market, ConnState::Connected);
                repaint(&ctx);
                // 重连后立即对账
                engine_tx.send(Command::RefreshAccount).ok();
                engine_tx.send(Command::RefreshOpenOrders { market }).ok();

                // 保活：期货 REST PUT listenKey（25min，listenKey 60min 有效期）；
                // 现货无 listenKey，改为每 2min 发一个轻量无签请求 session.status
                // ——既防中间设备空闲断连，其响应帧也喂饱 300s 空闲看门狗
                // （静默账户不会因无事件而被误判半开）
                let keepalive_period = match market {
                    Market::Spot => Duration::from_secs(120),
                    Market::UmFutures => Duration::from_secs(25 * 60),
                };
                let mut keepalive = tokio::time::interval(keepalive_period);
                keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // interval 首个 tick 立即完成，先消费掉，避免建连即触发保活
                keepalive.tick().await;
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
                            match market {
                                Market::Spot => {
                                    let ts_ms = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_millis() as i64)
                                        .unwrap_or(0);
                                    let ka = serde_json::json!({
                                        "id": format!("btp-ka-{ts_ms}"),
                                        "method": "session.status",
                                        "params": {},
                                    });
                                    if let Err(e) = write
                                        .send(tokio_tungstenite::tungstenite::Message::text(
                                            ka.to_string(),
                                        ))
                                        .await
                                    {
                                        tracing::warn!("用户数据流保活发送失败({market}): {e}，将重建");
                                        alive = false;
                                    }
                                }
                                Market::UmFutures => {
                                    if let Err(e) = client
                                        .key_auth(reqwest::Method::PUT, rc, &cfg, lk_path, &[])
                                        .await
                                    {
                                        tracing::warn!("listenKey 保活失败({market}): {e}，将重建");
                                        alive = false;
                                    }
                                }
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
                // 尽力收尾：期货 DELETE listenKey；现货退订（断连本身也会
                // 结束订阅，失败忽略）
                match market {
                    Market::Spot => {
                        let ts_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        let _ = write
                            .send(tokio_tungstenite::tungstenite::Message::text(
                                serde_json::json!({
                                    "id": format!("btp-usunsub-{ts_ms}"),
                                    "method": "userDataStream.unsubscribe",
                                    "params": {},
                                })
                                .to_string(),
                            ))
                            .await;
                    }
                    Market::UmFutures => {
                        let _ = client
                            .key_auth(reqwest::Method::DELETE, rc, &cfg, lk_path, &[])
                            .await;
                    }
                }
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
                is_conditional: false,
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
                is_conditional: false,
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
    fn resub_diff_symbol_switch() {
        // 现货槽位 A 连接建立后的服务端流（含市场级 miniTicker）
        let server = vec![
            "btcusdt@ticker".into(),
            "btcusdt@depth20@100ms".into(),
            "btcusdt@aggTrade".into(),
            "btcusdt@kline_1m".into(),
            "!miniTicker@arr".into(),
        ];
        // 切换到 ETHUSDT、周期不变
        let new = MarketWsSlot::A.scoped_streams("ethusdt", "1m", false);
        let (drop, add) = resub_diff(&server, &new);
        // 旧符号四个符号级流全部退订；市场级流必须保留
        assert_eq!(
            drop,
            vec![
                "btcusdt@ticker",
                "btcusdt@depth20@100ms",
                "btcusdt@aggTrade",
                "btcusdt@kline_1m"
            ]
        );
        assert_eq!(
            add,
            vec![
                "ethusdt@ticker",
                "ethusdt@depth20@100ms",
                "ethusdt@aggTrade",
                "ethusdt@kline_1m"
            ]
        );
        // 下一轮以新服务端视图计算：miniTicker 仍在
        let server2: Vec<String> = {
            let mut v = new.clone();
            v.push("!miniTicker@arr".into());
            v
        };
        let (d2, a2) = resub_diff(&server2, &new);
        assert!(
            d2.is_empty() && a2.is_empty(),
            "同订阅重复下发须判定为空差集"
        );
    }

    #[test]
    fn resub_diff_interval_only() {
        // 同交易对换周期：只有 kline 流变化
        let server = MarketWsSlot::B.scoped_streams("btcusdt", "15m", true);
        let new = MarketWsSlot::B.scoped_streams("btcusdt", "1h", true);
        let (drop, add) = resub_diff(&server, &new);
        assert_eq!(drop, vec!["btcusdt@kline_15m"]);
        assert_eq!(add, vec!["btcusdt@kline_1h"]);
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
        assert!(!handle_market_msg_text(
            &st,
            &spec,
            &ticker_frame("ETHUSDT")
        ));
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
        assert!(!handle_market_msg_text(
            &st,
            &spec,
            r#"{"result":null,"id":7}"#
        ));
    }

    #[test]
    fn history_segments_covers_window_in_7day_chunks() {
        const DAY: i64 = 86_400_000;
        let now = 1792175581000;
        let segs = history_segments(now, 30);
        // 30 天 = 5 段，段间首尾相接、无重叠，总覆盖 [now-30d, now]
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[0].1, now);
        assert_eq!(segs.last().unwrap().0, now - 30 * DAY);
        for w in segs.windows(2) {
            assert_eq!(w[0].0, w[1].1, "相邻段应首尾相接");
        }
        for (s, e) in &segs {
            assert!(*e - *s <= 7 * DAY, "每段不超过 7 天");
        }
        // 回退到非整周：floor 截断最后一段
        let segs = history_segments(now, 9);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].0, now - 9 * DAY);
    }

    #[test]
    fn flatten_history_merges_arrays_and_orders_wrapper() {
        // 数组段直接拼接
        let vals = vec![
            serde_json::json!([{"orderId": 1, "time": 2}]),
            serde_json::json!([{"orderId": 3, "time": 1}]),
        ];
        let list: Vec<OrderInfo> = flatten_history(vals).unwrap();
        assert_eq!(list.len(), 2);

        // {orders: [...]} 包裹段解包
        let vals = vec![serde_json::json!({"orders": [{"orderId": 7}]})];
        let list: Vec<OrderInfo> = flatten_history(vals).unwrap();
        assert_eq!(list[0].order_id, 7);

        // 非数组且无 orders 键：跳过该段（不污染其它段、不整体失败）
        let vals = vec![
            serde_json::json!([{"orderId": 1}]),
            serde_json::json!({"code": -1, "msg": "odd"}),
        ];
        let list: Vec<OrderInfo> = flatten_history(vals).unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn algo_to_order_info_maps_columns() {
        let a = AlgoOrderInfo {
            algo_id: 4000001973360743,
            symbol: "BTCUSDT".into(),
            side: "SELL".into(),
            order_type: "TAKE_PROFIT_MARKET".into(),
            algo_status: "NEW".into(),
            trigger_price: Some(Decimal::from(87200)),
            actual_qty: Decimal::ZERO,
            create_time: 1791449695293,
            ..Default::default()
        };
        let o = algo_to_order_info(&a);
        assert_eq!(o.order_id, a.algo_id);
        assert_eq!(o.order_type, "TAKE_PROFIT_MARKET");
        // 触发价同时落在价格列与触发价列
        assert_eq!(o.price.map(|x| x.to_string()).as_deref(), Some("87200"));
        assert_eq!(o.stop_price, o.price);
        assert_eq!(o.time, a.create_time);
        assert_eq!(o.status, "NEW");
    }

    #[test]
    fn algo_to_order_info_empty_status_defaults_to_new() {
        let a = AlgoOrderInfo {
            algo_id: 1,
            ..Default::default()
        };
        let o = algo_to_order_info(&a);
        assert_eq!(o.status, "NEW");
    }

    #[test]
    fn fetch_segments_fallback_is_single_request() {
        // 带时间参数：30 天分 5 段
        assert_eq!(fetch_segments(1_792_175_581_000, true).len(), 5);
        // 回退模式（端点拒绝 startTime/endTime）：只允许一段，
        // 否则会发出 N 个参数完全相同的请求、结果被拼接成 N 份重复记录
        assert_eq!(fetch_segments(1_792_175_581_000, false).len(), 1);
    }
}
