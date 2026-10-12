//! 通用小控件：状态灯、数字输入、Toast 提示、格式化工具

use crate::engine::state::{ConnState, SharedState, Toast, ToastKind};
use crate::ui::theme;
use chrono::{Local, TimeZone};
use eframe::egui;
use egui::{Align2, Color32, FontId, Id, Stroke};
use rust_decimal::Decimal;

pub fn status_dot(ui: &mut egui::Ui, st: &ConnState) {
    let (c, label) = match st {
        ConnState::Connected => (theme::cur().up, crate::i18n::tr("已连接").to_string()),
        ConnState::Connecting => (theme::cur().accent, crate::i18n::tr("连接中").to_string()),
        ConnState::Disconnected => (
            Color32::from_rgb(90, 99, 112),
            crate::i18n::tr("未连接").to_string(),
        ),
        ConnState::Failed(e) => (
            theme::cur().err,
            format!("{}{e}", crate::i18n::tr("错误: ")),
        ),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    let p = ui.painter();
    p.circle_filled(rect.center(), 4.5, c);
    // 已连接时加一圈微光晕
    if *st == ConnState::Connected {
        p.circle_stroke(
            rect.center(),
            6.5,
            Stroke::new(1.0_f32, c.gamma_multiply(0.35)),
        );
    }
    ui.label(
        egui::RichText::new(label)
            .size(11.5)
            .color(theme::cur().text_dim),
    );
}

/// 数字输入框：等宽字体 + 提示
pub fn num_field(ui: &mut egui::Ui, id: &str, text: &mut String, hint: &str) {
    egui::TextEdit::singleline(text)
        .id_salt(Id::new(id))
        .desired_width(120.0)
        .font(egui::TextStyle::Monospace)
        .text_color(theme::cur().text)
        .hint_text(hint)
        .show(ui);
}

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

/// 千分位分组格式化（用于展示层；精确值仍走 Decimal）
pub fn fmt_thousands(v: f64, dp: usize) -> String {
    let s = format!("{:.*}", dp, v);
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s.as_str(), None),
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
    let a = v.abs();
    if crate::i18n::lang() == crate::i18n::EN {
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

/// 右下角 Toast 提示（未过期的持续显示，过期自动移除）
pub fn draw_toasts(ctx: &egui::Context, state: &SharedState) {
    let toasts: Vec<Toast> = {
        let mut t = state.toasts.lock();
        // 错误类保留更久：下单被拒/断线等提示 5 秒即消失极易错过
        t.retain(|x| {
            let ttl = match x.kind {
                ToastKind::Err => 15.0,
                _ => 5.0,
            };
            x.at.elapsed().as_secs_f64() < ttl
        });
        t.clone()
    };
    if toasts.is_empty() {
        return;
    }
    egui::Area::new(Id::new("toasts_area"))
        .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::RIGHT), |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for t in toasts {
                    let (fg, icon) = match t.kind {
                        ToastKind::Ok => (theme::cur().up, "✓"),
                        ToastKind::Err => (theme::cur().err, "✗"),
                        ToastKind::Warn => (theme::cur().warn, "!"),
                        ToastKind::Info => (theme::cur().accent, "i"),
                    };
                    egui::Frame::default()
                        .fill(theme::cur().panel2)
                        .stroke(Stroke::new(1.0_f32, fg.gamma_multiply(0.8)))
                        .rounding(6.0)
                        .inner_margin(egui::Margin::symmetric(12.0, 9.0))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new(icon).color(fg).strong());
                                ui.label(egui::RichText::new(t.msg).color(theme::cur().text));
                            });
                        });
                }
            });
        });
}

/// 去除多余的十进制尾零后展示
pub fn fmt_dec(d: &Decimal) -> String {
    d.normalize().to_string()
}

pub fn fmt_time(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%m-%d %H:%M:%S").to_string(),
        None => "—".into(),
    }
}

pub fn fmt_time_short(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%H:%M:%S").to_string(),
        None => "—".into(),
    }
}

/// 空态/加载占位文本
pub fn placeholder(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(15.0),
        theme::cur().text_dim,
    );
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
    fn compact_formatting() {
        assert_eq!(fmt_compact(123_456_789.0), "1.23亿");
        assert_eq!(fmt_compact(34_500.0), "3.5万");
        assert_eq!(fmt_compact(123.0), "123");
    }

    #[test]
    fn compact_formatting_english() {
        // 保存并恢复语言：测试并行运行，compact_formatting 依赖默认 ZH
        let saved = crate::i18n::lang();
        crate::i18n::set_lang(crate::i18n::EN);
        assert_eq!(fmt_compact(1_234_567_890.0), "1.23B");
        assert_eq!(fmt_compact(34_500.0), "34.5K");
        assert_eq!(fmt_compact(123.0), "123");
        crate::i18n::set_lang(saved);
    }
}
