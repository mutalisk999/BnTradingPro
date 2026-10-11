//! 主题与字体：暗色专业配色 + 中文字体加载

use eframe::egui;
use egui::{Color32, FontData, FontDefinitions, Style, Visuals};

pub const UP: Color32 = Color32::from_rgb(46, 189, 133); // 涨/买（国际惯例绿涨红跌）
pub const DOWN: Color32 = Color32::from_rgb(246, 70, 93); // 跌/卖
pub const BG: Color32 = Color32::from_rgb(13, 17, 23);
pub const PANEL: Color32 = Color32::from_rgb(19, 24, 33);
pub const PANEL2: Color32 = Color32::from_rgb(25, 31, 42);
pub const BORDER: Color32 = Color32::from_rgb(44, 52, 66);
pub const TEXT: Color32 = Color32::from_rgb(222, 226, 232);
pub const TEXT_DIM: Color32 = Color32::from_rgb(132, 142, 156);
pub const ACCENT: Color32 = Color32::from_rgb(240, 185, 11); // 品牌黄
pub const WARN: Color32 = Color32::from_rgb(240, 150, 30);
pub const ERR: Color32 = DOWN;

pub fn signed_color(d: f64) -> Color32 {
    if d > 0.0 {
        UP
    } else if d < 0.0 {
        DOWN
    } else {
        TEXT_DIM
    }
}

/// 彩色小徽标
pub fn badge(ui: &mut egui::Ui, text: &str, fg: Color32) {
    let txt = egui::RichText::new(format!(" {text} ")).size(11.5).strong();
    egui::Frame::default()
        .fill(fg.gamma_multiply(0.25))
        .stroke(egui::Stroke::new(1.0_f32, fg))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(txt.color(fg));
        });
}

pub fn apply_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // Windows 常见中文字体，逐一尝试
    let candidates = [
        "msyh.ttc",
        "msyh.ttf",
        "msyhbd.ttc",
        "simhei.ttf",
        "simsun.ttc",
        "Deng.ttf",
    ];
    'outer: for name in candidates {
        let path = std::path::PathBuf::from("C:\\Windows\\Fonts").join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            tracing::info!("加载中文字体: {}", path.display());
            fonts
                .font_data
                .insert("cjk".to_owned(), FontData::from_owned(bytes));
            if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                fam.push("cjk".to_owned());
            }
            if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                fam.push("cjk".to_owned());
            }
            break 'outer;
        }
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    let mut style = Style {
        visuals: Visuals::dark(),
        ..Default::default()
    };
    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = PANEL2;
    v.extreme_bg_color = BG;
    v.faint_bg_color = Color32::from_rgb(23, 29, 39);
    v.window_stroke = egui::Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_fill = PANEL2;
    v.widgets.inactive.weak_bg_fill = PANEL2;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT_DIM);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(33, 41, 56);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(38, 47, 63);
    v.selection.bg_fill = Color32::from_rgb(52, 86, 128);
    v.hyperlink_color = ACCENT;

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::proportional(20.0)),
        (egui::TextStyle::Body, egui::FontId::proportional(14.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(14.0)),
        (egui::TextStyle::Small, egui::FontId::proportional(11.5)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(13.0)),
    ]
    .into();
    ctx.set_style(style);
}
