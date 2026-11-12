//! 通用小控件：状态灯、Toast 提示、格式化工具
//!
//! 里程碑 1：格式化纯函数与状态灯/Toast 的 GPUI 数据侧先行移植（测试全绿）；
//! 视觉组件在里程碑 2 完整重建（对应旧 egui 版 spec：状态灯 = 名称 + 10px
//! 圆点 + 光晕、Toast = 右下角堆叠卡片 + kind 色条 + TTL 进度条 + 点击关闭）。

use crate::engine::state::{ConnState, SharedState, Toast, ToastKind};
use gpui::{div, px, Div, ParentElement, Styled};
use rust_decimal::Decimal;

pub fn parse_dec(s: &str) -> Option<Decimal> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    Decimal::from_str_exact(t).ok()
}

/// Decimal → f64 数值转换（替代 to_string().parse()，避免每帧大量字符串堆分配）
pub fn f64_decimal(d: &Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    d.to_f64().unwrap_or(0.0)
}

/// 展示层千分位分组：绝对值 ≥1000 时按 Decimal 自身小数位分组（不丢失精度语境），
/// 其余保持 Decimal 全精度原文。价格不被四舍五入，仅加千分位分隔。
pub fn fmt_dec_grouped(d: &Decimal) -> String {
    if d.abs() >= Decimal::from(1000) {
        let dp = (d.scale() as usize).min(8);
        fmt_thousands_dec(d, dp)
    } else {
        d.normalize().to_string()
    }
}

/// 按交易对规则精度定长显示：小数位固定 dp，≥1000 附千分位分组。
/// 价格用 quote 资产侧精度（tickSize 小数位）、数量用 base 资产侧精度
/// （stepSize 小数位），同一列表格纵向小数点对齐
pub fn fmt_dec_fixed(d: &Decimal, dp: usize) -> String {
    if d.abs() >= Decimal::from(1000) {
        fmt_thousands_dec(d, dp)
    } else {
        fmt_dec_plain(d, dp)
    }
}

/// 按精度定长显示、不附千分位分组：侧栏等紧凑场景需要纯粹的价格数字，
/// 逗号既占宽度又打断纵向扫读（68,432.15 → 68432.15）
pub fn fmt_dec_plain(d: &Decimal, dp: usize) -> String {
    // 纯 Decimal 路径：round_dp 负责按精度舍入，Display 的 {:.*} 负责末位补零。
    // 全程不经过 f64——f64 只有 15~17 位有效数字，高精度币种（8 位小数）
    // 或币昂服务端返回的双精度残差（如 entryPrice 86997.7000000001）
    // 经 f64 中转后末位会露出脏数字
    // 舍入策略显式取 MidpointAwayFromZero：rust_decimal 的 round_dp 默认是
    // 银行家舍入（half-to-even，0.125 → 0.12），金融展示惯例是四舍五入（→ 0.13）
    format!("{:.*}", dp, round_money(d, dp))
}

/// 金额/价格的展示舍入：half away from zero（四舍五入）
fn round_money(d: &Decimal, dp: usize) -> Decimal {
    d.round_dp_with_strategy(
        dp as u32,
        rust_decimal::RoundingStrategy::MidpointAwayFromZero,
    )
}

/// 千分位分组格式化（用于展示层；坐标轴等本身就是 f64 的场景）
pub fn fmt_thousands(v: f64, dp: usize) -> String {
    group_thousands(&format!("{:.*}", dp, v))
}

/// 千分位分组（Decimal 源）：舍入与定长由 Decimal 完成，仅分组逻辑与
/// `fmt_thousands` 共用
fn fmt_thousands_dec(d: &Decimal, dp: usize) -> String {
    group_thousands(&format!("{:.*}", dp, round_money(d, dp)))
}

/// 已格式化数字串的 3 位分组：整数部分从右起每 3 位插一个逗号
fn group_thousands(s: &str) -> String {
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let neg = int_part.starts_with('-');
    let digits = int_part.trim_start_matches('-');
    let bytes = digits.as_bytes();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(*b as char);
    }
    let sign = if neg { "-" } else { "" };
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// 大数缩写（成交额）：中文 1.2亿 / 34.5万；英文 1.2B / 34.5K
pub fn fmt_compact(v: f64) -> String {
    fmt_compact_lang(v, crate::i18n::lang())
}

