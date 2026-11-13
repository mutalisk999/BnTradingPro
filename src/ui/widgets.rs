//! 通用小控件：按钮层级、胶囊页签、徽标、状态灯、卡片、表单行、Toast、
//! 空态/加载态、格式化工具
//!
//! 视觉规格 1:1 对应旧 egui 版 widget 系统（btn_* 族 / pill_tab 族 / badge /
//! card / form_row / status_item / toast），布局语义从「egui Ui 分配」换成
//! 「GPUI flex 构建器」：组件函数返回预样式化的元素，调用方链 `.on_click`
//! 等交互（需要 id 的可点组件以 id 为首参，避免兄弟节点 id 冲突）。

use crate::engine::state::{ConnState, SharedState, Toast, ToastKind};
use gpui::{
    div, hsla, point, prelude::FluentBuilder, px, relative, Animation, AnimationExt, AnyElement,
    AnyView, App, AppContext, BoxShadow, Context, Div, ElementId, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window,
};
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::Duration;

// ---------- 纯格式化函数（无 UI 依赖） ----------

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

// ---------- 主题/样式小工具 ----------

use crate::ui::theme::{
    self, cur, faded, font_body, font_xl, font_lg, font_md, font_sm, is_light, lerp_color, cur as _t,
};

/// hover 反馈色：向主题文字色插值一档。浅色主题下文字是深墨（压暗），
/// 深色主题下文字是近白（提亮）——方向随主题自适应，等价 egui 默认
/// widgets.hover 的反馈方向
fn hover_tint(c: Rgba) -> Hsla {
    Hsla::from(lerp_color(c, cur().text, 0.10))
}

// ---------- 悬停提示 ----------

/// 简单文字 tooltip 视图
struct TextTooltip {
    text: String,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = cur();
        div()
            .px(px(theme::SPACE_LG))
            .py(px(theme::SPACE_SM))
            .max_w(px(360.0))
            .bg(t.panel2)
            .border_1()
            .border_color(t.border)
            .rounded(px(crate::ui::theme::R_SM))
            .text_size(font_sm())
            .text_color(t.text)
            .child(self.text.clone())
    }
}

/// 构造 tooltip 视图（gpui 的 tooltip 回调返回 AnyView）
pub fn tooltip_view(text: String, cx: &mut App) -> AnyView {
    AnyView::from(cx.new(|_| TextTooltip { text }))
}

/// 给可交互元素挂文字提示（tooltip 需要 stateful 元素，故限定 trait）
pub fn with_tooltip<E: StatefulInteractiveElement>(el: E, text: String) -> E {
    el.tooltip(move |_, cx| tooltip_view(text.clone(), cx))
}

// ---------- 按钮层级 ----------
//
// 禁用态统一 opacity 0.5（egui add_enabled 的 inactive 视觉近似）；
// 可点组件以 id 为首参（gpui 兄弟节点要求 id 唯一，由调用方保证），
// 返回 Stateful<Div>，调用方链 .on_click(...)。

/// 主操作按钮：全宽 + accent 填充 + on_accent 加粗文字（保存、提交、解锁等
/// 唯一主入口）。禁用原因由调用方在 on_click 里自行判断
pub fn btn_primary(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let t = cur();
    let base = div()
        .id(id)
        .w_full()
        .h(px(crate::ui::theme::BTN_LG))
        .rounded(px(crate::ui::theme::R_MD))
        .flex()
        .items_center()
        .justify_center()
        .bg(t.accent)
        .text_size(font_body())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.on_accent)
        .whitespace_nowrap()
        .child(text.into());
    finish_btn(base, enabled)
}

/// 普通按钮：card 底 + 中性文字，hover 行高亮
pub fn btn_default(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let t = cur();
    let base = div()
        .id(id)
        .h(px(crate::ui::theme::BTN_H))
        .px(px(theme::SPACE_XL))
        .rounded(px(crate::ui::theme::R_MD))
        .flex()
        .items_center()
        .justify_center()
        .bg(t.card)
        .text_size(font_body())
        .text_color(t.text)
        .whitespace_nowrap()
        .when(enabled, |d| d.hover(|s| s.bg(t.row_hover)))
        .child(text.into());
    finish_btn(base, enabled)
}

