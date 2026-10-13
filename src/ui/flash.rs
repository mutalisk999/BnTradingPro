//! 价格变动高亮：数值变化时行底色短暂着色（涨/跌色），约 500ms 内渐变回原色。
//!
//! 即时模式纪律：
//! - 状态记录在 `BnApp` 上跨帧存活；每帧每位点只做一次整数比较 + 一次时间比较
//! - 盘口位点用定长数组按 (侧, 档位) 索引，全程零堆分配
//! - 时间源取 `ctx.input(|i| i.time)`（egui 内部秒表），自行管理衰减起点：
//!   egui 的 `animate_value_with_time` 在目标值不变时不会重启动画，
//!   无法表达"同一方向连续变动时把 500ms 倒计时重新清零"

use crate::app::BnApp;
use eframe::egui;

/// 单个数值位点的观测者
#[derive(Debug, Clone, Copy, Default)]
pub struct Flash {
    /// 上一次数值的有序 bit pattern（`f64::to_bits`）
    last: u64,
    /// 1 = 上涨闪烁中，-1 = 下跌闪烁中，0 = 无变化/已结束
    dir: i8,
    /// 本次闪烁起始时间（`ctx.input(|i| i.time)`，秒）
    start: f64,
    /// 是否已见到过有效值（首次到达不着色，避免初加载整片闪烁）
    seen: bool,
}

impl Flash {
    /// 观测一个新值：返回 (强度 0-1, 方向)。值变化即重置 500ms 倒计时
    fn observe(&mut self, now: f64, new_bits: u64) -> (f32, i8) {
        if new_bits != self.last {
            if self.seen {
                self.dir = if new_bits > self.last { 1 } else { -1 };
                self.start = now;
            }
            self.last = new_bits;
            self.seen = true;
        }
        if self.dir == 0 {
            return (0.0, 0);
        }
        // 线性衰减：FLASH_SECS 内强度从 1 回到 0
        let t = (now - self.start) / crate::ui::theme::FLASH_SECS as f64;
        if t >= 1.0 {
            self.dir = 0;
            (0.0, 0)
        } else {
            ((1.0 - t) as f32, self.dir)
        }
    }

    /// f64 观测：0/NaN 统一规整为 0，避免 NaN 比较恒不等导致每帧重复触发
    fn observe_f64(&mut self, now: f64, v: f64) -> (f32, i8) {
        let bits = if v == 0.0 || v.is_nan() { 0 } else { v.to_bits() };
        self.observe(now, bits)
    }
}

/// 由闪烁方向取高亮底色（随 `price_dir` 翻转）
pub fn flash_color(dir: i8) -> egui::Color32 {
    if dir > 0 {
        crate::ui::theme::up_color()
    } else {
        crate::ui::theme::down_color()
    }
}

/// 颜色插值（t=0 → from，t=1 → to）：用于无法铺整行底色的区域做文字闪烁
pub fn lerp_color(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |a: u8, b: u8| {
        let a = a as f32;
        let b = b as f32;
        (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
    };
    egui::Color32::from_rgb(ch(from.r(), to.r()), ch(from.g(), to.g()), ch(from.b(), to.b()))
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
    slots: [Flash; 4 * crate::ui::market::BOOK_LEVELS],
}

impl Default for FlashBook {
    fn default() -> Self {
        Self {
            slots: [Flash::default(); 4 * crate::ui::market::BOOK_LEVELS],
        }
    }
}

impl FlashBook {
    const fn idx(site: BookSite, i: usize) -> usize {
        (site as usize) * crate::ui::market::BOOK_LEVELS + i
    }

    /// 是否仍有位点在衰减中（供重绘调度判断）
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

impl BnApp {
    /// 页头最新价：(强度, 方向)。无变化时强度恒为 0
    pub fn flash_header_px(&mut self, ctx: &egui::Context, v: f64) -> (f32, i8) {
        let now = ctx.input(|i| i.time);
        self.flash_header.0.observe_f64(now, v)
    }

    /// 盘口单档位点：`idx` 为该侧数组下标（0 = 最优档）
    pub fn flash_book(
        &mut self,
        ctx: &egui::Context,
        site: BookSite,
        idx: usize,
        v: f64,
    ) -> (f32, i8) {
        if idx >= crate::ui::market::BOOK_LEVELS {
            return (0.0, 0);
        }
        let now = ctx.input(|i| i.time);
        self.flash_book.slots[FlashBook::idx(site, idx)].observe_f64(now, v)
    }

    /// 逐笔成交最新行：调用方每帧传入"当前最新一笔"的价格。
    /// 新成交到达时价格变化即触发闪烁；同一笔重复渲染不重复触发
    pub fn flash_trade_top(&mut self, ctx: &egui::Context, price: f64) -> (f32, i8) {
        let now = ctx.input(|i| i.time);
        self.flash_trade.0.observe_f64(now, price)
    }

    /// 清空全部位点（切换交易对/市场时调用，避免旧数据的首值被误判为变动）
    pub fn flash_reset(&mut self) {
        self.flash_book = FlashBook::default();
        self.flash_trade = FlashTrade::default();
        self.flash_header = FlashHeader::default();
    }
}