/// 按指定语言缩写：参数化版供测试并行使用——测试不得改全局 LANG
/// （i18n LANG 是进程级 AtomicU8，并行测试一个 set_lang 会把其它
/// 读全局的测试带偏，实测偶发 "34.5K" != "3.5万"）
fn fmt_compact_lang(v: f64, lang: u8) -> String {
    let a = v.abs();
    if lang == crate::i18n::EN {
        if a >= 1e9 {
            format!("{:.2}B", v / 1e9)
        } else if a >= 1e6 {
            format!("{:.2}M", v / 1e6)
        } else if a >= 1e3 {
            format!("{:.1}K", v / 1e3)
        } else if a >= 1.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.4}")
        }
    } else if a >= 1e8 {
        format!("{:.2}亿", v / 1e8)
    } else if a >= 1e4 {
        format!("{:.1}万", v / 1e4)
    } else if a >= 1.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.4}")
    }
}

/// 单条 Toast 的样式常量（视觉组件在里程碑 2 落地时引用）
pub const TOAST_TTL_SECS: f64 = 5.0;
/// 错误类保留更久：下单被拒/断线等提示 5 秒即消失极易错过
pub const TOAST_TTL_ERR_SECS: f64 = 15.0;
/// Toast 面板最大宽度
pub const TOAST_MAX_W: f32 = 420.0;

/// Toast 的 TTL（错误 15s，其余 5s）
pub fn toast_ttl(kind: ToastKind) -> f64 {
    match kind {
        ToastKind::Err => TOAST_TTL_ERR_SECS,
        _ => TOAST_TTL_SECS,
    }
}

/// Toast kind 对应的主题色（色条/图标底/进度条同源）
pub fn toast_color(kind: ToastKind) -> gpui::Rgba {
    let t = crate::ui::theme::cur();
    match kind {
        ToastKind::Ok => t.up,
        ToastKind::Err => t.err,
        ToastKind::Warn => t.warn,
        ToastKind::Info => t.accent,
    }
}

/// 淘汰过期 toast（心跳/渲染帧调用）：返回是否仍有存活条目
pub fn prune_toasts(state: &SharedState) -> Vec<Toast> {
    let mut t = state.toasts.lock();
    t.retain(|x| x.at.elapsed().as_secs_f64() < toast_ttl(x.kind));
    t.clone()
}

/// 右下角 Toast 覆盖层（里程碑 1 简版：文字行；完整卡片样式在里程碑 2 重建：
/// kind 色条 + 圆形图标 + TTL 进度条 + 点击关闭 + 250ms 入场淡入）
pub fn draw_toasts(state: &SharedState) -> Div {
    let toasts = prune_toasts(state);
    let mut wrap = div().absolute().bottom(px(16.0)).right(px(16.0)).flex().flex_col();
    if toasts.is_empty() {
        return wrap;
    }
    for t in toasts {
        wrap = wrap.child(
            div()
                .max_w(px(TOAST_MAX_W))
                .mt(px(6.0))
                .px(px(12.0))
                .py(px(9.0))
                .bg(crate::ui::theme::cur().panel2)
                .border_1()
                .border_color(crate::ui::theme::cur().border)
                .rounded(px(crate::ui::theme::R_LG))
                .text_size(crate::ui::theme::font_md())
                .child(t.msg),
        );
    }
    wrap
}

/// 顶栏连接状态项的状态色与提示文案（视觉组件在里程碑 2 重建）：
/// 返回 (状态色, 悬停提示)
pub fn status_light_info(name: &str, st: &ConnState) -> (gpui::Rgba, String) {
    let (c, st_txt) = match st {
        ConnState::Connected => (
            crate::ui::theme::up_color(),
            crate::i18n::tr("已连接").to_string(),
        ),
        ConnState::Connecting => (
            crate::ui::theme::cur().accent,
            crate::i18n::tr("连接中").to_string(),
        ),
        ConnState::Disconnected => (
            crate::ui::theme::cur().text_dim,
            crate::i18n::tr("未连接").to_string(),
        ),
        ConnState::Failed(e) => (
            crate::ui::theme::cur().err,
            if e.is_empty() {
                crate::i18n::tr("错误").to_string()
            } else {
                format!("{}{e}", crate::i18n::tr("错误: "))
            },
        ),
    };
    let _ = name;
    (c, st_txt)
}

