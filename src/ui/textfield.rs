//! 手写单行文本输入框（GPUI 0.2 无内置文本组件，本迁移最大的新控件）。
//!
//! 设计：
//! - `TextField` 是 Entity：文本/光标/选区状态跨帧存活，焦点由 FocusHandle 管理
//! - 实现 `EntityInputHandler` 并在绘制期经 `window.handle_input` 挂载 →
//!   获得系统 IME/composition 与粘贴支持（marked text 最小正确实现：把
//!   组合文本视为直接替换，中文数字输入场景够用）
//! - 键盘处理在 on_key_down：字符插入、Backspace/Delete、方向键、Home/End、
//!   Ctrl+A/C/V/X、Enter 提交回调
//! - 数字模式：插入与粘贴内容先过滤（数字/. e E + -），与 widgets::parse_dec 兼容
//! - masked 模式：显示 ●（显示文本字节位与原文不同，光标/选区经字符序号映射）
//! - 聚焦时 stop_propagation：B/S 快捷键等根级按键监听不触发（对应
//!   egui 版的 focused 门控）；Escape 不拦截，交给根级解除两步武装/关模态
//!
//! 取值模型：状态由父视图持有（OrderForm 的 String 字段），TextField 通过
//! `on_change` 回调把每次编辑写回模型；父视图在渲染帧把模型值同步进实体
//! （外部改价，如盘口点击填价）。双向一致性由"谁改谁通知"保证。

use gpui::{
    canvas, div, font, point, prelude::FluentBuilder, px, size, App, Bounds, ClipboardItem,
    Context,
    EntityInputHandler, FocusHandle, Hsla, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, TextRun, UTF16Selection, Window,
};
use crate::ui::theme;
use std::ops::Range;

/// 文本框实例：由父视图 `cx.new(|cx| TextField::new(cx))` 创建并持有 Entity
pub struct TextField {
    text: String,
    /// 光标字节位（恒在字符边界上）
    cursor: usize,
    /// 选区锚点（字节位）：Some = 选中 anchor..cursor
    sel_anchor: Option<usize>,
    focus: FocusHandle,
    id: SharedString,
    placeholder: SharedString,
    masked: bool,
    numeric: bool,
    font_family: SharedString,
    font_size: Pixels,
    line_height: Pixels,
    /// 渲染期写入的文本颜色（shape/paint 用）
    paint_color: Hsla,
    on_change: Option<Box<TextChangeFn>>,
    on_submit: Option<Box<SubmitFn>>,
    /// 绘制期实测 bounds（点击→光标定位用，滞后一帧可接受）
    bounds: Bounds<Pixels>,
}

/// 文本变更回调（on_change）：文本、窗口、App
type TextChangeFn = dyn Fn(&str, &mut Window, &mut App);
/// 提交回调（on_submit）：回车触发
type SubmitFn = dyn Fn(&mut Window, &mut App);

// ---------- 字节/字符/UTF-16 换算 ----------

fn next_boundary(s: &str, i: usize) -> usize {
    s[i..].chars().next().map(|c| i + c.len_utf8()).unwrap_or(i)
}

fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i]
        .chars()
        .next_back()
        .map(|c| i - c.len_utf8())
        .unwrap_or(i)
}

fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text[..byte].chars().map(char::len_utf16).sum()
}

fn utf16_to_byte(text: &str, u16pos: usize) -> usize {
    let mut n = 0;
    for (i, ch) in text.char_indices() {
        if n >= u16pos {
            return i;
        }
        n += ch.len_utf16();
    }
    text.len()
}

const MASK_CHAR: char = '●';

/// 文本字节位 → 显示文本字节位（masked 时显示文本是 ● 重复，字节宽不同）
fn text_byte_to_display_byte(text: &str, byte: usize, masked: bool) -> usize {
    if masked {
        text[..byte].chars().count() * MASK_CHAR.len_utf8()
    } else {
        byte
    }
}

/// 显示文本字节位 → 文本字节位
fn display_byte_to_text_byte(text: &str, db: usize, masked: bool) -> usize {
    if masked {
        let chars = db / MASK_CHAR.len_utf8();
        text.char_indices()
            .nth(chars)
            .map(|(i, _)| i)
            .unwrap_or(text.len())
    } else {
        db.min(text.len())
    }
}

