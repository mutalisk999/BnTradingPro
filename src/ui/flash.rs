//! 价格变动高亮：数值变化时行底色短暂着色（涨/跌色），约 500ms 内渐变回原色。
//!
//! 保留模式纪律：
//! - 状态记录在应用视图（AppView）上跨帧存活；每位点只做一次整数比较 + 一次时间比较
//! - 盘口位点用定长数组按 (侧, 档位) 索引，全程零堆分配
//! - 时间源取 `std::time::Instant::now()`（原 egui 版取内部秒表，
//!   GPUI 无对应 API，Instant 单调时钟语义一致），自行管理衰减起点：
//!   同一方向连续变动时把 500ms 倒计时重新清零

use gpui::Rgba;
use std::time::Instant;

/// 单个数值位点的观测者
#[derive(Debug, Clone, Copy, Default)]
pub struct Flash {
    /// 上一次数值的有序 bit pattern（`f64::to_bits`）
    last: u64,
    /// 1 = 上涨闪烁中，-1 = 下跌闪烁中，0 = 无变化/已结束
    dir: i8,
    /// 本次闪烁起始时间（Instant；Default 的 Instant 取 epoch，首值必然
    /// 早于任何 now，dir=0 短路后无影响）
    start: Option<Instant>,
    /// 是否已见到过有效值（首次到达不着色，避免初加载整片闪烁）
    seen: bool,
}

impl Flash {
    /// 观测一个新值：返回 (强度 0-1, 方向)。值变化即重置 500ms 倒计时
    fn observe(&mut self, now: Instant, new_bits: u64) -> (f32, i8) {
        if new_bits != self.last {
            if self.seen {
                self.dir = if new_bits > self.last { 1 } else { -1 };
                self.start = Some(now);
            }
            self.last = new_bits;
            self.seen = true;
        }
        let Some(start) = self.start else {
            return (0.0, 0);
        };
        if self.dir == 0 {
            return (0.0, 0);
        }
        // 线性衰减：FLASH_SECS 内强度从 1 回到 0
        let t = now.duration_since(start).as_secs_f64()
            / crate::ui::theme::FLASH_SECS as f64;
        if t >= 1.0 {
            self.dir = 0;
            (0.0, 0)
        } else {
            ((1.0 - t) as f32, self.dir)
        }
    }

    /// f64 观测：0/NaN 统一规整为 0，避免 NaN 比较恒不等导致每帧重复触发
    fn observe_f64(&mut self, now: Instant, v: f64) -> (f32, i8) {
        let bits = if v == 0.0 || v.is_nan() {
            0
        } else {
            v.to_bits()
        };
        self.observe(now, bits)
    }
}

/// 由闪烁方向取高亮底色（随 `price_dir` 翻转）
pub fn flash_color(dir: i8) -> Rgba {
    if dir > 0 {
        crate::ui::theme::up_color()
    } else {
        crate::ui::theme::down_color()
    }
}

// ---------- 盘口位点集合 ----------

/// 盘口位点：卖盘/买盘 × 价格/数量（判别值 0..4，用作数组下标）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookSite {
    AskPrice = 0,
    AskQty = 1,
    BidPrice = 2,
    BidQty = 3,
}

/// 盘口各档位点：定长数组，启动时一次分配，运行期无堆操作
#[derive(Debug, Clone)]
pub struct FlashBook {
    slots: [Flash; 4 * crate::ui::market::BOOK_MAX_LEVELS],
}

impl Default for FlashBook {
    fn default() -> Self {
        Self {
            slots: [Flash::default(); 4 * crate::ui::market::BOOK_MAX_LEVELS],
        }
    }
}

impl FlashBook {
    const fn idx(site: BookSite, i: usize) -> usize {
        (site as usize) * crate::ui::market::BOOK_MAX_LEVELS + i
    }

    /// 是否仍有位点在衰减中（供心跳调度判断）
    pub fn active(&self) -> bool {
        self.slots.iter().any(|f| f.dir != 0)
    }
}

impl BookSite {
    /// 同侧的数量位点（价格位点 ↔ 数量位点配对）
    pub const fn qty_site(self) -> Self {
        match self {
            BookSite::AskPrice => BookSite::AskQty,
            BookSite::AskQty => BookSite::AskPrice,
            BookSite::BidPrice => BookSite::BidQty,
            BookSite::BidQty => BookSite::BidPrice,
        }
    }
}

/// 逐笔成交位点：只观测最新一笔的价格
#[derive(Debug, Clone, Default)]
pub struct FlashTrade(Flash);

/// 页头最新价位点
#[derive(Debug, Clone, Default)]
pub struct FlashHeader(Flash);

impl FlashTrade {
    /// 是否仍在衰减中
    pub fn active(&self) -> bool {
        self.0.dir != 0
    }
}

impl FlashHeader {
    /// 是否仍在衰减中
    pub fn active(&self) -> bool {
        self.0.dir != 0
    }
}

/// 位点集合宿主：GPUI 版不再挂在 BnApp 上，由主视图 Entity 持有此结构，
/// 渲染/事件回调里调用其方法
#[derive(Debug, Clone, Default)]
pub struct FlashState {
    pub book: FlashBook,
    pub trade: FlashTrade,
    pub header: FlashHeader,
}

impl FlashState {
    /// 页头最新价：(强度, 方向)。无变化时强度恒为 0
    pub fn header_px(&mut self, v: f64) -> (f32, i8) {
        self.header.0.observe_f64(Instant::now(), v)
    }

    /// 盘口单档位点：`idx` 为该侧数组下标（0 = 最优档）
    pub fn book(&mut self, site: BookSite, idx: usize, v: f64) -> (f32, i8) {
        if idx >= crate::ui::market::BOOK_MAX_LEVELS {
            return (0.0, 0);
        }
        self.book.slots[FlashBook::idx(site, idx)].observe_f64(Instant::now(), v)
    }

    /// 逐笔成交最新行：调用方每次渲染传入"当前最新一笔"的价格。
    /// 新成交到达时价格变化即触发闪烁；同一笔重复渲染不重复触发
    pub fn trade_top(&mut self, price: f64) -> (f32, i8) {
        self.trade.0.observe_f64(Instant::now(), price)
    }

    /// 是否仍有位点在衰减中（供心跳档位判断）
    pub fn active(&self) -> bool {
        self.book.active() || self.trade.active() || self.header.active()
    }

    /// 清空全部位点（切换交易对/市场时调用，避免旧数据的首值被误判为变动）
    pub fn reset(&mut self) {
        *self = FlashState::default();
    }
}
