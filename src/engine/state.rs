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

/// 当前 Unix 毫秒时间戳
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

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
    /// K 线内容版本号：每次 K 线写入/替换时递增，UI 侧图表据此做克隆缓存
    pub kline_version: u64,
    /// 上次因逐笔成交推进K线而递增版本号的毫秒时间戳（节流用）
    pub last_kline_bump_ms: i64,
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
    /// 上次账户 REST 拉取的失败原因（None = 成功或尚未尝试）；
    /// None 且账户为空才是"加载中"，有错误必须显式给出重试入口
    pub spot_err: Option<String>,
    pub fut_err: Option<String>,
    pub fut_positions: Vec<PositionRisk>,
    pub fut_dual_side: bool,
    pub fut_leverage: HashMap<String, Decimal>,
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

    /// 配置/凭据版本号，变更后各后台任务比对重连
    pub cfg_version: AtomicU64,
    pub creds_version: AtomicU64,

    pub rest_ok: AtomicBool,

    pub rules: RwLock<[RuleBook; 2]>,
    /// 规则簿代次：ReloadAll 重写后递增（UI 侧符号列表缓存据此失效）
    pub rules_version: AtomicU64,
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
            rest_ok: AtomicBool::new(false),
            rules: RwLock::new([RuleBook::default(), RuleBook::default()]),
            rules_version: AtomicU64::new(0),
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

    /// 自选交易对列表
    pub fn favorites(&self, market: Market) -> Vec<String> {
        let cfg = self.cfg.read();
        match market {
            Market::Spot => cfg.favorites_spot.clone(),
            Market::UmFutures => cfg.favorites_futures.clone(),
        }
    }

    /// 切换自选（落盘走引擎命令，UI 线程只改内存，不触发引擎重连）
    pub fn toggle_favorite(&self, market: Market, symbol: &str) {
        {
            let mut cfg = self.cfg.write();
            let list = match market {
                Market::Spot => &mut cfg.favorites_spot,
                Market::UmFutures => &mut cfg.favorites_futures,
            };
            if let Some(i) = list.iter().position(|x| x == symbol) {
                list.remove(i);
            } else {
                list.insert(0, symbol.to_string());
            }
        }
        self.send_cmd(crate::engine::Command::PersistConfig);
    }

    /// 单个交易对的规则（避免整本规则簿克隆）
    pub fn rule_of(
        &self,
        market: Market,
        symbol: &str,
    ) -> Option<crate::binance::symbols::SymbolRules> {
        self.rules.read()[market as usize].get(symbol).cloned()
    }

    /// 只读借用单条规则（不克隆）——UI 每帧多次读取精度等小字段时使用
    pub fn with_rule<R>(
        &self,
        market: Market,
        symbol: &str,
        f: impl FnOnce(Option<&crate::binance::symbols::SymbolRules>) -> R,
    ) -> R {
        let rules = self.rules.read();
        f(rules[market as usize].get(symbol))
    }

    /// 更新配置（后台任务通过版本号感知并重连）
    pub fn update_cfg(&self, cfg: AppConfig) {
        *self.cfg.write() = cfg;
        self.cfg_version.fetch_add(1, Ordering::Relaxed);
        self.send_cmd(crate::engine::Command::ReloadAll);
    }

    /// 设置解密后的凭据并触发账户数据流重建
    pub fn set_creds(&self, creds: Option<crate::security::vault::Creds>) {
        *self.creds.write() = creds;
        self.creds_version.fetch_add(1, Ordering::Relaxed);
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
                }
                // 空缓冲（REST 快照失败/未加载时）也接受 WS 事件起步
                None => md.klines.push(candle),
                _ => {}
            }
            if md.klines.len() > 1500 {
                let n = md.klines.len() - 1500;
                md.klines.drain(0..n);
            }
            md.kline_version = md.kline_version.wrapping_add(1);
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

    /// 用逐笔成交实时推进最后一根未收盘 K 线。
    /// kline 流本身约 2s 才推送一次，若只依赖它，最后一根蜡烛的收盘价
    /// 会滞后成交价；aggTrade 每笔都到，用它把开高低收量跟着成交推进。
    /// 版本号递增按 200ms 节流：图表克隆+前缀和重建最多 5Hz，足够平滑。
    pub fn apply_trade_to_kline(&self, market: Market, symbol: &str, t: &TradeTick) {
        self.with_symbol_md(market, symbol, |md| {
            let Some(last) = md.klines.last_mut() else {
                return;
            };
            if last.closed || t.time < last.t {
                return;
            }
            if t.time >= last.t + md.interval.ms() {
                // 已跨周期：新蜡烛由 kline 流开启，这里不凭空造蜡烛
                return;
            }
            last.c = t.price;
            if t.price > last.h {
                last.h = t.price;
            }
            if t.price < last.l {
                last.l = t.price;
            }
            last.v += t.qty;
            let now = now_ms();
            if now - md.last_kline_bump_ms >= 200 {
                md.last_kline_bump_ms = now;
                md.kline_version = md.kline_version.wrapping_add(1);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kline(t: i64, interval: &str, closed: bool) -> WsKlineInner {
        WsKlineInner {
            start_time: t,
            interval: interval.into(),
            open: Decimal::ONE,
            high: Decimal::ONE,
            low: Decimal::ONE,
            close: Decimal::ONE,
            volume: Decimal::ONE,
            closed,
        }
    }

    #[test]
    fn kline_merge_replace_append_ignore() {
        let st = SharedState::default();
        // 同一根 K 线重复推送 → 替换不追加，收盘标记生效
        st.apply_kline(
            Market::Spot,
            "BTCUSDT",
            Interval::M1,
            &kline(100, "1m", false),
        );
        st.apply_kline(
            Market::Spot,
            "BTCUSDT",
            Interval::M1,
            &kline(100, "1m", true),
        );
        {
            let md = st.md.lock();
            let m = md.get(Market::Spot, "BTCUSDT").unwrap();
            assert_eq!(m.klines.len(), 1);
            assert!(m.klines[0].closed);
        }
        // 新 K 线 → 追加，且前一根标记为已收盘
        st.apply_kline(
            Market::Spot,
            "BTCUSDT",
            Interval::M1,
            &kline(60_000, "1m", false),
        );
        {
            let md = st.md.lock();
            let m = md.get(Market::Spot, "BTCUSDT").unwrap();
            assert_eq!(m.klines.len(), 2);
            assert!(m.klines[0].closed);
            assert!(!m.klines[1].closed);
        }
        // 乱序/过期事件 → 忽略
        st.apply_kline(
            Market::Spot,
            "BTCUSDT",
            Interval::M1,
            &kline(50_000, "1m", false),
        );
        // 周期不匹配 → 忽略
        st.apply_kline(
            Market::Spot,
            "BTCUSDT",
            Interval::M1,
            &kline(120_000, "5m", false),
        );
        {
            let md = st.md.lock();
            assert_eq!(md.get(Market::Spot, "BTCUSDT").unwrap().klines.len(), 2);
        }
        // 超过 1500 根时丢弃最旧
        for i in 0..1600u64 {
            st.apply_kline(
                Market::Spot,
                "ETHUSDT",
                Interval::M1,
                &kline((i * 60_000) as i64, "1m", false),
            );
        }
        let md = st.md.lock();
        let m = md.get(Market::Spot, "ETHUSDT").unwrap();
        assert_eq!(m.klines.len(), 1500);
        assert_eq!(m.klines[0].t, 100 * 60_000);
    }

    #[test]
    fn trade_updates_last_kline() {
        let st = SharedState::default();
        // 生产路径中 md.interval 由 Subscribe 快照先写入，这里对齐
        st.with_symbol_md(Market::Spot, "BTCUSDT", |md| md.interval = Interval::M1);
        st.apply_kline(
            Market::Spot,
            "BTCUSDT",
            Interval::M1,
            &kline(60_000, "1m", false),
        );
        let trade = |time: i64, price: &str, qty: &str| TradeTick {
            id: 1,
            price: Decimal::from_str_exact(price).unwrap(),
            qty: Decimal::from_str_exact(qty).unwrap(),
            time,
            buyer_maker: false,
        };
        // 周期内成交 → 推进收/高/低/量
        st.apply_trade_to_kline(Market::Spot, "BTCUSDT", &trade(90_000, "105", "2"));
        {
            let md = st.md.lock();
            let c = &md.get(Market::Spot, "BTCUSDT").unwrap().klines[0];
            assert_eq!(c.c.to_string(), "105");
            assert_eq!(c.h.to_string(), "105");
            // 种子蜡烛自带量 1 + 成交 2
            assert_eq!(c.v.to_string(), "3");
        }
        // 更高价/更低价（种子蜡烛 low=1，用 0.9 才能压低最低价）
        st.apply_trade_to_kline(Market::Spot, "BTCUSDT", &trade(95_000, "0.9", "1"));
        st.apply_trade_to_kline(Market::Spot, "BTCUSDT", &trade(96_000, "110", "1"));
        {
            let md = st.md.lock();
            let c = &md.get(Market::Spot, "BTCUSDT").unwrap().klines[0];
            assert_eq!(c.l.to_string(), "0.9");
            assert_eq!(c.h.to_string(), "110");
            assert_eq!(c.v.to_string(), "5");
        }
        // 跨周期成交 → 忽略（新蜡烛由 kline 流开启）
        st.apply_trade_to_kline(Market::Spot, "BTCUSDT", &trade(130_000, "999", "5"));
        {
            let md = st.md.lock();
            let m = md.get(Market::Spot, "BTCUSDT").unwrap();
            assert_eq!(m.klines.len(), 1);
            assert_eq!(m.klines[0].c.to_string(), "110");
        }
        // 已收盘蜡烛 → 忽略
        st.apply_kline(
            Market::Spot,
            "BTCUSDT",
            Interval::M1,
            &kline(120_000, "1m", true),
        );
        st.apply_trade_to_kline(Market::Spot, "BTCUSDT", &trade(125_000, "500", "1"));
        {
            let md = st.md.lock();
            let m = md.get(Market::Spot, "BTCUSDT").unwrap();
            assert_eq!(m.klines[1].c.to_string(), "1");
        }
    }
}
