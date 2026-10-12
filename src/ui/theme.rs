//! 主题与字体：明暗双主题 + Windows 系统字体（雅黑 UI / 等宽数字）
//!
//! 颜色通过 `theme::cur()` 运行时取值（`theme::cur().up` 等），主题切换即时生效。

use eframe::egui;
use egui::{Color32, FontData, FontDefinitions, Stroke, Style, Visuals};

// ---------- 主题定义 ----------

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub up: Color32,
    pub down: Color32,
    pub bg: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub card: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub warn: Color32,
    pub err: Color32,
    /// 图表横向网格线
    pub grid: Color32,
    /// 图表纵向时间网格线
    pub grid_time: Color32,
    /// 十字光标
    pub crosshair: Color32,
    /// 图表悬浮信息框背景
    pub tooltip_bg: Color32,
    /// 轴标签/时间标签底色
    pub tag_bg: Color32,
    /// 列表行悬停高亮
    pub row_hover: Color32,
    /// 大按钮未选中态底色（买/卖切换、页签等）
    pub btn_idle: Color32,
}

pub const DARK: Theme = Theme {
    up: Color32::from_rgb(14, 203, 129),
    down: Color32::from_rgb(246, 70, 93),
    bg: Color32::from_rgb(10, 13, 18),
    panel: Color32::from_rgb(18, 22, 30),
    panel2: Color32::from_rgb(25, 31, 42),
    card: Color32::from_rgb(31, 38, 50),
    border: Color32::from_rgb(44, 53, 66),
    text: Color32::from_rgb(232, 234, 237),
    text_dim: Color32::from_rgb(139, 149, 165),
    accent: Color32::from_rgb(240, 185, 11),
    warn: Color32::from_rgb(240, 160, 48),
    err: Color32::from_rgb(246, 70, 93),
    grid: Color32::from_rgb(28, 34, 44),
    grid_time: Color32::from_rgb(26, 32, 42),
    crosshair: Color32::from_rgb(120, 130, 148),
    tooltip_bg: Color32::from_rgb(24, 30, 41),
    tag_bg: Color32::from_rgb(44, 52, 66),
    row_hover: Color32::from_rgb(38, 46, 60),
    btn_idle: Color32::from_rgb(30, 37, 48),
};

pub const LIGHT: Theme = Theme {
    up: Color32::from_rgb(6, 158, 98),
    down: Color32::from_rgb(214, 58, 80),
    bg: Color32::from_rgb(244, 246, 249),
    panel: Color32::from_rgb(255, 255, 255),
    panel2: Color32::from_rgb(248, 250, 252),
    card: Color32::from_rgb(239, 242, 247),
    border: Color32::from_rgb(216, 222, 231),
    text: Color32::from_rgb(27, 32, 38),
    text_dim: Color32::from_rgb(107, 118, 132),
    accent: Color32::from_rgb(201, 148, 0),
    warn: Color32::from_rgb(185, 122, 20),
    err: Color32::from_rgb(214, 58, 80),
    grid: Color32::from_rgb(223, 228, 236),
    grid_time: Color32::from_rgb(228, 233, 239),
    crosshair: Color32::from_rgb(110, 122, 140),
    tooltip_bg: Color32::from_rgb(255, 255, 255),
    tag_bg: Color32::from_rgb(219, 225, 234),
    row_hover: Color32::from_rgb(235, 239, 245),
    btn_idle: Color32::from_rgb(226, 232, 240),
};

static MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub const DARK_MODE: u8 = 0;
pub const LIGHT_MODE: u8 = 1;

pub fn set_mode(mode: u8) {
    MODE.store(mode, std::sync::atomic::Ordering::Relaxed);
}

pub fn mode() -> u8 {
    MODE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn is_light() -> bool {
    mode() == LIGHT_MODE
}

/// 当前主题配色
pub fn cur() -> &'static Theme {
    if is_light() {
        &LIGHT
    } else {
        &DARK
    }
}

pub fn signed_color(d: f64) -> Color32 {
    let t = cur();
    if d > 0.0 {
        t.up
    } else if d < 0.0 {
        t.down
    } else {
        t.text_dim
    }
}

/// 彩色小徽标
pub fn badge(ui: &mut egui::Ui, text: &str, fg: Color32) {
    let txt = egui::RichText::new(format!(" {text} ")).size(11.5).strong();
    egui::Frame::default()
        .fill(fg.gamma_multiply(0.22))
        .stroke(Stroke::new(1.0_f32, fg.gamma_multiply(0.85)))
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(txt.color(fg));
        });
}

// ---------- 字体 ----------

/// 从 Windows 字体目录按优先级加载第一个存在的字体
fn load_font(fonts: &mut FontDefinitions, key: &str, candidates: &[&str]) -> bool {
    for name in candidates {
        let path = std::path::PathBuf::from("C:\\Windows\\Fonts").join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            tracing::info!("load font {}: {}", key, path.display());
            fonts.font_data.insert(
                key.to_owned(),
                std::sync::Arc::new(FontData::from_owned(bytes)),
            );
            return true;
        }
    }
    false
}

