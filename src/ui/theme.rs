//! 主题与字体：明暗双主题 + Windows 系统字体（雅黑 UI / 等宽数字）
//!
//! 颜色通过 `theme::cur()` 运行时取值（`theme::cur().up` 等），主题切换即时生效。

use eframe::egui;
use egui::{Color32, FontData, FontDefinitions, Stroke, Style, Visuals};

// ---------- 字号 / 控件高度 token ----------
/// 全应用仅 5 档字号：小字（辅助小字）→ 次要（表格/说明）→ 正文 → 页面标题 → 大数字
pub const FONT_SM: f32 = 11.0;
pub const FONT_MD: f32 = 12.5;
pub const FONT_BODY: f32 = 14.0;
/// 品牌名等需要强调的展示型文字
pub const FONT_XL: f32 = 20.0;
pub const FONT_LG: f32 = 16.5;
pub const FONT_HERO: f32 = 20.0;
/// 常规按钮/页签高度
pub const BTN_H: f32 = 28.0;
/// 主操作按钮高度（买卖切换、下单提交）
pub const BTN_LG: f32 = 36.0;

// ---------- 价格变动高亮（flash）token ----------
/// flash 高亮持续时长：价格变化后行底色在此时间内渐变回原色
pub const FLASH_SECS: f32 = 0.5;
/// flash 行底色不透明度（0-1）：叠在行背景之上的涨/跌色强度
pub const FLASH_ALPHA: f32 = 0.28;
/// flash 文字提亮幅度（0-1）：无法铺行底色的区域（Grid 成交列表）
/// 用文字向主题文字色插值代替底色闪烁
pub const FLASH_BRIGHTEN: f32 = 0.5;
/// 盘口消失档位残影的淡出时长（秒）：吃单后旧档位底色在此时间内渐隐
pub const GHOST_SECS: f64 = 0.4;

// ---------- 重绘调度（request_repaint_after 间隔，毫秒）----------
/// 图表/动画档：≈30fps，K 线最新价与闪烁衰减都在此刷新
pub const REPAINT_CHART: u64 = 33;
/// 行情档：盘口深度 20 档 @100ms 推送，盘口/页头/成交随之刷新
pub const REPAINT_QUOTE: u64 = 100;
/// 空闲档：无行情变动的页面（资产/设置/帮助）保持低频重绘
pub const REPAINT_IDLE: u64 = 400;

// ---------- 响应式等比缩放 ----------
/// 设计基准逻辑尺寸：全部布局常量（面板宽/行高/字号）按此基准标定
pub const BASE_W: f32 = 1280.0;
pub const BASE_H: f32 = 720.0;
/// 等比缩放上下限：小于基准最多缩到 ×0.85，大于基准最多放大到 ×1.5
pub const SCALE_MIN: f32 = 0.85;
pub const SCALE_MAX: f32 = 1.5;

/// 按窗口物理尺寸推算全局 pixels_per_point，锁定逻辑工作区 ≈ 基准尺寸，
/// 实现"窗口变大 → 全站字号/行高/面板/图表等比变大"。返回 None 表示无需调整。
///
/// 无反馈回路：物理尺寸 = 逻辑 rect × 当前 ppi，与目标 ppi 无关；
/// 系数按 5% 量化，拖拽窗口时减少字体重排/重新光栅化次数。
pub fn auto_ppi(ctx: &egui::Context) -> Option<f32> {
    let native = ctx.native_pixels_per_point().unwrap_or(1.0);
    // 0.36 起 screen_rect 在 RawInput 里（Option<Rect>）：逻辑 rect × 当前 ppi = 物理尺寸
    let rect = ctx.input(|i| i.raw.screen_rect).unwrap_or(egui::Rect::ZERO);
    let phys = rect.size() * ctx.pixels_per_point();
    let s = (phys.x / (BASE_W * native)).min(phys.y / (BASE_H * native));
    // 向下取整而非四舍五入：zoom 变更帧 egui 会用 ratio×content_rect 重写 screen_rect，
    // 物理尺寸带亚像素回抖（1440 → 1439.9）；1.125 这类边界值用 round 会在相邻档位
    // 间漂移（1.15↔1.1），floor 对亚像素噪声稳定——恢复后回到与启动一致的档位
    let s = ((s * 20.0 + 0.001).floor() / 20.0).clamp(SCALE_MIN, SCALE_MAX);
    let target = native * s;
    ((ctx.pixels_per_point() - target).abs() > 0.01).then_some(target)
}

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
    /// 涨/跌等彩色填充（买卖按钮、最新价标签）上的前景文字
    pub on_fill: Color32,
    /// 金色 accent 填充（主按钮）上的前景文字
    pub on_accent: Color32,
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
    // 涨/跌填充上的文字用深色：白色在 up=rgb(14,203,129) 上对比度仅 2.1:1，
    // 深色对 up 8.1:1、对 down 4.9:1
    on_fill: Color32::from_rgb(6, 32, 22),
    on_accent: Color32::from_rgb(28, 22, 2),
};