/// 普通按钮（指定最小宽度）：表格行内按钮需要比文字更宽的点击区时用
/// （如持仓行的止盈止损按钮，双语长度不一且点击频率高）
pub fn btn_default_w(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    min_w: f32,
    enabled: bool,
) -> Stateful<Div> {
    let t = cur();
    let base = div()
        .id(id)
        .min_w(px(min_w))
        .h(px(crate::ui::theme::BTN_H))
        .px(px(theme::SPACE_XL))
        .rounded(px(crate::ui::theme::R_MD))
        .flex()
        .items_center()
        .justify_center()
        .bg(t.card)
        .text_size(font_body())
        .text_color(t.text)
        .whitespace_nowrap()
        .when(enabled, |d| d.hover(|s| s.bg(t.row_hover)))
        .child(text.into());
    finish_btn(base, enabled)
}

/// 次按钮：全宽、金色描边 —— 认证页的替代路径（跳过 / 仅看行情）。
/// 字号与 btn_primary 同为 font_body()：两者同为 BTN_LG 高的主/次一对
pub fn btn_secondary(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let t = cur();
    let base = div()
        .id(id)
        .w_full()
        .h(px(crate::ui::theme::BTN_LG))
        .rounded(px(crate::ui::theme::R_MD))
        .flex()
        .items_center()
        .justify_center()
        .bg(t.panel2)
        .border_1()
        .border_color(faded(t.accent, 0.55))
        .text_size(font_body())
        .text_color(t.text)
        .whitespace_nowrap()
        .when(enabled, |d| d.hover(|s| s.bg(t.row_hover)))
        .child(text.into());
    finish_btn(base, enabled)
}

/// 危险按钮：红色淡填充 + 红字 + 红描边（撤销/平仓/删除凭据库等破坏性操作）
pub fn btn_danger(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let t = cur();
    let base = div()
        .id(id)
        .h(px(crate::ui::theme::BTN_H))
        .px(px(theme::SPACE_XL))
        .rounded(px(crate::ui::theme::R_MD))
        .flex()
        .items_center()
        .justify_center()
        .bg(faded(t.err, 0.16))
        .border_1()
        .border_color(faded(t.err, 0.55))
        .text_size(font_body())
        .text_color(t.err)
        .whitespace_nowrap()
        .when(enabled, |d| d.hover(|s| s.bg(hover_tint(t.err))))
        .child(text.into());
    finish_btn(base, enabled)
}

/// 语义实心按钮（横排、自适应宽）：涨/跌色实心填充 + on_fill 文字。
/// 用于确认弹窗的提交键等"彩色即语义"的场合（btn_primary 是全宽金色，场景不同）
pub fn btn_solid(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    fill: Rgba,
    enabled: bool,
) -> Stateful<Div> {
    let t = cur();
    let base = div()
        .id(id)
        .h(px(crate::ui::theme::BTN_H))
        .px(px(theme::SPACE_XL))
        .rounded(px(crate::ui::theme::R_MD))
        .flex()
        .items_center()
        .justify_center()
        .bg(fill)
        .text_size(font_body())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.on_fill)
        .whitespace_nowrap()
        .when(enabled, |d| d.hover(|s| s.bg(hover_tint(fill))))
        .child(text.into());
    finish_btn(base, enabled)
}

/// 幽灵小按钮：透明底 + 细边框 + 小字，行内轻量操作
/// （比例填充 / 买一卖一 / 设定等）
pub fn btn_ghost(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let t = cur();
    let txt_c = if enabled { t.text } else { t.text_dim };
    let base = div()
        .id(id)
        .h(px(22.0))
        .px(px(theme::SPACE_LG))
        .rounded(px(crate::ui::theme::R_SM))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(t.border)
        .text_size(font_sm())
        .text_color(txt_c)
        .whitespace_nowrap()
        .when(enabled, |d| {
            d.hover(move |s| s.bg(t.row_hover).border_color(t.text_dim))
        })
        .child(text.into());
    finish_btn(base, enabled)
}

/// 按钮公共收尾：禁用降透明度，可用时手型光标 + 按压反馈
/// （gpui 无 hover transition，按压瞬时降透明度作反馈）
fn finish_btn(base: Stateful<gpui::Div>, enabled: bool) -> Stateful<gpui::Div> {
    base.when(!enabled, |d| d.opacity(0.5))
        .when(enabled, |d| d.cursor_pointer().active(|s| s.opacity(0.85)))
}

// ---------- 胶囊页签 ----------