/// 数字模式输入过滤：与 Decimal::from_str_exact 的合法字符对齐
fn numeric_filter(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        .collect()
}

impl TextField {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            sel_anchor: None,
            focus: cx.focus_handle(),
            id: "text_field".into(),
            placeholder: "".into(),
            masked: false,
            numeric: false,
            font_family: crate::ui::theme::FONT_MONO.into(),
            font_size: crate::ui::theme::font_md(),
            line_height: px(crate::ui::theme::CONTROL_H),
            paint_color: crate::ui::theme::cur().text.into(),
            on_change: None,
            on_submit: None,
            bounds: Bounds::default(),
        }
    }

    // ---- 配置（渲染帧调用） ----

    pub fn set_id(&mut self, id: impl Into<SharedString>) -> &mut Self {
        self.id = id.into();
        self
    }
    pub fn set_placeholder(&mut self, p: impl Into<SharedString>) -> &mut Self {
        self.placeholder = p.into();
        self
    }
    pub fn set_masked(&mut self, masked: bool) -> &mut Self {
        self.masked = masked;
        self
    }
    pub fn set_numeric(&mut self, numeric: bool) -> &mut Self {
        self.numeric = numeric;
        self
    }
    pub fn set_font_size(&mut self, s: Pixels) -> &mut Self {
        self.font_size = s;
        self
    }
    pub fn set_on_change(&mut self, f: Box<TextChangeFn>) -> &mut Self {
        self.on_change = Some(f);
        self
    }
    pub fn set_on_submit(&mut self, f: Box<SubmitFn>) -> &mut Self {
        self.on_submit = Some(f);
        self
    }

    // ---- 状态读写（父视图调用） ----

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn set_text(&mut self, s: impl Into<String>) {
        self.text = s.into();
        self.cursor = self.text.len();
        self.sel_anchor = None;
    }
    pub fn clear(&mut self) {
        self.set_text("");
    }
    pub fn focus(&self, window: &mut Window) {
        self.focus.focus(window);
    }
    /// 本输入框是否持有焦点（根级快捷键让位判定用）
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }
    pub fn select_all(&mut self) {
        self.sel_anchor = Some(0);
        self.cursor = self.text.len();
    }

    // ---- 编辑内部实现 ----

    fn sel_range(&self) -> Option<Range<usize>> {
        self.sel_anchor
            .map(|a| if a <= self.cursor { a..self.cursor } else { self.cursor..a })
    }

    /// 删除选区（若有），返回是否存在选区
    fn delete_selection(&mut self) -> bool {
        if let Some(r) = self.sel_range() {
            self.text.replace_range(r.clone(), "");
            self.cursor = r.start;
            self.sel_anchor = None;
            true
        } else {
            false
        }
    }

    fn insert_str(&mut self, s: &str, window: &mut Window, cx: &mut Context<Self>) {
        let s = if self.numeric { numeric_filter(s) } else { s.to_string() };
        if s.is_empty() {
            return;
        }
        self.delete_selection();
        let at = self.cursor.min(self.text.len());
        self.text.insert_str(at, &s);
        self.cursor = at + s.len();
        self.emit(window, cx);
    }

    fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_selection() {
            self.emit(window, cx);
            return;
        }
        if self.cursor > 0 {
            let p = prev_boundary(&self.text, self.cursor);
            self.text.replace_range(p..self.cursor, "");
            self.cursor = p;
            self.emit(window, cx);
        }
    }

    fn delete_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.delete_selection() {
            self.emit(window, cx);
            return;
        }
        if self.cursor < self.text.len() {
            let n = next_boundary(&self.text, self.cursor);
            self.text.replace_range(self.cursor..n, "");
            self.emit(window, cx);
        }
    }

    fn emit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = &self.on_change {
            let text = self.text.clone();
            f(&text, window, cx);
        }
        cx.notify();
    }

    // ---- 显示文本与字形 ----

    fn display_text(&self) -> String {
        if self.masked {
            std::iter::repeat_n(MASK_CHAR, self.text.chars().count()).collect()
        } else {
            self.text.clone()
        }
    }

    fn shape_display(&self, window: &Window) -> gpui::ShapedLine {
        let display = self.display_text();
        let font = font(self.font_family.clone());
        let run = TextRun {
            len: display.len(),
            font,
            color: self.paint_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(display.into(), self.font_size, &[run], None)
    }

    // ---- 事件处理 ----

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        let x = e.position.x - self.bounds.origin.x;
        let line = self.shape_display(window);
        let db = line.index_for_x(x.max(px(0.0))).unwrap_or(self.display_text().len());
        self.cursor = display_byte_to_text_byte(&self.text, db, self.masked);
        self.sel_anchor = None;
        cx.notify();
        // 吃掉本次点击，避免穿透到根级"点击空白解除武装"逻辑
        cx.stop_propagation();
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let key = e.keystroke.key.as_str();
        let ctrl = e.keystroke.modifiers.control || e.keystroke.modifiers.platform;
        // 特殊键先处理；字符插入走 key_char
        match key {
            "backspace" => {
                self.backspace(window, cx);
            }
            "delete" => {
                self.delete_forward(window, cx);
            }
            "left" => {
                if self.sel_anchor.is_none() && e.keystroke.modifiers.shift {
                    self.sel_anchor = Some(self.cursor);
                }
                if self.sel_anchor.is_some() && !e.keystroke.modifiers.shift {
                    if let Some(r) = self.sel_range() {
                        self.cursor = r.start;
                    }
                    self.sel_anchor = None;
                } else if self.cursor > 0 {
                    self.cursor = prev_boundary(&self.text, self.cursor);
                }
                cx.notify();
            }
            "right" => {
                if self.sel_anchor.is_none() && e.keystroke.modifiers.shift {
                    self.sel_anchor = Some(self.cursor);
                }
                if self.sel_anchor.is_some() && !e.keystroke.modifiers.shift {
                    if let Some(r) = self.sel_range() {
                        self.cursor = r.end;
                    }
                    self.sel_anchor = None;
                } else if self.cursor < self.text.len() {
                    self.cursor = next_boundary(&self.text, self.cursor);
                }
                cx.notify();
            }
            "home" => {
                if e.keystroke.modifiers.shift {
                    self.sel_anchor.get_or_insert(self.cursor);
                } else {
                    self.sel_anchor = None;
                }
                self.cursor = 0;
                cx.notify();
            }
            "end" => {
                if e.keystroke.modifiers.shift {
                    self.sel_anchor.get_or_insert(self.cursor);
                } else {
                    self.sel_anchor = None;
                }
                self.cursor = self.text.len();
                cx.notify();
            }
            "enter" => {
                if let Some(f) = self.on_submit.take() {
                    f(window, cx);
                    self.on_submit = Some(f);
                }
            }
            "escape" => {
                // 不拦截：根级 Esc 用于解除两步武装/关闭弹窗
                return;
            }
            "a" if ctrl => {
                self.select_all();
                cx.notify();
            }
            "c" | "x" if ctrl => {
                // 掩码字段（主密码/API Key/Secret）禁止进剪贴板：系统剪贴板
                // 全局可读，剪贴板历史/跨设备同步还可能保留超出会话的生命周期
                if self.masked {
                    cx.notify();
                } else if let Some(r) = self.sel_range() {
                    let s = self.text[r.clone()].to_string();
                    cx.write_to_clipboard(ClipboardItem::new_string(s));
                    if key == "x" {
                        self.delete_selection();
                        self.emit(window, cx);
                    }
                }
            }
            "v" if ctrl => {
                if let Some(item) = cx.read_from_clipboard() {
                    if let Some(text) = item.text() {
                        self.insert_str(&text, window, cx);
                    }
                }
            }
            _ => {
                if let Some(ch) = &e.keystroke.key_char {
                    self.insert_str(ch, window, cx);
                } else {
                    // 未消费的修饰键/功能键：不拦截，交给根级
                    return;
                }
            }
        }
        // 聚焦期间消费所有已处理按键：B/S 快捷键等根级监听不得触发
        cx.stop_propagation();
    }

    fn on_clicked(&mut self, _e: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        cx.notify();
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::ui::theme::c(cx);
        let focused = self.focus.is_focused(window);
        self.paint_color = t.text.into();

        let show_placeholder = self.text.is_empty() && !self.placeholder.is_empty();
        let shaped_src: String = if show_placeholder {
            self.placeholder.to_string()
        } else {
            self.display_text()
        };
        let text_color: Hsla = if show_placeholder {
            t.text_dim.into()
        } else {
            t.text.into()
        };

        let cursor_db = text_byte_to_display_byte(&self.text, self.cursor, self.masked);
        let sel = self.sel_range().map(|r| {
            text_byte_to_display_byte(&self.text, r.start, self.masked)
                ..text_byte_to_display_byte(&self.text, r.end, self.masked)
        });
        let line_h = self.line_height;
        let border_color: Hsla = if focused {
            t.accent.into()
        } else {
            t.border.into()
        };

        let entity = cx.entity();
        let focus = self.focus.clone();
        let font = font(self.font_family.clone());
        let font_size = self.font_size;

        // 聚焦态：accent 边框之外再叠一圈柔和 accent 光晕（BoxShadow 无偏移
        // 纯模糊），与边框同源同色，输入位置一眼可辨
        let focus_ring = gpui::BoxShadow {
            color: gpui::Hsla::from(t.accent).opacity(theme::TINT_SOFT),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(6.0),
            spread_radius: px(0.0),
        };

        div()
            .id(self.id.clone())
            .track_focus(&self.focus)
            .on_click(cx.listener(Self::on_clicked))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_key_down(cx.listener(Self::on_key))
            .flex()
            .items_center()
            .w_full()
            .h(line_h)
            .px(px(theme::SPACE_MD))
            .bg(t.panel2)
            .border_1()
            .border_color(border_color)
            .rounded(px(crate::ui::theme::R_SM))
            .when(focused, |d| d.shadow(vec![focus_ring]))
            .overflow_hidden()
            .child(canvas(
                move |bounds, _window, _cx| bounds,
                move |bounds, _, window: &mut Window, cx: &mut App| {
                    // 实测 bounds 回写：点击→光标定位与 IME 候选窗定位都靠它
                    entity.update(cx, |tf, _| tf.bounds = bounds);
                    // IME/输入法挂载：绘制期注册 input handler（聚焦时生效）
                    window.handle_input(&focus, gpui::ElementInputHandler::new(bounds, entity.clone()), cx);
                    // 字形绘制：选区底色 → 文本 → 光标
                    let run = TextRun {
                        len: shaped_src.len(),
                        font: font.clone(),
                        color: text_color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window
                        .text_system()
                        .shape_line(shaped_src.into(), font_size, &[run], None);
                    // 选区高亮（高度取实测内容盒，避免压过边框）
                    if let Some(sel) = &sel {
                        if sel.start < sel.end {
                            let x0 = line.x_for_index(sel.start);
                            let x1 = line.x_for_index(sel.end);
                            if x1 > x0 {
                                window.paint_quad(gpui::fill(
                                    Bounds {
                                        origin: Point {
                                            x: bounds.origin.x + x0,
                                            y: bounds.origin.y,
                                        },
                                        size: size(x1 - x0, bounds.size.height),
                                    },
                                    crate::ui::theme::faded(t.accent, 0.22),
                                ));
                            }
                        }
                    }
                    // 行高取实测内容盒高度：文本在盒内精确垂直居中（paint 内部按
                    // (line_h - ascent - descent)/2 定基线）
                    let paint_h = if bounds.size.height > px(0.0) { bounds.size.height } else { line_h };
                    let _ = line.paint(bounds.origin, paint_h, window, cx);
                    // 光标：聚焦时 2px 竖线（在内容盒内垂直居中）
                    if focused {
                        let cx_pos = line.x_for_index(cursor_db);
                        let cur_h = line_h - px(6.0);
                        window.paint_quad(gpui::fill(
                            Bounds {
                                origin: Point {
                                    x: bounds.origin.x + cx_pos,
                                    y: bounds.origin.y + (bounds.size.height - cur_h) / 2.0,
                                },
                                size: size(px(2.0), cur_h),
                            },
                            Hsla::from(t.text),
                        ));
                    }
                },
            )
            // canvas 必须显式撑满内容盒：Canvas 布局默认 Style::default() 是 0×0，
            // 在 items_center 父容器里会居中成一个点，文本从盒中点开始画掉进下半区
            .size_full())
    }
}

