//! 通用小控件：状态灯、数字输入、Toast 提示、格式化工具

use crate::engine::state::{ConnState, SharedState, Toast, ToastKind};
use crate::ui::theme;
use chrono::{Local, TimeZone};
use eframe::egui;
use egui::{Align2, Color32, FontId, Id, Stroke};
use rust_decimal::Decimal;

pub fn status_dot(ui: &mut egui::Ui, st: &ConnState) {
    let (c, label) = match st {
        ConnState::Connected => (theme::UP, "已连接".to_string()),
        ConnState::Connecting => (theme::ACCENT, "连接中".to_string()),
        ConnState::Disconnected => (Color32::from_rgb(90, 99, 112), "未连接".to_string()),
        ConnState::Failed(e) => (theme::ERR, format!("错误: {e}")),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, c);
    ui.label(egui::RichText::new(label).size(11.5).color(theme::TEXT_DIM));
}

/// 数字输入框；返回当前合法解析值
pub fn num_field(ui: &mut egui::Ui, id: &str, text: &mut String, hint: &str) -> Option<Decimal> {
    egui::TextEdit::singleline(text)
        .id_salt(Id::new(id))
        .desired_width(120.0)
        .font(egui::TextStyle::Monospace)
        .hint_text(hint)
        .show(ui);
    parse_dec(text)
}

pub fn parse_dec(s: &str) -> Option<Decimal> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    Decimal::from_str_exact(t).ok()
}

/// 右下角 Toast 提示（未过期的持续显示，过期自动移除）
pub fn draw_toasts(ctx: &egui::Context, state: &SharedState) {
    let toasts: Vec<Toast> = {
        let mut t = state.toasts.lock();
        t.retain(|x| x.at.elapsed().as_secs_f64() < 5.0);
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
                        ToastKind::Ok => (theme::UP, "✓"),
                        ToastKind::Err => (theme::ERR, "✗"),
                        ToastKind::Warn => (theme::WARN, "!"),
                        ToastKind::Info => (theme::ACCENT, "i"),
                    };
                    egui::Frame::default()
                        .fill(theme::PANEL2)
                        .stroke(Stroke::new(1.0_f32, fg))
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new(icon).color(fg).strong());
                                ui.label(egui::RichText::new(t.msg).color(theme::TEXT));
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
        theme::TEXT_DIM,
    );
}