/// 胶囊页签：激活态 accent 淡填充 + accent 文字，未激活透明 + 暗文字。
/// （egui 版激活填充有 120ms 淡入动画；GPUI 的 with_animation 会吞掉
/// 返回值上的 on_click 链式调用，此处改为静态填充 + hover 反馈）
pub fn pill_tab(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let t = cur();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(theme::SPACE_XL))
        .h(px(crate::ui::theme::BTN_H))
        .rounded(px(crate::ui::theme::R_MD))
        .text_size(font_body())
        .text_color(if active { t.accent } else { t.text_dim })
        .whitespace_nowrap()
        .cursor_pointer()
        // 无条件带 1px 描边：选中 accent 描边、未选中透明，选中态有
        // 轮廓且两种状态间无布局抖动
        .border_1()
        .border_color(if active {
            faded(t.accent, theme::TINT_SOFT)
        } else {
            gpui::transparent_black()
        })
        .when(active, |d| d.bg(faded(t.accent, theme::TINT_FILL)))
        .when(!active, |d| d.hover(|s| s.bg(t.row_hover)))
        .child(label.into())
}

/// 紧凑胶囊页签：周期直选等需要在一行内放下大量档位的场景。
/// font_sm() + 5pt 横向内边距 + 22pt 行高，宽度约为 pill_tab 的 55%
pub fn pill_tab_sm(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let t = cur();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(5.0))
        .h(px(22.0))
        .rounded(px(crate::ui::theme::R_SM))
        .text_size(font_sm())
        .text_color(if active { t.accent } else { t.text_dim })
        .whitespace_nowrap()
        .cursor_pointer()
        // 无条件带 1px 描边：选中 accent 描边、未选中透明，选中态有
        // 轮廓且两种状态间无布局抖动
        .border_1()
        .border_color(if active {
            faded(t.accent, theme::TINT_SOFT)
        } else {
            gpui::transparent_black()
        })
        .when(active, |d| d.bg(faded(t.accent, theme::TINT_FILL)))
        .when(!active, |d| d.hover(|s| s.bg(t.row_hover)))
        .child(label.into())
}

/// 在分配好的空间内绘制胶囊页签（文本居中，命中区=整个分配空间）：
/// 供「半区等分」类布局复用——调用方先给元素 flex_1 / w_1_2 等分配宽度
pub fn pill_tab_rect(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let t = cur();
    div()
        .id(id)
        .flex_1()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(crate::ui::theme::R_MD))
        .text_size(font_body())
        .text_color(if active { t.accent } else { t.text_dim })
        .whitespace_nowrap()
        .cursor_pointer()
        // 无条件带 1px 描边：选中 accent 描边、未选中透明，选中态有
        // 轮廓且两种状态间无布局抖动
        .border_1()
        .border_color(if active {
            faded(t.accent, theme::TINT_SOFT)
        } else {
            gpui::transparent_black()
        })
        .when(active, |d| d.bg(faded(t.accent, theme::TINT_FILL)))
        .when(!active, |d| d.hover(|s| s.bg(t.row_hover)))
        .child(label.into())
}

// ---------- 徽标 / 状态灯 ----------

/// 彩色小徽标（egui 版的 extra_letter_spacing 在 GPUI 无对应样式，字距省略）
pub fn badge(text: impl Into<SharedString>, fg: Rgba) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .px(px(7.0))
        .py(px(theme::SPACE_XS))
        .rounded(px(crate::ui::theme::R_SM))
        .bg(faded(fg, 0.22))
        .border_1()
        .border_color(faded(fg, 0.85))
        .text_size(font_sm())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .whitespace_nowrap()
        .child(text.into())
}

/// 顶栏连接状态项：「名称 + 状态灯」紧邻成组，详细状态（连接中/未连接/
/// 错误原因）收进悬停提示
pub fn status_light(name: &str, st: &ConnState) -> Stateful<gpui::Div> {
    let (c, st_txt) = status_light_info(name, st);
    status_item(name, c, *st == ConnState::Connected, st_txt)
}

/// 顶栏布尔状态项（REST 接口）：结构与 status_light 一致，仅状态来自布尔
pub fn status_light_ok(name: &str, ok: bool, ok_txt: &str, err_txt: &str) -> Stateful<gpui::Div> {
    let t = cur();
    status_item(
        name,
        if ok { t.up } else { t.err },
        ok,
        if ok {
            ok_txt.to_string()
        } else {
            err_txt.to_string()
        },
    )
}