pub const LIGHT: Theme = Theme {
    up: Color32::from_rgb(6, 158, 98),
    down: Color32::from_rgb(214, 58, 80),
    bg: Color32::from_rgb(244, 246, 249),
    panel: Color32::from_rgb(255, 255, 255),
    panel2: Color32::from_rgb(248, 250, 252),
    // 卡片近纯白：与灰底 bg 拉开层次，卡片"浮"在页面上
    card: Color32::from_rgb(252, 253, 255),
    border: Color32::from_rgb(216, 222, 231),
    text: Color32::from_rgb(27, 32, 38),
    // 加深一档：在近白卡片底 (252,253,255) 上对比度 ≥4.5:1
    text_dim: Color32::from_rgb(96, 106, 120),
    // accent/warn 加深：原色在白底上对比度 2.7:1 / 3.5:1，不满足 4.5:1
    accent: Color32::from_rgb(140, 102, 0),
    warn: Color32::from_rgb(146, 96, 8),
    err: Color32::from_rgb(214, 58, 80),
    grid: Color32::from_rgb(223, 228, 236),
    grid_time: Color32::from_rgb(228, 233, 239),
    crosshair: Color32::from_rgb(110, 122, 140),
    tooltip_bg: Color32::from_rgb(255, 255, 255),
    tag_bg: Color32::from_rgb(219, 225, 234),
    row_hover: Color32::from_rgb(235, 239, 245),
    btn_idle: Color32::from_rgb(226, 232, 240),
    // 彩色填充（up/down）上的文字用黑色：白色对浅色 up 仅 3.5:1；
    // 黑色对 up 6.1:1、对 down 4.6:1
    on_fill: Color32::from_rgb(0, 0, 0),
    // accent 加深后深色字对比度跌至 3.1:1，改白色（5.2:1）
    on_accent: Color32::from_rgb(255, 255, 255),
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

/// 当前主题配色（up/down 字段已按涨跌方向翻转：
/// 红涨绿跌模式下全应用所有 cur().up/down 取色点统一生效，含K线/盘口/统计）
pub fn cur() -> Theme {
    let base = if is_light() { LIGHT } else { DARK };
    if PRICE_DIR.load(std::sync::atomic::Ordering::Relaxed) == DIR_RED_UP {
        Theme {
            up: base.down,
            down: base.up,
            ..base
        }
    } else {
        base
    }
}

/// 涨跌颜色方向：红涨绿跌（A 股/币安默认）或绿涨红跌（美股习惯）
///
/// `theme::signed_color` 与所有显式取 `up/down` 的绘制点都应改走
/// `up_color()/down_color()`，切换本枚举即时全应用生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceDir {
    /// 绿涨红跌（币安默认：up=#0ECB81 涨，down=#F6465D 跌）
    GreenUp,
    /// 红涨绿跌
    RedUp,
}

impl PriceDir {
    pub fn from_cfg(s: &str) -> Self {
        if s == "red_up" {
            PriceDir::RedUp
        } else {
            PriceDir::GreenUp
        }
    }

    pub fn as_cfg(self) -> &'static str {
        match self {
            PriceDir::GreenUp => "green_up",
            PriceDir::RedUp => "red_up",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PriceDir::GreenUp => crate::i18n::tr("绿涨红跌"),
            PriceDir::RedUp => crate::i18n::tr("红涨绿跌"),
        }
    }
}

static PRICE_DIR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub const DIR_GREEN_UP: u8 = 0;
pub const DIR_RED_UP: u8 = 1;