pub fn apply_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    // UI 主字体：微软雅黑（拉丁+CJK 统一渲染），放最前优先命中
    if load_font(
        &mut fonts,
        "cjk",
        &[
            "msyh.ttc",
            "msyh.ttf",
            "msyhbd.ttc",
            "simhei.ttf",
            "Deng.ttf",
        ],
    ) {
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            fam.insert(0, "cjk".to_owned());
        }
    }
    // 等宽数字字体：Cascadia（Win11）→ Consolas（Win10 均有），中文回退雅黑
    if load_font(
        &mut fonts,
        "mono",
        &[
            "CascadiaCode.ttf",
            "CascadiaMono.ttf",
            "consola.ttf",
            "lucon.ttf",
        ],
    ) {
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            fam.insert(0, "mono".to_owned());
        }
    }
    if fonts.font_data.contains_key("cjk") {
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            fam.push("cjk".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

// ---------- 全局样式（随主题） ----------

pub fn apply(ctx: &egui::Context) {
    let t = cur();
    let mut style = Style {
        visuals: Visuals::dark(),
        ..Default::default()
    };
    let v = &mut style.visuals;
    v.dark_mode = !is_light();
    v.override_text_color = Some(t.text);
    v.panel_fill = t.panel;
    v.window_fill = t.panel2;
    v.extreme_bg_color = t.bg;
    v.faint_bg_color = if is_light() {
        Color32::from_rgb(236, 240, 246)
    } else {
        Color32::from_rgb(24, 30, 40)
    };
    v.window_corner_radius = 6.into();
    v.menu_corner_radius = 4.into();
    v.window_stroke = Stroke::new(1.0_f32, t.border);
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;

    let hover_bg = if is_light() {
        Color32::from_rgb(226, 232, 241)
    } else {
        Color32::from_rgb(38, 46, 60)
    };
    let active_bg = if is_light() {
        Color32::from_rgb(214, 222, 234)
    } else {
        Color32::from_rgb(46, 56, 72)
    };
    let widget_bg = if is_light() {
        Color32::from_rgb(255, 255, 255)
    } else {
        Color32::from_rgb(30, 37, 48)
    };
    let hover_border = if is_light() {
        Color32::from_rgb(176, 187, 202)
    } else {
        Color32::from_rgb(70, 82, 100)
    };

    v.widgets.noninteractive.bg_fill = t.panel2;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text);
    v.widgets.noninteractive.corner_radius = 4.into();

    v.widgets.inactive.bg_fill = widget_bg;
    v.widgets.inactive.weak_bg_fill = widget_bg;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, t.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, t.text_dim);
    v.widgets.inactive.corner_radius = 4.into();

    v.widgets.hovered.bg_fill = hover_bg;
    v.widgets.hovered.weak_bg_fill = hover_bg;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, hover_border);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, t.text);
    v.widgets.hovered.corner_radius = 4.into();

    v.widgets.active.bg_fill = active_bg;
    v.widgets.active.weak_bg_fill = active_bg;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, t.text);
    v.widgets.active.corner_radius = 4.into();

    v.selection.bg_fill = if is_light() {
        Color32::from_rgb(191, 214, 241)
    } else {
        Color32::from_rgb(42, 75, 117)
    };
    v.selection.stroke = Stroke::NONE;
    v.hyperlink_color = t.accent;

    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(12.0, 5.0);
    style.spacing.menu_margin = egui::Margin::symmetric(6, 4);
    style.spacing.combo_width = 90.0;
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::proportional(19.0)),
        (egui::TextStyle::Body, egui::FontId::proportional(14.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(14.0)),
        (egui::TextStyle::Small, egui::FontId::proportional(11.5)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(13.5)),
    ]
    .into();
    // 0.35+ 按主题分别持有 Style；本应用自带明暗配色（theme::cur()），两个主题统一用同一份
    let (dark, light) = (style.clone(), style);
    ctx.set_style_of(egui::Theme::Dark, dark);
    ctx.set_style_of(egui::Theme::Light, light);
}

// ---------- 组件辅助 ----------

/// 卡片容器：圆角 + 细边框 + 内边距
pub fn card<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::default()
        .fill(cur().card)
        .stroke(Stroke::new(1.0_f32, cur().border))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .outer_margin(egui::Margin::symmetric(0, 4))
        .show(ui, add_contents)
}

/// 顶部导航页签（激活态金色文字 + 边框）
pub fn nav_tab(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    let txt = if active {
        egui::RichText::new(label)
            .strong()
            .size(14.5)
            .color(cur().accent)
    } else {
        egui::RichText::new(label).size(14.0).color(cur().text_dim)
    };
    let btn = egui::Button::new(txt)
        .fill(Color32::TRANSPARENT)
        .stroke(if active {
            Stroke::new(2.0_f32, cur().accent)
        } else {
            Stroke::NONE
        })
        .corner_radius(3)
        .min_size(egui::vec2(0.0, 28.0));
    ui.add(btn).clicked()
}

/// 区块小标题
pub fn section(ui: &mut egui::Ui, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .size(11.5)
            .strong()
            .color(cur().text_dim),
    );
}

/// 表格/卡片内的小标签文字
pub fn card_label(label: &str) -> egui::RichText {
    egui::RichText::new(label.to_string())
        .size(11.5)
        .color(cur().text_dim)
}

/// 由配置字符串设置主题模式
pub fn set_mode_str(s: &str) {
    set_mode(if s == "light" { LIGHT_MODE } else { DARK_MODE });
}