/// 顶栏连接状态项的状态色与提示文案
pub fn status_light_info(name: &str, st: &ConnState) -> (Rgba, String) {
    let (c, st_txt) = match st {
        ConnState::Connected => (cur().up, crate::i18n::tr("已连接").to_string()),
        ConnState::Connecting => (cur().accent, crate::i18n::tr("连接中").to_string()),
        ConnState::Disconnected => (cur().text_dim, crate::i18n::tr("未连接").to_string()),
        ConnState::Failed(e) => (
            cur().err,
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

fn status_item(name: &str, c: Rgba, glow: bool, hover: String) -> Stateful<gpui::Div> {
    let t = cur();
    let tip = format!("{name} · {hover}");
    let el = div()
        .id(SharedString::from(format!("st_{name}")))
        .flex()
        .items_center()
        .gap(px(theme::SPACE_SM))
        .child(
            div()
                .text_size(font_sm())
                .text_color(t.text_dim)
                .whitespace_nowrap()
                .child(name.to_string()),
        )
        .child(
            // 状态灯：10px 槽位内 9px 圆点；已连接时加一圈微光晕（13px 描边环）
            div()
                .relative()
                .w(px(10.0))
                .h(px(10.0))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().w(px(9.0)).h(px(9.0)).rounded_full().bg(c)),
                )
                .when(glow, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-1.5))
                            .left(px(-1.5))
                            .w(px(13.0))
                            .h(px(13.0))
                            .rounded_full()
                            .border_1()
                            .border_color(faded(c, 0.35)),
                    )
                }),
        );
    with_tooltip(el, tip)
}

/// 紧凑文字/图标按钮：自绘 glyph 的方形热区，无按钮的固定最小高——
/// 混排不同字号的行里与相邻文字做竖直对齐时，flex items_center 已天然
/// 解决（旧 egui 版需要手动 anchor_y 锚定的对齐问题在 flex 布局下不存在）
pub fn glyph_button(
    id: impl Into<ElementId>,
    text: &str,
    size: f32,
    color: Rgba,
    hover: &str,
) -> Stateful<gpui::Div> {
    let t = cur();
    let el = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(size + 4.0))
        .px(px(theme::SPACE_SM))
        .rounded(px(crate::ui::theme::R_SM))
        .text_size(px(size))
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.bg(t.row_hover))
        .child(text.to_string());
    with_tooltip(el, hover.to_string())
}

// ---------- 卡片 / 表单行 ----------

/// 卡片容器：圆角 + 细边框 + 内边距（内容子元素由调用方 .child 追加）
pub fn card() -> gpui::Div {
    let t = cur();
    div()
        .bg(t.card)
        // 边框压暗一档：卡片轮廓更实，与面板底色拉开层次
        .border_1()
        .border_color(faded(t.border, theme::FADE_BORDER))
        .rounded(px(crate::ui::theme::R_LG))
        .px(px(theme::SPACE_XL))
        .py(px(theme::SPACE_LG))
        .my(px(theme::SPACE_SM))
}

/// 卡片内标题行：标题 + flex_1 弹性槽（调用方在槽后追加右侧操作元素即右对齐）
pub fn card_header(title: impl Into<SharedString>) -> gpui::Div {
    let t = cur();
    div()
        .flex()
        .items_center()
        .w_full()
        // 标题前 3px accent 竖条：卡头可扫读锚点（chart 选中行/侧栏同款语义）
        .child(
            div()
                .w(px(3.0))
                .h(px(14.0))
                .rounded(px(1.5))
                .bg(t.accent)
                .mr(px(theme::SPACE_MD)),
        )
        .child(
            div()
                .text_size(font_lg())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(title.into()),
        )
        .child(div().flex_1())
}

/// 水平分隔线（卡片头部与内容体之间等）
pub fn separator() -> gpui::Div {
    let t = cur();
    div()
        .w_full()
        .h(px(1.0))
        .my(px(theme::SPACE_MD))
        .bg(faded(t.border, theme::FADE_BORDER))
}

