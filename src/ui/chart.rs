//! 自绘蜡烛图：缩放/平移、十字光标、最新价与挂单/持仓叠加线、副图指标
//!
//! 里程碑 1：ChartState 与纯数学辅助函数原样迁移（无 egui 依赖）；
//! canvas 绘制在里程碑 6 移植。

use crate::binance::model::Candle;
use crate::binance::Interval;
use crate::ui::indicators::SubSeries;

// 供 app.rs 以 `chart::SubIndicator` 引用（保持既有导入路径不变）
pub use crate::ui::indicators::SubIndicator;

/// 均线周期（MA5/MA10/MA20/MA60）；配色见 `theme::ma_colors()`（随主题切换）
pub const MA_PERIODS: [usize; 4] = [5, 10, 20, 60];

#[derive(Debug, Clone)]
pub struct ChartState {
    pub bars_visible: f32,
    pub right_off: f32,
    pub ma: [bool; 4],
    /// K 线克隆缓存：按 (市场, 交易对, 周期, kline_version) 失效。
    /// 之前每帧深克隆最多 1500 根 K 线并全量重建 SMA 前缀和（~10MB/s 分配），
    /// 缓存后仅在数据变化时重算一次
    cache_key: Option<(crate::binance::Market, String, Interval, u64)>,
    cache_klines: Vec<Candle>,
    cache_closes: Vec<f64>,
    /// 收盘价前缀和（长度 = closes.len()+1）：所有周期 SMA 共用，
    /// 查询 (prefix[gi+1]-prefix[gi+1-period])/period
    cache_prefix: Vec<f64>,
    /// 副图指标序列缓存：按 (市场, 交易对, 周期, kline_version, 指标) 失效
    sub_key: Option<(crate::binance::Market, String, Interval, u64, SubIndicator)>,
    sub_series: Option<SubSeries>,
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            bars_visible: 120.0,
            right_off: 0.0,
            ma: [true, true, true, true],
            cache_key: None,
            cache_klines: Vec::new(),
            cache_closes: Vec::new(),
            cache_prefix: Vec::new(),
            sub_key: None,
            sub_series: None,
        }
    }
}

/// 图表叠加线（挂单/强平价等）：绘制时取主题色
pub struct Overlay {
    pub price: f64,
    /// 颜色按角色语义存为字符串（"up"/"down"/"accent"/"warn"/"dim"），
    /// 绘制时经 theme::c() 解析——GPUI 色板是运行时全局，不能在状态里存死
    pub color: gpui::Rgba,
    pub label: String,
}

fn time_label(ms: i64, interval: Interval) -> String {
    use chrono::TimeZone;
    let t = match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => t,
        None => return "—".into(),
    };
    match interval {
        Interval::MO1 => t.format("%Y-%m").to_string(),
        Interval::D1 | Interval::D3 | Interval::W1 => t.format("%m-%d").to_string(),
        _ => t.format("%H:%M").to_string(),
    }
}

fn time_label_full(ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        None => "—".into(),
    }
}

/// 选择"好看"的网格步长
fn nice_step(range: f64, target: usize) -> f64 {
    if range <= 0.0 {
        return 1.0;
    }
    let raw = range / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if raw <= m * mag {
            return m * mag;
        }
    }
    10.0 * mag
}
