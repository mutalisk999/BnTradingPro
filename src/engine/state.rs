//! 引擎与 UI 共享的快照状态
//!
//! UI 线程只读快照 + 下发命令；后台 tokio 任务写快照后请求重绘。

use crate::binance::model::*;
use crate::binance::symbols::RuleBook;
use crate::binance::{Interval, Market};
use crate::config::AppConfig;
use parking_lot::{Mutex, RwLock};
use rust_decimal::Decimal;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::Notify;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ConnState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Ok,
    Warn,
    Err,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub msg: String,
    pub at: Instant,
}

/// 单交易对行情快照
#[derive(Debug, Clone, Default)]
pub struct SymbolMd {
    pub ticker: Option<Ticker24>,
    pub depth: Depth,
    pub trades: VecDeque<TradeTick>,
    pub klines: Vec<Candle>,
    pub interval: Interval,
    pub mark: Option<MarkPriceUpdate>,
    pub ws: ConnState,
}

/// 行情快照（分市场）
#[derive(Default)]
pub struct MarketMd {
    pub spot: HashMap<String, SymbolMd>,
    pub fut: HashMap<String, SymbolMd>,
    pub market_ws: [ConnState; 2],
    pub user_ws: [ConnState; 2],
}

impl MarketMd {
    pub fn slot(&mut self, market: Market) -> &mut HashMap<String, SymbolMd> {
        match market {
            Market::Spot => &mut self.spot,
            Market::UmFutures => &mut self.fut,
        }
    }

    pub fn get(&self, market: Market, symbol: &str) -> Option<&SymbolMd> {
        match market {
            Market::Spot => self.spot.get(symbol),
            Market::UmFutures => self.fut.get(symbol),
        }
    }

    pub fn market_ws(&self, market: Market) -> &ConnState {
        &self.market_ws[market as usize]
    }

    pub fn user_ws(&self, market: Market) -> &ConnState {
        &self.user_ws[market as usize]
    }
}

#[derive(Default)]
pub struct AccountSnap {
    pub spot: Option<SpotAccount>,
    pub fut: Option<FutAccount>,
    pub fut_positions: Vec<PositionRisk>,
    pub fut_dual_side: bool,
    pub fut_leverage: HashMap<String, Decimal>,
    pub spot_loaded: bool,
    pub fut_loaded: bool,
}

#[derive(Default)]
pub struct OrdersSnap {
    pub open: [Vec<OrderInfo>; 2],
    pub history_symbol: Option<(Market, String)>,
    pub history: Vec<OrderInfo>,
    pub trades: Vec<MyTrade>,
}

impl OrdersSnap {
    pub fn open(&self, market: Market) -> &Vec<OrderInfo> {
        &self.open[market as usize]
    }
}

/// 全局共享状态
pub struct SharedState {
    pub cfg: RwLock<AppConfig>,
    pub creds: RwLock<Option<crate::security::vault::Creds>>,

    /// 配置/凭据版本号，变更后各后台任务重连
    pub cfg_version: AtomicU64,
    pub creds_version: AtomicU64,
    pub version_notify: Notify,

    pub rest_ok: AtomicBool,

    pub rules: RwLock<[RuleBook; 2]>,
    pub md: Mutex<MarketMd>,
    pub acct: Mutex<AccountSnap>,
    pub orders: Mutex<OrdersSnap>,
    pub toasts: Mutex<Vec<Toast>>,
    pub busy: Mutex<HashMap<String, u32>>,

    /// 引擎命令通道（UI 与后台事件共用）
    pub cmd_tx: RwLock<Option<tokio::sync::mpsc::UnboundedSender<crate::engine::Command>>>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            cfg: RwLock::new(AppConfig::default()),
            creds: RwLock::new(None),
            cfg_version: AtomicU64::new(1),
            creds_version: AtomicU64::new(0),
            version_notify: Notify::new(),
            rest_ok: AtomicBool::new(false),
            rules: RwLock::new([RuleBook::default(), RuleBook::default()]),
            md: Mutex::new(MarketMd::default()),
            acct: Mutex::new(AccountSnap::default()),
            orders: Mutex::new(OrdersSnap::default()),
            toasts: Mutex::new(Vec::new()),
            busy: Mutex::new(HashMap::new()),
            cmd_tx: RwLock::new(None),
        }
    }
}