/// 模态/浮层卡片统一外壳：panel2 底 + FADE_BORDER 描边 + R_LG + shadow_lg
/// + SPACE_XL 内边距 + SPACE_MD 纵向间距。
///
/// 调用方可再覆盖宽度/描边色等（如确认下单的买卖色描边）；
/// 遮罩层由调用方自绘（有无遮罩因弹层而异）
pub fn modal_card() -> gpui::Div {
    let t = cur();
    div()
        .bg(t.panel2)
        .border_1()
        .border_color(faded(t.border, theme::FADE_BORDER))
        .rounded(px(crate::ui::theme::R_LG))
        .shadow_lg()
        .p(px(theme::SPACE_XL))
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
}

/// 带标题行的卡片：标题 + 可选右侧操作槽 + 分隔线 + 内容体
pub fn card_with_header(
    title: impl Into<SharedString>,
    right: Option<AnyElement>,
    body: AnyElement,
) -> gpui::Div {
    let mut header = card_header(title);
    if let Some(r) = right {
        header = header.child(r);
    }
    card().flex().flex_col().child(header).child(separator()).child(body)
}

/// 固定高度表单行：行内所有元素以同一基线垂直居中（egui 版预分配 ROW_H
/// 防止后置高控件把行撑高造成标签错位；flex 布局下行高即由此决定，
/// 高控件不再顶歪标签）。行内间距由调用方 gap_* 控制
pub fn form_row() -> gpui::Div {
    div()
        .h(px(crate::ui::theme::ROW_H))
        .flex()
        .flex_row()
        .items_center()
}

/// 表单行标签：固定宽度保证后续控件左缘对齐
pub fn row_label(text: impl Into<SharedString>, w: f32) -> gpui::Div {
    div()
        .w(px(w))
        .h(px(crate::ui::theme::ROW_H))
        .flex()
        .items_center()
        // 定宽放不下（如英文 "Default Interval"）时按边界裁剪——
        // 遮挡优于叠印：标签文字绝不溢出画到后面的控件上
        .overflow_hidden()
        .text_size(font_body())
        .whitespace_nowrap()
        .child(text.into())
}

/// 区块小标题
pub fn section(label: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(font_sm())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cur().text_dim)
        .child(label.into())
}

/// 表格/卡片内的小标签文字
pub fn card_label(label: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(font_sm())
        .text_color(cur().text_dim)
        .child(label.into())
}

/// 表格数值单元格：右对齐 + 等宽字体（价格/数量/金额/盈亏列通用）
pub fn num_cell(txt: impl Into<SharedString>, color: Option<Rgba>) -> gpui::Div {
    let t = cur();
    div()
        .flex()
        .justify_end()
        .font_family(crate::ui::theme::FONT_MONO)
        .text_size(font_md())
        .text_color(color.unwrap_or(t.text))
        .whitespace_nowrap()
        .child(txt.into())
}

// ---------- 空态 / 加载态 ----------

/// 空状态：居中的暗色图标 + 标题 + 副标题（subtitle 为空则不渲染）
pub fn empty_state(icon: &str, title: &str, subtitle: &str) -> gpui::Div {
    let t = cur();
    let mut col = div()
        .flex()
        .flex_col()
        .items_center()
        .w_full()
        .pt(px(theme::SPACE_XXL))
        .pb(px(theme::SPACE_XXL))
        .child(
            div()
                .text_size(font_xl())
                .text_color(t.text_dim)
                .child(icon.to_string()),
        )
        .child(
            // 标题用正文色加粗压过副标题：空态也要有信息层级
            div()
                .mt(px(theme::SPACE_XS))
                .text_size(font_body())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(title.to_string()),
        );
    if !subtitle.is_empty() {
        col = col.child(
            div()
                .text_size(font_md())
                .text_color(faded(t.text_dim, 0.85))
                .child(subtitle.to_string()),
        );
    }
    col
}

/// 加载态：脉冲圆点 + 暗字说明，并预留最小高度——数据到达后不从一行字
/// 跳成整表（盘口/成交/资产等首帧加载共用）
pub fn loading_state(msg: &str) -> impl IntoElement {
    let t = cur();
    div()
        .w_full()
        .h(px(120.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::SPACE_LG))
        .child(
            div()
                .w(px(10.0))
                .h(px(10.0))
                .rounded_full()
                .bg(t.accent)
                .with_animation(
                    ElementId::Name("loading_pulse".into()),
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |el, delta| el.opacity(0.25 + 0.75 * delta),
                ),
        )
        .child(
            div()
                .text_size(font_md())
                .text_color(t.text_dim)
                .child(msg.to_string()),
        )
}