pub fn fmt_time(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%m-%d %H:%M:%S").to_string(),
        None => "—".into(),
    }
}

pub fn fmt_time_short(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%H:%M:%S").to_string(),
        None => "—".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_grouping() {
        assert_eq!(fmt_thousands(96000.5, 2), "96,000.50");
        assert_eq!(fmt_thousands(1234567.891, 2), "1,234,567.89");
        assert_eq!(fmt_thousands(-1234.5, 1), "-1,234.5");
        assert_eq!(fmt_thousands(42.0, 0), "42");
        assert_eq!(fmt_thousands(0.15, 4), "0.1500");
    }

    #[test]
    fn plain_formatting_no_grouping() {
        let d = rust_decimal::Decimal::from_str_exact("68432.15").unwrap();
        assert_eq!(fmt_dec_plain(&d, 2), "68432.15");
        let big = rust_decimal::Decimal::from_str_exact("1234567.891").unwrap();
        assert_eq!(fmt_dec_plain(&big, 2), "1234567.89");
        let small = rust_decimal::Decimal::from_str_exact("0.1").unwrap();
        assert_eq!(fmt_dec_plain(&small, 4), "0.1000");
    }

    #[test]
    fn decimal_formatters_never_leak_float_residue() {
        // 币昂返回的 entryPrice 原文带双精度尾数（服务端浮点残差），
        // Decimal 全精度保留但在展示层必须按 tick 精度收敛
        let d = rust_decimal::Decimal::from_str_exact("86997.7000000001").unwrap();
        assert_eq!(fmt_dec_plain(&d, 2), "86997.70");
        assert_eq!(fmt_dec_fixed(&d, 2), "86,997.70");
        // 高精度币按其 tick 保留（SHIB 类 8 位小数）：f64 中转会让
        // 第 8 位露出脏数字，纯 Decimal 路径下精确
        let shib = rust_decimal::Decimal::from_str_exact("0.000012345678901234").unwrap();
        assert_eq!(fmt_dec_plain(&shib, 8), "0.00001235");
        // 舍入而非截断
        assert_eq!(
            fmt_dec_plain(&rust_decimal::Decimal::from_str_exact("0.125").unwrap(), 2),
            "0.13"
        );
        // 大整数金额：f64 在 10^16 以上丢失精度，Decimal 不丢
        let huge = rust_decimal::Decimal::from_str_exact("12345678901234567.89").unwrap();
        assert_eq!(fmt_dec_fixed(&huge, 2), "12,345,678,901,234,567.89");
    }

    #[test]
    fn grouped_formatting_keeps_decimal_text() {
        // group_thousands 只分组不改数字：负数符号、零、纯整数均稳定
        assert_eq!(
            fmt_dec_grouped(&rust_decimal::Decimal::from_str_exact("1234567.891").unwrap()),
            "1,234,567.891"
        );
        assert_eq!(
            fmt_dec_grouped(
                &rust_decimal::Decimal::from_str_exact("123456789012345678.1").unwrap()
            ),
            "123,456,789,012,345,678.1"
        );
    }

    #[test]
    fn compact_formatting() {
        // 参数化 ZH：不读全局 LANG（任何测试都不再 set_lang，并行无竞争）
        assert_eq!(fmt_compact_lang(123_456_789.0, crate::i18n::ZH), "1.23亿");
        assert_eq!(fmt_compact_lang(34_500.0, crate::i18n::ZH), "3.5万");
        assert_eq!(fmt_compact_lang(123.0, crate::i18n::ZH), "123");
    }

    #[test]
    fn compact_formatting_english() {
        // 参数化 EN：不改全局（曾因 set_lang 与并行测试竞争导致
        // compact_formatting 偶发读到 EN 而失败）
        assert_eq!(fmt_compact_lang(1_234_567_890.0, crate::i18n::EN), "1.23B");
        assert_eq!(fmt_compact_lang(34_500.0, crate::i18n::EN), "34.5K");
        assert_eq!(fmt_compact_lang(123.0, crate::i18n::EN), "123");
    }

    #[test]
    fn compact_formatting_reads_global_default() {
        // 默认全局为 ZH 且无测试再改动它：fmt_compact 包装层稳走 ZH 分支
        assert_eq!(fmt_compact(34_500.0), "3.5万");
    }
}
