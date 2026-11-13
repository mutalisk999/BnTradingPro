//! 左侧交易对列表：搜索 + 自选置顶 + 全量虚拟化列表（uniform_list，自选/全部同构渲染）

use crate::app::BnApp;
use crate::ui::theme::{self, cur, faded, font_md, font_sm, FONT_MONO};
use crate::ui::widgets;
use gpui::{
    div, prelude::FluentBuilder, px, uniform_list, AnyElement, Bounds, ClickEvent, Context,
    ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    Point, SharedString, StatefulInteractiveElement, Styled, UniformListDecoration, WeakEntity,
};
use std::collections::HashSet;
use std::sync::Arc;

/// 星标按钮固定宽度：不依赖字形自然宽度（等宽字体优先下 "☆" 可能回落到
/// 全角字形），确保星标永远落在行槽裁剪区内、热区完整可点击
const STAR_W: f32 = 24.0;
/// 小节标题固定占位高度
const SECTION_H: f32 = 22.0;
/// 卡片右缘内边距：第二行涨跌幅靠最右，与左缘 6pt 缩进对称
const CARD_RIGHT_PAD: f32 = 6.0;
/// 行内第一行（名称）与第二行（价格/涨跌）之间的行隙
const LINE_GAP: f32 = 2.0;
/// 卡片左缘到名称文字的缩进：行首内边距 6 + 星标槽宽 + 星标/名称间距 4
const NAME_X: f32 = 6.0 + STAR_W + 4.0;
/// 相邻两个交易对行之间的留白：悬停/选中色带不覆盖这部分，
/// 视觉上每行各自成为独立卡片、不再「粘」在一起
const ROW_GAP: f32 = 7.0;
/// 卡片上下内边距：文字行高之外再留的呼吸空间
const CARD_PAD: f32 = 4.0;

/// 行槽尺度：GPUI 文本默认行高为字号 ×1.618，行槽取 max(文字行高, 18)
/// 兜底（与 egui 版 interact_size.y 语义一致）。所有高度按当前字号档位
/// 现算，行槽与行内排版共用同一组数字，虚拟列表不会错位
#[derive(Clone, Copy)]
struct Metrics {
    /// 第一行（星标 + 名称）行高
    l1: f32,
    /// 第二行（价格 + 涨跌）行高
    l2: f32,
    /// 单行卡片色带高度
    band: f32,
    /// 整行节距（uniform_list 的行高）
    pitch: f32,
}

impl Metrics {
    fn current() -> Self {
        let l1 = (f32::from(font_md()) * 1.618).max(18.0);
        let l2 = (f32::from(font_sm()) * 1.618).max(18.0);
        let band = l1 + LINE_GAP + l2 + 2.0 * CARD_PAD;
        let pitch = (band + ROW_GAP).max(SECTION_H);
        Self { l1, l2, band, pitch }
    }
}