// ---------- Toast ----------

/// 单条 Toast 的样式常量
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
pub fn toast_color(kind: ToastKind) -> Rgba {
    let t = cur();
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

/// 右下角 Toast 覆盖层：投影 + 圆角卡片 + 左侧 kind 色条 + 圆形图标 +
/// TTL 进度条 + 点击关闭，入场 250ms 淡入
pub fn draw_toasts(state: &Arc<SharedState>) -> gpui::Div {
    let toasts = prune_toasts(state);
    let mut wrap = div()
        .absolute()
        .bottom(px(theme::SPACE_XXL))
        .right(px(theme::SPACE_XXL))
        .flex()
        .flex_col()
        .items_end()
        .gap(px(theme::SPACE_MD));
    for (i, t) in toasts.iter().enumerate() {
        wrap = wrap.child(draw_toast(state, i, t));
    }
    wrap
}

/// 单条 Toast 卡片
fn draw_toast(state: &Arc<SharedState>, i: usize, t: &Toast) -> impl IntoElement {
    let t_cur = cur();
    let kind_color = toast_color(t.kind);
    // TTL 与 prune_toasts 的 retain 规则保持一致：错误 15s，其余 5s
    let ttl = toast_ttl(t.kind);
    let remain = ((ttl - t.at.elapsed().as_secs_f64()) / ttl).clamp(0.0, 1.0) as f32;
    let shadow_a = if is_light() { 40.0 / 255.0 } else { 90.0 / 255.0 };
    let msg = t.msg.clone();
    let at = t.at;
    let st = Arc::clone(state);

    let card = div()
        .id(ElementId::named_usize("toast", i))
        .relative()
        .max_w(px(TOAST_MAX_W))
        .px(px(theme::SPACE_XL))
        .py(px(theme::SPACE_LG))
        .flex()
        .items_center()
        .gap(px(theme::SPACE_LG))
        .bg(t_cur.panel2)
        .border_1()
        .border_color(t_cur.border)
        .rounded(px(crate::ui::theme::R_LG))
        .shadow(vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, shadow_a),
            offset: point(px(0.0), px(4.0)),
            blur_radius: px(14.0),
            spread_radius: px(0.0),
        }])
        .cursor_pointer()
        // 圆形图标底 + 内嵌 8px 实心圆点：不用 ✓/✗/! 字形，
        // 规避等宽字体优先后字形缺字回退成豆腐块的风险
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(faded(kind_color, 0.2))
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(Hsla::from(kind_color)),
                ),
        )
        .child(
            div()
                .text_size(font_md())
                .text_color(t_cur.text)
                .child(msg.clone()),
        )
        // 左侧 kind 色条（3px，上下内缩 4px 避开圆角）
        .child(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(4.0))
                .bottom(px(4.0))
                .w(px(3.0))
                .rounded(px(2.0))
                .bg(Hsla::from(kind_color)),
        )
        // TTL 进度条（底部 2px，kind 色，宽度 = 剩余时间占比；两端内缩避开圆角）
        .child(
            div()
                .absolute()
                .left(px(8.0))
                .right(px(8.0))
                .bottom(px(0.0))
                .h(px(2.0))
                .overflow_hidden()
                .child(
                    div()
                        .w(relative(remain))
                        .h_full()
                        .bg(Hsla::from(kind_color)),
                ),
        )
        // 点击即关闭（不等 TTL 自然过期）：按 (内容, 入队时刻) 精确匹配
        .on_click(move |_, _, _| {
            st.toasts.lock().retain(|x| !(x.msg == msg && x.at == at));
        });
    let _ = _t;
    with_tooltip(card, crate::i18n::tr("点击关闭").to_string())
        // 入场 250ms 滑入淡入（整体 opacity 等价旧版逐色 alpha 衰减）
        .with_animation(
            ElementId::named_usize("toast_in", i),
            Animation::new(Duration::from_millis(250)),
            |el, delta| el.opacity(delta.clamp(0.0, 1.0)),
        )
}

// ---------- 拖拽把手幽灵视图 ----------

/// 面板宽度把手的拖拽跟随物：gpui 的 `on_drag` 构造器必须返回一个
/// Render 实体作为拖拽光标旁的跟随视图；宽度把手只需要拖拽手势本身，
/// 跟随物给空 div 占位即可
pub struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

// ---------- 测试 ----------

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