// ---------- EntityInputHandler（IME / 系统文本服务接口） ----------

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        // 平台给的 range 是 UTF-16 位；换算成字节位并回填 adjusted
        let s = utf16_to_byte(&self.text, range.start);
        let e = utf16_to_byte(&self.text, range.end);
        *adjusted_range = Some(s..e);
        Some(self.text[s..e].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (a, b) = self
            .sel_range()
            .map(|r| (r.start, r.end))
            .unwrap_or((self.cursor, self.cursor));
        Some(UTF16Selection {
            range: byte_to_utf16(&self.text, a)..byte_to_utf16(&self.text, b),
            reversed: self.sel_anchor.is_some_and(|a| a > self.cursor),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        // marked text 最小实现：不做组合态跟踪
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (start, end) = match range {
            Some(r) => (
                utf16_to_byte(&self.text, r.start),
                utf16_to_byte(&self.text, r.end),
            ),
            // None = 替换当前选区（或光标处插入）
            None => match self.sel_range() {
                Some(r) => (r.start, r.end),
                None => (self.cursor, self.cursor),
            },
        };
        let text = if self.numeric { numeric_filter(text) } else { text.to_string() };
        let start = start.min(self.text.len());
        let end = end.min(self.text.len()).max(start);
        self.text.replace_range(start..end, &text);
        self.cursor = start + text.len();
        self.sel_anchor = None;
        self.emit(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 最小实现：组合文本直接替换落地
        self.replace_text_in_range(range, new_text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let s = utf16_to_byte(&self.text, range_utf16.start);
        let e = utf16_to_byte(&self.text, range_utf16.end);
        let line = self.shape_display(window);
        let x0 = line
            .x_for_index(text_byte_to_display_byte(&self.text, s, self.masked));
        let x1 = line
            .x_for_index(text_byte_to_display_byte(&self.text, e, self.masked));
        Some(Bounds {
            origin: Point {
                x: element_bounds.origin.x + x0.min(x1),
                y: element_bounds.origin.y,
            },
            size: size((x1 - x0).abs().max(px(2.0)), element_bounds.size.height),
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let x = point.x - self.bounds.origin.x;
        let line = self.shape_display(window);
        let db = line.index_for_x(x.max(px(0.0)))?;
        let byte = display_byte_to_text_byte(&self.text, db, self.masked);
        Some(byte_to_utf16(&self.text, byte))
    }
}

// ---------- 测试：换算工具（不依赖窗口） ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_roundtrip_ascii() {
        let s = "abc123";
        assert_eq!(utf16_to_byte(s, byte_to_utf16(s, 4)), 4);
        assert_eq!(utf16_to_byte(s, 100), s.len());
    }

    #[test]
    fn utf16_roundtrip_cjk() {
        // 中文字符 UTF-16 长 1、UTF-8 长 3：位映射必须经字符换算
        let s = "价格1.5";
        let b4 = s.char_indices().nth(3).unwrap().0; // '1' 的字节位
        assert_eq!(byte_to_utf16(s, b4), 3);
        assert_eq!(utf16_to_byte(s, 3), b4);
    }

    #[test]
    fn boundary_moves_are_char_wise() {
        let s = "ab价c";
        let i1 = next_boundary(s, 0);
        let i2 = next_boundary(s, i1);
        let i3 = next_boundary(s, i2); // 跨过"价"（3 字节）
        assert_eq!(i3, 2 + 3);
        assert_eq!(prev_boundary(s, i3), i2);
    }

    #[test]
    fn masked_display_mapping() {
        let s = "ab1";
        // ● 占 3 字节：第 2 个字符的显示字节位 = 3
        assert_eq!(text_byte_to_display_byte(s, 1, true), 3);
        assert_eq!(display_byte_to_text_byte(s, 3, true), 1);
        assert_eq!(display_byte_to_text_byte(s, 9, true), 3);
        // 非 masked 恒等
        assert_eq!(text_byte_to_display_byte(s, 2, false), 2);
    }

    #[test]
    fn numeric_filter_keeps_decimal_chars() {
        assert_eq!(numeric_filter("12.5e-3x"), "12.5e-3");
        assert_eq!(numeric_filter("abc"), "");
    }
}