/// 虚拟化列表的行模型：小节标题或交易对行。
/// 全部区的符号串走 `Arc<Vec<SharedString>>` 缓存引用（行向量按帧克隆时
/// 只递增引用计数，不为 ~2600 个符号逐个分配），自选区是小列表、按帧克隆
enum Row {
    Section(&'static str),
    Fav(SharedString),
    /// 缓存列表 + 下标（排除自选后仍保持字典序）
    All(Arc<Vec<SharedString>>, usize),
}

/// 左侧列表主体：自选置顶 + 全部进同一个 uniform_list，同一行高、同一渲染。
/// 搜索框由 app.rs 顶行承载；过滤词直接读搜索框文本（单一数据源）
pub fn list(app: &mut BnApp, cx: &mut Context<BnApp>) -> AnyElement {
    let t = cur();
    let m = Metrics::current();
    let weak = cx.entity().downgrade();

    let filter = app.search_field.read(cx).text().trim().to_uppercase();
    let favs: Vec<SharedString> = app
        .state
        .favorites(app.market)
        .into_iter()
        .filter(|s| filter.is_empty() || s.contains(&filter))
        .map(SharedString::from)
        .collect();
    let fav_set: HashSet<&str> = favs.iter().map(|s| s.as_ref()).collect();

    // 规则簿为空说明 exchangeInfo 尚未加载（网络/代理问题），与"搜索无结果"
    // 是两回事。自选列表来自本地配置可照常显示，全部区给出加载提示
    let rules_loaded = {
        let rules = app.state.rules.read();
        !rules[app.market as usize].symbols.is_empty()
    };
    if !rules_loaded {
        let mut col = div().flex_1().flex().flex_col().mt(px(2.0));
        if !favs.is_empty() {
            col = col
                .child(section_row(crate::i18n::tr("自选"), m))
                .children(favs.iter().enumerate().map(|(i, s)| {
                    // 规则书未加载时无 tick 可依，回退 2 位（USDT 计价对的常见精度）
                    sym_row(app, &weak, s.as_ref(), true, 2, i, m)
                }));
        }
        col = col.child(
            div()
                .mt(px(6.0))
                .text_size(font_md())
                .text_color(t.text_dim)
                .child(crate::i18n::tr("交易规则加载中，请检查网络与代理").to_string()),
        );
        return col.into_any_element();
    }

    // 全部交易对：过滤结果按 (市场, 过滤词) 缓存，避免每帧从规则簿克隆
    // ~2600 个符号字符串；行数据每帧只构造引用视图。缓存键带规则版本：
    // ReloadAll 重拉 exchangeInfo 后旧列表必须失效
    let rules_ver = app
        .state
        .rules_version
        .load(std::sync::atomic::Ordering::Relaxed);
    let cached = app
        .symbols_cache
        .as_ref()
        .filter(|(mk, f, v, _)| *mk == app.market && *f == filter && *v == rules_ver);
    let all_filtered: Arc<Vec<SharedString>> = match cached {
        Some((_, _, _, list)) => Arc::clone(list),
        None => {
            let list: Arc<Vec<SharedString>> = Arc::new({
                let rules = app.state.rules.read();
                let book = &rules[app.market as usize];
                book.symbols
                    .iter()
                    .filter(|s| filter.is_empty() || s.contains(&filter))
                    .map(|s| SharedString::from(s.clone()))
                    .collect()
            });
            app.symbols_cache = Some((app.market, filter.clone(), rules_ver, Arc::clone(&list)));
            list
        }
    };
    let all_idxs: Vec<usize> = all_filtered
        .iter()
        .enumerate()
        .filter(|(_, s)| !fav_set.contains(s.as_ref()))
        .map(|(i, _)| i)
        .collect();

    if favs.is_empty() && all_idxs.is_empty() {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(widgets::empty_state(
                "◎",
                crate::i18n::tr("无匹配交易对"),
                crate::i18n::tr("尝试其他关键词"),
            ))
            .into_any_element();
    }

    // 统一行模型：自选置顶 + 全部进同一个滚动列表
    let mut rows: Vec<Row> = Vec::with_capacity(favs.len() + all_idxs.len() + 2);
    if !favs.is_empty() {
        rows.push(Row::Section(crate::i18n::tr("自选")));
        rows.extend(favs.into_iter().map(Row::Fav));
    }
    if !all_idxs.is_empty() {
        rows.push(Row::Section(crate::i18n::tr("全部")));
        rows.extend(all_idxs.into_iter().map(|i| Row::All(Arc::clone(&all_filtered), i)));
    }
    let rows = Arc::new(rows);
    let count = rows.len();
    let market = app.market;

    uniform_list(
        "symbols_list",
        count,
        move |range, _window, cx| {
            let Some(entity) = weak.upgrade() else {
                return Vec::new();
            };
            let app = entity.read(cx);
            let mut out: Vec<AnyElement> = Vec::with_capacity(range.len());
            for i in range {
                let row = match rows.get(i) {
                    Some(r) => r,
                    None => continue,
                };
                out.push(match row {
                    Row::Section(title) => section_row(title, m).into_any_element(),
                    Row::Fav(sym) => {
                        // 精度取该交易对 tick 的小数位（= quote 侧报价精度）；
                        // 只对可见行逐个查规则书，全量符号每帧查会无谓地抢锁
                        let dp = app
                            .state
                            .with_rule(market, sym, |r| {
                                r.map(|x| x.price_dp() as usize).unwrap_or(2)
                            });
                        sym_row(app, &weak, sym, true, dp, i, m)
                    }
                    Row::All(syms, idx) => {
                        let sym = &syms[*idx];
                        let dp = app
                            .state
                            .with_rule(market, sym, |r| {
                                r.map(|x| x.price_dp() as usize).unwrap_or(2)
                            });
                        sym_row(app, &weak, sym, false, dp, i, m)
                    }
                });
            }
            out
        },
    )
    // 滚动偏移由 element_state 按 id 持久化（不挂每帧新建的 handle——
    // 行情心跳每 ~100ms 重渲染会把刚滚出的偏移打回 0，表现为滚不动）
    .with_decoration(ListScrollbar::new())
    .flex_1()
    .w_full()
    .into_any_element()
}

/// 虚拟列表装饰：右侧自绘滚动条拇指。gpui 0.2.2 的 uniform_list
/// 自带滚轮滚动但不绘制任何可视滚动条（scrollbar_width 只参与布局
/// 不参与绘制），用 UniformListDecoration 在 viewport 裁剪内补一枚
/// 拇指形指示条。纯视觉指示：拖拽拇指暂不支持（需常驻 ScrollHandle）
struct ListScrollbar;

impl ListScrollbar {
    fn new() -> Self {
        Self
    }
}

impl UniformListDecoration for ListScrollbar {
    fn compute(
        &self,
        _visible_range: std::ops::Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        _window: &mut gpui::Window,
        _cx: &mut gpui::App,
    ) -> AnyElement {
        let t = cur();
        let view_h = bounds.size.height;
        let content_h = item_height * item_count as f32;
        // 内容不足一屏：无滚动也无拇指
        if content_h <= view_h || view_h <= px(0.0) {
            return div().into_any_element();
        }
        let track_h = view_h - px(4.0);
        let thumb_h = (track_h * f32::from(view_h) / f32::from(content_h)).max(px(24.0));
        // scroll_offset.y 下滚为负，取负归一到 0-1
        let frac = (-scroll_offset.y / (content_h - view_h)).clamp(0.0, 1.0);
        div()
            .relative()
            .w_full()
            .h_full()
            .child(
                div()
                    .absolute()
                    .top(px(2.0) + frac * (track_h - thumb_h))
                    .right(px(1.0))
                    .w(px(4.0))
                    .h(thumb_h)
                    .rounded_full()
                    .bg(faded(t.text, 0.25)),
            )
            .into_any_element()
    }
}

/// 小节标题行：与交易对行同槽高、垂直居中，整行淡色带与下方列表分隔
fn section_row(title: &'static str, m: Metrics) -> gpui::Div {
    let t = cur();
    div()
        .w_full()
        .h(px(m.pitch))
        .overflow_hidden()
        .child(
            div()
                .w_full()
                .h(px(m.pitch - 2.0))
                .rounded(px(crate::ui::theme::R_SM))
                // 淡色带 0.35：比悬停行更弱，只作视觉分隔不作交互暗示
                .bg(faded(t.row_hover, 0.35))
                .flex()
                .items_center()
                .pl(px(7.0))
                .child(widgets::section(title)),
        )
}

/// 侧边栏交易对行：整行是一张卡片——星标在卡片最前，名称紧随其后，
/// 第二行（实时价格 + 24h 涨跌幅，无行情时占位）缩进与名称对齐。
/// 点击卡片任意位置即切换交易对；星标点击单独拦截为加/取自选。
/// 行槽（pitch）固定高、卡片（band）占其中扣除行间留白的部分——
/// 字号放大挤高时由 overflow 遮挡裁剪，不把相邻行顶歪
fn sym_row(
    app: &BnApp,
    weak: &WeakEntity<BnApp>,
    sym: &str,
    is_fav: bool,
    dp: usize,
    i: usize,
    m: Metrics,
) -> AnyElement {
    let t = cur();
    let selected = sym == app.page.symbol;
    let sym_owned: SharedString = SharedString::from(sym.to_string());
    let star_glyph: SharedString = if is_fav { "★".into() } else { "☆".into() };
    let star_tip = if is_fav {
        crate::i18n::tr("取消自选").to_string()
    } else {
        crate::i18n::tr("加入自选").to_string()
    };

    // ---- 第二行：实时价格 + 24h 涨跌幅（价格与涨跌幅同色，方向一眼可辨）----
    let (price_txt, chg_txt, px_color): (SharedString, SharedString, Hsla) =
        match app.state.ticker_of(app.market, sym) {
            Some(tk) => {
                // 不附千分位分组：侧栏紧凑场景需要纯粹的价格数字
                let chg = widgets::f64_decimal(&tk.chg_pct());
                let c = theme::signed_color(chg);
                (
                    widgets::fmt_dec_plain(&tk.close, dp).into(),
                    format!("{chg:+.2}%").into(),
                    Hsla::from(c),
                )
            }
            None => {
                // 无行情时占住第二行槽位，避免行高不一致造成列表跳动
                ("—".into(), String::new().into(), Hsla::from(t.text_dim))
            }
        };

    // ---- 卡片 ----
    let card = div()
        .id(ElementId::from(("symrow", i)))
        .relative()
        .w_full()
        .h(px(m.band))
        .rounded(px(crate::ui::theme::R_SM))
        .flex()
        .flex_col()
        .pt(px(CARD_PAD))
        .pb(px(CARD_PAD))
        .gap(px(LINE_GAP))
        .overflow_hidden()
        .cursor_pointer()
        .when(selected, |d| d.bg(t.row_hover))
        .when(!selected, |d| d.bg(t.card).hover(|s| s.bg(t.row_hover)))
        // 选中行的 3px accent 条压在卡片最左
        .when(selected, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(3.0))
                    .rounded(px(2.0))
                    .bg(t.accent),
            )
        })
        // ---- 第一行：星标 + 名称 ----
        .child(
            div()
                .h(px(m.l1))
                .flex()
                .flex_row()
                .items_center()
                .pl(px(6.0))
                .child(
                    div()
                        .id(ElementId::from(("symstar", i)))
                        .w(px(STAR_W))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .mr(px(4.0))
                        .text_size(font_md())
                        .text_color(if is_fav { t.accent } else { t.text_dim })
                        .cursor_pointer()
                        .hover(|s| s.bg(faded(t.row_hover, 0.6)))
                        .child(star_glyph)
                        // 星标命中时不让同一次点击冒泡成「切换交易对」
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_click({
                            let weak = weak.clone();
                            let sym = sym_owned.clone();
                            move |_: &ClickEvent, _, cx| {
                                let _ = weak.update(cx, |app: &mut BnApp, cx| {
                                    app.state.toggle_favorite(app.market, &sym);
                                    cx.notify();
                                });
                            }
                        })
                        .tooltip({
                            let star_tip = star_tip.clone();
                            move |_, cx| widgets::tooltip_view(star_tip.clone(), cx)
                        }),
                )
                // 名称：纯展示（等宽 token，与下一行价格同体系），点击切换
                // 由整卡热区负责；宽度不足时按边界裁剪（遮挡优于换行顶行高）
                .child(
                    div()
                        .flex_1()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .font_family(FONT_MONO)
                        .text_size(font_md())
                        .text_color(if selected { t.accent } else { t.text })
                        .when(selected, |d| d.font_weight(FontWeight::MEDIUM))
                        .child(sym_owned.clone()),
                ),
        )
        // ---- 第二行：实时价格 + 涨跌幅 ----
        .child(
            div()
                .h(px(m.l2))
                .flex()
                .flex_row()
                .items_center()
                .pl(px(NAME_X))
                .overflow_hidden()
                .font_family(FONT_MONO)
                .text_size(font_sm())
                .text_color(px_color)
                .whitespace_nowrap()
                .child(price_txt)
                .child(div().flex_1())
                .child(chg_txt)
                .child(div().w(px(CARD_RIGHT_PAD))),
        )
        // 整卡点击 = 切换交易对（星标已在 on_mouse_down 阶段拦截传播）
        .on_click({
            let weak = weak.clone();
            let sym = sym_owned.clone();
            move |_: &ClickEvent, _, cx| {
                let _ = weak.update(cx, |app: &mut BnApp, cx| {
                    app.switch_symbol(sym.to_string());
                    cx.notify();
                });
            }
        })
        // 交易对全名进悬停提示：长名被裁剪时信息不丢
        .tooltip({
            let sym = sym_owned.clone();
            move |_, cx| widgets::tooltip_view(sym.to_string(), cx)
        });

    // 槽位 div：固定 pitch 高，底部 ROW_GAP 留白不被卡片色带覆盖
    div()
        .w_full()
        .h(px(m.pitch))
        .overflow_hidden()
        .child(card)
        .into_any_element()
}