/// 由配置字符串设置涨跌方向
pub fn set_price_dir_str(s: &str) {
    let v = if s == "red_up" {
        DIR_RED_UP
    } else {
        DIR_GREEN_UP
    };
    PRICE_DIR.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// 上涨色（cur() 已按 price_dir 翻转字段，直读即可）
pub fn up_color() -> Color32 {
    cur().up
}

/// 下跌色（cur() 已按 price_dir 翻转字段，直读即可）
pub fn down_color() -> Color32 {
    cur().down
}

/// 由涨跌幅取颜色：>0 上涨色，<0 下跌色，0 中性。
/// 替代旧的 `signed_color`（写死 up/down，不响应方向配置）
pub fn signed_color(d: f64) -> Color32 {
    if d > 0.0 {
        up_color()
    } else if d < 0.0 {
        down_color()
    } else {
        cur().text_dim
    }
}

/// 逐笔成交主动方向着色：`buyer_maker=true` 记为跌（红），
/// `false` 记为涨（绿），同样随 price_dir 翻转
pub fn trade_color(buyer_maker: bool) -> Color32 {
    if buyer_maker {
        down_color()
    } else {
        up_color()
    }
}

/// MA 均线配色：浅色主题用加深变体（原色在近白图表背景上对比度不足 3:1）
pub fn ma_colors() -> [Color32; 4] {
    if is_light() {
        [
            Color32::from_rgb(178, 122, 0),  // 琥珀（加深）
            Color32::from_rgb(186, 32, 118), // 粉（加深）
            Color32::from_rgb(0, 116, 172),  // 青（加深）
            Color32::from_rgb(104, 66, 180), // 紫（加深）
        ]
    } else {
        [
            Color32::from_rgb(255, 193, 61),
            Color32::from_rgb(240, 100, 181),
            Color32::from_rgb(80, 200, 255),
            Color32::from_rgb(170, 130, 255),
        ]
    }
}

/// 彩色小徽标
pub fn badge(ui: &mut egui::Ui, text: &str, fg: Color32) {
    let txt = egui::RichText::new(format!(" {text} "))
        .size(FONT_SM)
        .strong();
    egui::Frame::default()
        .fill(fg.gamma_multiply(0.22))
        .stroke(Stroke::new(1.0_f32, fg.gamma_multiply(0.85)))
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(txt.color(fg));
        });
}

// ---------- 按钮层级 ----------

/// 主操作按钮：全宽 + accent 填充 + on_accent 文字（保存、提交、解锁等唯一主入口）
pub fn btn_primary(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(
        egui::RichText::new(text)
            .strong()
            .size(FONT_BODY)
            .color(cur().on_accent),
    )
    .fill(cur().accent)
    .corner_radius(5)
    .min_size(egui::vec2(ui.available_width(), BTN_LG));
    ui.add_enabled(enabled, btn)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 普通按钮：card 底 + 边框，中性文字
pub fn btn_default(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(egui::RichText::new(text).size(FONT_BODY))
        .fill(cur().card)
        .stroke(Stroke::new(1.0_f32, cur().border))
        .corner_radius(6)
        .min_size(egui::vec2(0.0, BTN_H));
    ui.add_enabled(enabled, btn)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 次按钮：全宽、金色描边 —— 认证页的替代路径（跳过 / 仅看行情）。
/// small_button 的默认底色与卡片底色几乎相同，辨识度太差
pub fn btn_secondary(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(egui::RichText::new(text).size(FONT_MD).color(cur().text))
        .fill(cur().panel2)
        .stroke(Stroke::new(1.0_f32, cur().accent.gamma_multiply(0.55)))
        .corner_radius(5)
        .min_size(egui::vec2(ui.available_width(), BTN_LG));
    ui.add_enabled(enabled, btn)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 危险按钮：红色淡填充 + 红字（撤销/平仓/删除凭据库等破坏性操作）
pub fn btn_danger(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    // 用半透明红叠加：gamma_multiply 在浅色主题下会变成近黑色块
    // （egui 0.36 移除了 ecolor::lerp_color；自透明向 c 以 t 渐显 ≈ 全通道乘 t）
    let fill = cur().err.gamma_multiply(0.16);
    let btn = egui::Button::new(egui::RichText::new(text).size(FONT_BODY).color(cur().err))
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, cur().err.gamma_multiply(0.55)))
        .corner_radius(6)
        .min_size(egui::vec2(0.0, BTN_H));
    ui.add_enabled(enabled, btn)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 语义实心按钮（横排、自适应宽）：涨/跌色实心填充 + on_fill 文字。
/// 用于确认弹窗的提交键等"彩色即语义"的场合（btn_primary 是全宽金色，场景不同）
pub fn btn_solid(ui: &mut egui::Ui, text: &str, fill: Color32, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(
        egui::RichText::new(text)
            .strong()
            .size(FONT_BODY)
            .color(cur().on_fill),
    )
    .fill(fill)
    .corner_radius(6)
    .min_size(egui::vec2(0.0, BTN_H));
    ui.add_enabled(enabled, btn)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 固定字号的小加载圈（统一 13px，避免调用点出现裸 .size()）
pub fn spinner(ui: &mut egui::Ui) {
    ui.add(egui::Spinner::new().size(FONT_BODY));
}

/// 胶囊页签：激活态 accent 淡填充 + accent 文字，未激活透明 + 暗文字，
/// 填充带淡入动画（hover/切换只触发一次动画，不常驻重绘）
pub fn pill_tab(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let t = cur();
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(FONT_BODY),
        if active { t.accent } else { t.text_dim },
    );
    let pad_x = 12.0;
    let size = egui::vec2(galley.size().x + pad_x * 2.0, BTN_H);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let anim = ui
        .ctx()
        .animate_bool(egui::Id::new("pill_tab").with(label), active);
    // 半透明 accent 叠加：深浅两主题下都是"淡淡的 accent 色块"
    let mut fill = t.accent.gamma_multiply(0.16 * anim);
    if resp.hovered() && !active {
        fill = t.row_hover;
    }
    ui.painter().rect_filled(rect, 6.0, fill);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        if active { t.accent } else { t.text_dim },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
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

    // 等宽数字字体：Cascadia（Win11）→ Consolas（Win10 均有）。
    // mono 优先：Proportional 首位命中拉丁/数字（交易终端数字对齐是第一优先级），
    // CJK 与 emoji 逐字回退到后续字体
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
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            fam.insert(0, "mono".to_owned());
        }
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            fam.insert(0, "mono".to_owned());
        }
    }
    // UI 中文字体：微软雅黑，只作回退（拉丁/数字已被 mono 命中，中文才落到这里）
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
            fam.push("cjk".to_owned());
        }
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
        (
            egui::TextStyle::Heading,
            egui::FontId::proportional(FONT_HERO),
        ),
        (egui::TextStyle::Body, egui::FontId::proportional(FONT_BODY)),
        (
            egui::TextStyle::Button,
            egui::FontId::proportional(FONT_BODY),
        ),
        (egui::TextStyle::Small, egui::FontId::proportional(FONT_SM)),
        (
            egui::TextStyle::Monospace,
            egui::FontId::monospace(FONT_BODY),
        ),
    ]
    .into();
    // 0.35+ 按主题分别持有 Style；本应用自带明暗配色（theme::cur()），两个主题统一用同一份
    let (dark, light) = (style.clone(), style);
    ctx.set_style_of(egui::Theme::Dark, dark);
    ctx.set_style_of(egui::Theme::Light, light);
    // 缩放职责已由 auto_ppi（响应式等比）接管：关闭 egui 内置 Ctrl+加减号缩放，
    // 否则用户手动缩放会在下一帧被 auto_ppi 静默回退
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
}