impl SharedState {
    pub fn has_creds(&self) -> bool {
        self.creds.read().is_some()
    }

    /// 单个交易对的规则（避免整本规则簿克隆）
    pub fn rule_of(
        &self,
        market: Market,
        symbol: &str,
    ) -> Option<crate::binance::symbols::SymbolRules> {
        self.rules.read()[market as usize].get(symbol).cloned()
    }

    /// 更新配置并唤醒所有后台任务（重建连接）
    pub fn update_cfg(&self, cfg: AppConfig) {
        *self.cfg.write() = cfg;
        self.cfg_version.fetch_add(1, Ordering::Relaxed);
        self.version_notify.notify_waiters();
        self.send_cmd(crate::engine::Command::ReloadAll);
    }

    /// 设置解密后的凭据并触发账户数据流重建
    pub fn set_creds(&self, creds: Option<crate::security::vault::Creds>) {
        *self.creds.write() = creds;
        self.creds_version.fetch_add(1, Ordering::Relaxed);
        self.version_notify.notify_waiters();
        self.send_cmd(crate::engine::Command::ReloadAll);
    }

    pub fn send_cmd(&self, cmd: crate::engine::Command) {
        if let Some(tx) = self.cmd_tx.read().as_ref() {
            let _ = tx.send(cmd);
        }
    }

    pub fn cfg_snapshot(&self) -> AppConfig {
        self.cfg.read().clone()
    }

    pub fn rules_of(&self, market: Market) -> RuleBook {
        self.rules.read()[market as usize].clone()
    }

    pub fn push_toast(&self, kind: ToastKind, msg: impl Into<String>) {
        let mut t = self.toasts.lock();
        t.push(Toast {
            kind,
            msg: msg.into(),
            at: Instant::now(),
        });
        if t.len() > 8 {
            let drop_n = t.len() - 8;
            t.drain(0..drop_n);
        }
    }

    pub fn set_busy(&self, key: &str, on: bool) {
        let mut b = self.busy.lock();
        let e = b.entry(key.to_string()).or_insert(0);
        if on {
            *e += 1;
        } else {
            *e = e.saturating_sub(1);
            if *e == 0 {
                b.remove(key);
            }
        }
    }

    pub fn is_busy(&self, key: &str) -> bool {
        self.busy.lock().contains_key(key)
    }

    // ---- 行情写入（由后台任务调用） ----

    pub fn with_symbol_md(&self, market: Market, symbol: &str, f: impl FnOnce(&mut SymbolMd)) {
        let mut md = self.md.lock();
        let slot = md.slot(market);
        let e = slot.entry(symbol.to_string()).or_default();
        f(e);
    }

    pub fn set_market_ws(&self, market: Market, st: ConnState) {
        let mut md = self.md.lock();
        md.market_ws[market as usize] = st;
    }

    pub fn set_user_ws(&self, market: Market, st: ConnState) {
        let mut md = self.md.lock();
        md.user_ws[market as usize] = st;
    }

    /// 合并 K 线事件（替换未收盘 / 追加新 K 线）
    pub fn apply_kline(&self, market: Market, symbol: &str, interval: Interval, k: &WsKlineInner) {
        if Interval::from_str(&k.interval) != interval {
            return;
        }
        self.with_symbol_md(market, symbol, |md| {
            let candle = Candle {
                t: k.start_time,
                o: k.open,
                h: k.high,
                l: k.low,
                c: k.close,
                v: k.volume,
                closed: k.closed,
            };
            match md.klines.last_mut() {
                Some(last) if last.t == candle.t => *last = candle,
                Some(last) if candle.t > last.t => {
                    last.closed = true;
                    md.klines.push(candle);
                    if md.klines.len() > 1500 {
                        let n = md.klines.len() - 1500;
                        md.klines.drain(0..n);
                    }
                }
                _ => {}
            }
        });
    }

    pub fn apply_trades(&self, market: Market, symbol: &str, t: TradeTick) {
        self.with_symbol_md(market, symbol, |md| {
            md.trades.push_back(t);
            while md.trades.len() > 100 {
                md.trades.pop_front();
            }
        });
    }
}