// ---------- 组件辅助 ----------

/// 卡片容器：圆角 + 细边框 + 内边距
pub fn card<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::default()
        .fill(cur().card)
        // 边框压暗一档：卡片轮廓更实，与面板底色拉开层次
        .stroke(Stroke::new(1.0_f32, cur().border.gamma_multiply(0.7)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .outer_margin(egui::Margin::symmetric(0, 4))
        .show(ui, add_contents)
}

/// 带标题行的卡片：标题 + 可选右侧操作槽 + 分隔线 + 内容体
pub fn card_with_header<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_right: impl FnOnce(&mut egui::Ui),
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(FONT_LG)
                    .strong()
                    .color(cur().text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_right);
        });
        ui.separator();
        add_contents(ui)
    })
}

// ---------- 表单行 ----------

/// 表单行内输入控件（TextEdit/DragValue）统一最小高
pub const CONTROL_H: f32 = 26.0;
/// 表单行标准高度（≥ 控件高，留少量呼吸空间）
pub const ROW_H: f32 = 30.0;

/// 固定高度表单行。
///
/// `ui.horizontal` 的行高初始只有 `interact_size.y`（18px），行内后置的高控件
/// （ComboBox/按钮 ≈28px）会把行撑高，而先放置的标签不会随之重新居中，
/// 造成"标签偏高、控件偏矮"的逐项错位。这里预分配 ROW_H 并统一控件高度，
/// 行内所有元素以同一基线垂直居中，行高不再中途变化。
pub fn form_row<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let w = ui.available_width();
    ui.allocate_ui(egui::vec2(w, ROW_H), |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            // 按钮高度 = 文本 + 2×button_padding.y（下拉框同理），压到 4 使其
            // 与 interact_size.y 撑起的输入控件同为 CONTROL_H
            ui.spacing_mut().button_padding.y = 4.0;
            ui.spacing_mut().interact_size.y = CONTROL_H;
            add_contents(ui)
        })
    })
    .inner
}

/// 表单行标签：固定宽度保证后续控件左缘对齐，行高保证文字垂直居中
pub fn row_label(ui: &mut egui::Ui, text: impl Into<egui::RichText>, w: f32) {
    ui.allocate_ui(egui::vec2(w, ROW_H), |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.label(text.into());
        });
    });
}

/// 顶部导航页签（胶囊样式，激活态 accent 填充）
pub fn nav_tab(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    pill_tab(ui, label, active).clicked()
}

/// 区块小标题
pub fn section(ui: &mut egui::Ui, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .size(FONT_SM)
            .strong()
            .color(cur().text_dim),
    );
}

/// 表格/卡片内的小标签文字
pub fn card_label(label: &str) -> egui::RichText {
    egui::RichText::new(label.to_string())
        .size(FONT_SM)
        .color(cur().text_dim)
}

/// 由配置字符串设置主题模式
pub fn set_mode_str(s: &str) {
    set_mode(if s == "light" { LIGHT_MODE } else { DARK_MODE });
}
