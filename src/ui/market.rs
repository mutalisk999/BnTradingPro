//! 盘口（深度）与逐笔成交列表
//!
//! 锁纪律：md 是全系统最热的锁（depth20@100ms / aggTrade 每条 WS 消息都要写），
//! 这里统一"短锁拷贝 → 释放 → 绘制"，绝不在面板构建期间持有锁。
//!
//! 常量、页签枚举与盘口动画纯状态（BookAnim）自里程碑 1 原样迁移；
//! 面板渲染（页签 / 盘口行 / 价差 / 档位胶囊 / 成交明细 / 页头统计）为 GPUI 版。

use crate::app::BnApp;
use crate::ui::flash::{self, BookSite};
use crate::ui::theme::{
    self, cur, faded, font_body, font_lg, font_md, font_sm, FONT_MONO,
};
use crate::ui::widgets;
use gpui::{
    div, prelude::FluentBuilder, px, ClickEvent, Context, Div, ElementId, Font, FontWeight,
    Hsla, InteractiveElement, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    TextRun, Window,
};
use rust_decimal::Decimal;
use std::sync::Arc;

/// 每侧显示档数下限（面板过矮时的保底）。按用户要求每侧再增一档
pub const BOOK_LEVELS: usize = 12;
/// 每侧显示档数上限：与订阅的部分深度流档数一致（@depth20 推满 20 档）。
/// 上限再大也没有数据——take(n) 只是取到原始推送为止，面板反而出现
/// 半幅留白；档位数随面板高度在此区间自适应，行高微调（±<1pt）填满面板
pub const BOOK_MAX_LEVELS: usize = 20;

/// 盘口行闪光/残影强度：取全局 FLASH_ALPHA 的一半——盘口是整列大量行同时
/// 更新，0.28 的整行底色在高频推送下连成整片红/绿，压到一半后只剩
/// "这一格刚变化"的提示意味，不再抢占视线。深度条颜色不走此值
pub const BOOK_FLASH_ALPHA: f32 = theme::FLASH_ALPHA * 0.5;

/// 右栏内容页签：盘口 / 成交明细
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookTab {
    /// 盘口（档位随面板高度自适应，12~20 档，行高微调填满面板）
    Book,
    /// 成交明细
    Trades,
}

/// 「档位」胶囊条的固定占位高度（含上方分隔线）：盘口的自适应档数须按此
/// 扣除，胶囊永远钉在盘口下方、不被顶出可视区
pub const BOOK_GROUP_H: f32 = 36.0;

/// 已消失盘口档位的残影
#[derive(Debug, Clone, Copy)]
pub struct BookGhost {
    /// true = 卖盘，false = 买盘
    pub is_ask: bool,
    /// 显示行槽位（0 = 该侧块顶部行）
    pub slot: usize,
    /// 消失时的深度条占比
    pub ratio: f64,
    /// 消失时刻（单调时钟秒域；GPUI 版由调用方传 Instant 基准的秒数）
    pub t0: f64,
}

/// 盘口动画状态：上一帧价格集合 + 消失档位残影（跨帧存于 BnApp，缓冲区复用零堆分配）
#[derive(Debug, Clone, Default)]
pub struct BookAnim {
    prev_ask: Vec<(Decimal, f64)>,
    prev_bid: Vec<(Decimal, f64)>,
    ghosts: Vec<BookGhost>,
}

impl BookAnim {
    /// 清空（切换交易对/市场时调用，避免跨符号误判）
    pub fn clear(&mut self) {
        self.prev_ask.clear();
        self.prev_bid.clear();
        self.ghosts.clear();
    }

    /// 当前未过期的残影列表（渲染层绘制用）
    pub fn ghosts(&self) -> &[BookGhost] {
        &self.ghosts
    }

    /// 与上一帧 diff：消失的档位记为残影，过期残影清除。
    /// `now` 取单调时钟秒（`Instant::now()` 相对某基准的秒数），同一时间域
    /// 内与 GHOST_SECS 比较即可；egui 版取内部秒表，语义一致
    pub fn update(
        &mut self,
        now: f64,
        is_ask: bool,
        rows: &[(Decimal, Decimal)],
        cums: &[f64],
        max_cum: f64,
    ) {
        let mut prev = std::mem::take(if is_ask {
            &mut self.prev_ask
        } else {
            &mut self.prev_bid
        });
        self.ghosts.retain(|g| now - g.t0 < theme::GHOST_SECS);
        for (p, ratio) in &prev {
            // 仍在当前帧的档位不是残影
            if rows.iter().any(|(cp, _)| cp == p) {
                continue;
            }
            // 消失档位在当前升序列中的排序位 → 显示槽位
            let idx = rows.partition_point(|(cp, _)| *cp < *p);
            let slot = if is_ask {
                // ask 显示倒序：slot = len-1-idx；idx==len（远端消失）时无槽位
                rows.len().checked_sub(1 + idx)
            } else {
                // bid 显示正序；idx==len（远端消失）时无槽位
                (idx < rows.len()).then_some(idx)
            };
            let Some(slot) = slot else { continue };
            if self.ghosts.len() >= 8 {
                self.ghosts.remove(0);
            }
            self.ghosts.push(BookGhost {
                is_ask,
                slot,
                ratio: *ratio,
                t0: now,
            });
        }
        // 记录本帧（价格, 深度占比），供下一帧 diff；复用缓冲区
        prev.clear();
        for (i, (p, _)) in rows.iter().enumerate() {
            prev.push((*p, cums[i] / max_cum));
        }
        if is_ask {
            self.prev_ask = prev;
        } else {
            self.prev_bid = prev;
        }
    }
}

// ---------- 渲染 ----------

/// 单调时钟秒域（BookGhost.t0 基准）：进程内静态基准 Instant 起的流逝秒数。
/// egui 版取 `ctx.input(|i| i.time)`，GPUI 无对应 API，单调秒语义一致
fn now_secs() -> f64 {
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let base = BASE.get_or_init(std::time::Instant::now);
    base.elapsed().as_secs_f64()
}

/// 文本实宽测量（成交明细列宽 / 档位胶囊统一槽宽 / 下单类型菜单宽用）：
/// 单行排布取宽度。egui 版用 painter.layout_no_wrap；GPUI 走 text_system().shape_line
pub(crate) fn measure(text: &str, size: f32, mono: bool, window: &Window) -> f32 {
    let font = Font {
        family: if mono { FONT_MONO } else { theme::FONT_UI }.into(),
        ..gpui::font("")
    };
    let run = TextRun {
        len: text.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(size),
        &[run],
        None,
    );
    f32::from(line.width)
}

/// 数据缺位占位：居中提示（加载中 / 等待深度）
fn loading_slot(msg: &str) -> Div {
    div()
        .w_full()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .child(widgets::loading_state(msg))
}

/// 右栏：页签 + 内容双页签，互斥显示。盘口页 = 页签 / 盘口 /
/// 底部钉住的档位胶囊；成交明细页行数随面板高度自适应。
/// `avail_h` = 页签与分隔线之下的内容高度预算，`avail_w` = 面板内容宽度
/// （由 app.rs 按视口与面板几何预算，GPUI 布局前无法询问剩余空间）
pub fn book_panel(
    app: &mut BnApp,
    window: &mut Window,
    cx: &mut Context<BnApp>,
    avail_h: f32,
    avail_w: f32,
) -> Div {
    let t = cur();
    // 页签区两等分：左右半区各放一枚页签（水平垂直居中），中缝一条
    // 竖向分隔线（上下各留 4px 不顶满）
    let tabs = div()
        .w_full()
        .h(px(theme::BTN_H))
        .flex()
        .flex_row()
        .child(
            widgets::pill_tab_rect(
                "booktab_book",
                crate::i18n::tr("盘口"),
                app.book_tab == BookTab::Book,
            )
            .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                set_book_tab(app, BookTab::Book);
                cx.notify();
            })),
        )
        .child(div().w(px(1.0)).my(px(theme::SPACE_SM)).bg(t.border))
        .child(
            widgets::pill_tab_rect(
                "booktab_trades",
                crate::i18n::tr("成交明细"),
                app.book_tab == BookTab::Trades,
            )
            .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                set_book_tab(app, BookTab::Trades);
                cx.notify();
            })),
        );
    let mut panel = div()
        // flex_1 占满右栏剩余高度：无论盘口当前有多少档，底部档位胶囊条
        // 都钉在面板最下缘，不随档位行数减少而上浮
        .w_full()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(tabs)
        .child(div().w_full().h(px(1.0)).my(px(theme::SPACE_SM)).bg(faded(t.border, 0.7)));
    match app.book_tab {
        BookTab::Trades => panel = panel.child(
            show_trades(app, window, cx, avail_h, avail_w)
                .flex_1()
                .min_h(px(0.0)),
        ),
        BookTab::Book => {
            // 先扣除底部胶囊条的固定高度，盘口在剩余高度内做档数自适应；
            // 不扣除会把胶囊顶出面板底缘
            let book_h = (avail_h - BOOK_GROUP_H).max(0.0);
            panel = panel
                .child(
                    show_book(app, window, cx, book_h, avail_w)
                        .flex_1()
                        .min_h(px(0.0)),
                )
                .child(show_book_group(window, app, cx));
        }
    }
    panel
}

/// 切换页签并重置闪光/残影基线——旧页的「上一帧」属于旧内容，切回时
/// 不应把缺失时段累积的价格变化整列闪一遍
fn set_book_tab(app: &mut BnApp, tab: BookTab) {
    if app.book_tab != tab {
        app.book_tab = tab;
        app.flash.book = Default::default();
        app.flash.trade = Default::default();
        app.book_anim.clear();
    }
}

/// 盘口面板：档位随高度自适应（12~20 档），行高微调填满面板；
/// 点击价格 → 填入下单价格
fn show_book(
    app: &mut BnApp,
    _window: &mut Window,
    cx: &mut Context<BnApp>,
    avail_h: f32,
    avail_w: f32,
) -> Div {
    let t = cur();
    let state = Arc::clone(&app.state);
    let dp = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.price_dp() as usize)
        })
        .unwrap_or(2);
    let qdp = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.qty_dp() as usize)
        })
        .unwrap_or(4);

    // 行距收紧为无缝 + 档数 n 取「最接近可用高度」的整数（四舍五入），
    // 余数由行高分摊：行高在基准 17pt 上下微调（±<1pt），面板被恰好填满，
    // 不再在卖盘顶部或买盘底部留下整条空档
    const SPREAD_H: f32 = 24.0;
    const SPREAD_PAD: f32 = 8.0; // 价差行上下呼吸
    const BOOK_ROW_H: f32 = 17.0;
    const BOOK_ROW_H_MIN: f32 = BOOK_ROW_H * 0.75; // 面板过矮（n 触底下限）时允许压缩
    const BOOK_ROW_H_MAX: f32 = BOOK_ROW_H * 1.35; // n 触顶 20 档时允许拉伸
    let budget = (avail_h - SPREAD_H - SPREAD_PAD).max(2.0 * BOOK_ROW_H_MIN);
    let n = ((budget / (2.0 * BOOK_ROW_H)).round() as usize).clamp(BOOK_LEVELS, BOOK_MAX_LEVELS);
    let row_h = (budget / (2.0 * n as f32)).clamp(BOOK_ROW_H_MIN, BOOK_ROW_H_MAX);

    // 聚合档位步长：tick 的 1/10/100/1000 倍（0 = 不聚合）
    let group_step = {
        let steps_tick = [1u32, 10, 100, 1000];
        let idx = app.page.book_group.min(steps_tick.len() - 1);
        let tick = state.with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.tick).unwrap_or_default()
        });
        if idx == 0 {
            Decimal::ZERO
        } else {
            tick * Decimal::from(steps_tick[idx])
        }
    };

    // 短锁拷贝（价格, 数量）。开启聚合时取全部原始档位按步长分桶：
    // bids 向下取整、asks 向上取整到桶边界，同桶数量合并（桶价与
    // Binance 深度聚合一致）；桶数少于 n 时面板留白
    let (asks, bids) = {
        let take = |side: &Vec<crate::binance::model::Level>, up: bool| -> Vec<(Decimal, Decimal)> {
            let raw: Vec<(Decimal, Decimal)> =
                side.iter().map(|l| (l.price, l.qty)).collect();
            if group_step.is_zero() {
                return raw.into_iter().take(n).collect();
            }
            let mut merged: Vec<(Decimal, Decimal)> = Vec::with_capacity(raw.len());
            for (p, q) in raw {
                let buckets = p / group_step;
                let b = (if up { buckets.ceil() } else { buckets.floor() }) * group_step;
                if let Some(last) = merged.last_mut() {
                    if last.0 == b {
                        last.1 += q;
                        continue;
                    }
                }
                merged.push((b, q));
            }
            merged.into_iter().take(n).collect()
        };
        let md = state.md.lock();
        let Some(md) = md.get(app.market, &app.page.symbol) else {
            return loading_slot(crate::i18n::tr("加载中…"));
        };
        if md.depth.asks.is_empty() && md.depth.bids.is_empty() {
            return loading_slot(crate::i18n::tr("等待深度数据…"));
        }
        (take(&md.depth.asks, true), take(&md.depth.bids, false))
    };

    // 累计量用于深度条：从价差向外累积（显示行自上而下为最远档 → 最近档）
    let cum_of = |rows: &[(Decimal, Decimal)]| -> (Vec<f64>, f64) {
        let mut cum = 0.0f64;
        let cums = rows
            .iter()
            .map(|(_, q)| {
                cum += widgets::f64_decimal(q);
                cum
            })
            .collect();
        (cums, cum)
    };
    let (ask_cums, ask_total) = cum_of(&asks);
    let (bid_cums, bid_total) = cum_of(&bids);
    let max_cum = ask_total.max(bid_total).max(0.000001);
    let now = now_secs();

    // 残影更新：与上一帧价格集合 diff，消失的档位记为淡出残影
    app.book_anim.update(now, true, &asks, &ask_cums, max_cum);
    app.book_anim.update(now, false, &bids, &bid_cums, max_cum);

    // 残影按显示槽位归位：ask 显示倒序，槽位 s ↔ 显示位置 s（0 = 顶部行），
    // bid 正序同理；槽位超出当前行数（档位减少）时自然跳过
    let mut ask_ghosts: Vec<Option<(f64, f32)>> = vec![None; asks.len()];
    let mut bid_ghosts: Vec<Option<(f64, f32)>> = vec![None; bids.len()];
    for g in app.book_anim.ghosts() {
        let age = now - g.t0;
        if !(0.0..theme::GHOST_SECS).contains(&age) {
            continue;
        }
        let k = (1.0 - age / theme::GHOST_SECS) as f32;
        if g.is_ask {
            if g.slot < ask_ghosts.len() {
                ask_ghosts[g.slot] = Some((g.ratio, k));
            }
        } else if g.slot < bid_ghosts.len() {
            bid_ghosts[g.slot] = Some((g.ratio, k));
        }
    }

    let mut col = div().w_full().flex().flex_col();
    // 残余留白（仅 n 触顶/触底时存在）一分为二：一半顶到卖盘上方，
    // 另一半自然沉到买盘下方
    let gap = (budget - 2.0 * n as f32 * row_h).max(0.0) / 2.0;
    if gap > 0.5 {
        col = col.child(div().w_full().h(px(gap)));
    }
    // 卖盘（高→低）：asks 升序（最近档在前），显示时倒序遍历，
    // 两侧最优价都贴着价差行
    let bar_ask = faded(t.down, 0.28);
    for i in (0..asks.len()).rev() {
        let disp = asks.len() - 1 - i;
        col = col.child(book_row(
            app,
            cx,
            row_h,
            avail_w,
            ask_cums[i] / max_cum,
            bar_ask,
            &asks[i].0,
            dp,
            widgets::f64_decimal(&asks[i].1),
            qdp,
            t.down,
            BookSite::AskPrice,
            i,
            "bkask",
            ask_ghosts[disp],
        ));
    }

    // 价差行（单边盘口时直接显示可得一侧价格，避免中间价=半价的错误数值）
    let best_ask = asks.first().map(|(p, _)| *p);
    let best_bid = bids.first().map(|(p, _)| *p);
    let (mid, spread) = match (best_bid, best_ask) {
        (Some(b), Some(a)) => ((b + a) / Decimal::from(2), a - b),
        (Some(only), None) | (None, Some(only)) => (only, Decimal::ZERO),
        (None, None) => (Decimal::ZERO, Decimal::ZERO),
    };
    let mid_f = widgets::f64_decimal(&mid);
    let spread_f = widgets::f64_decimal(&spread);
    let spread_pct = if mid_f > 0.0 { spread_f / mid_f * 100.0 } else { 0.0 };
    let one_sided = best_bid.is_none() || best_ask.is_none();
    let spread_txt = if spread.is_zero() && one_sided {
        String::new()
    } else {
        format!("{:.*} ({:.3}%)", dp, spread_f, spread_pct)
    };
    col = col.child(div().h(px(SPREAD_PAD / 2.0)));
    col = col.child(
        div()
            .w_full()
            .h(px(SPREAD_H))
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden()
            .px(px(theme::SPACE_XS))
            // 价差行中间价：与页头最新价同一「大数字」语义，700 字重
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(font_body())
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .whitespace_nowrap()
                    .child(format!("{:.*}", dp, mid_f)),
            )
            .child(div().flex_1())
            // 数值走等宽 + 正文色（信息数据不用暗色），"价差"标签才是辅助暗字
            .when(!spread_txt.is_empty(), |d| {
                d.child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(font_sm())
                        .text_color(t.text)
                        .whitespace_nowrap()
                        .child(spread_txt),
                )
            })
            .child(
                div()
                    .text_size(font_sm())
                    .text_color(t.text_dim)
                    .ml(px(theme::SPACE_SM))
                    .child(crate::i18n::tr("价差").to_string()),
            ),
    );
    col = col.child(div().h(px(SPREAD_PAD / 2.0)));

    // 买盘（低→高）
    let bar_bid = faded(t.up, 0.28);
    for i in 0..bids.len() {
        col = col.child(book_row(
            app,
            cx,
            row_h,
            avail_w,
            bid_cums[i] / max_cum,
            bar_bid,
            &bids[i].0,
            dp,
            widgets::f64_decimal(&bids[i].1),
            qdp,
            t.up,
            BookSite::BidPrice,
            i,
            "bkbid",
            bid_ghosts[i],
        ));
    }
    col
}

/// 盘口单行：悬停/闪烁底色 + 右锚定深度条 + 价格（左）/数量（右），点击价格填单。
/// 图层自下而上：闪烁底色 → 深度条 → 残影 → 交互层（悬停高亮/点击/提示）→ 文字
#[allow(clippy::too_many_arguments)]
fn book_row(
    app: &mut BnApp,
    cx: &mut Context<BnApp>,
    row_h: f32,
    w: f32,
    ratio: f64,
    bar_color: Hsla,
    price: &Decimal,
    dp: usize,
    qty_f: f64,
    qdp: usize,
    price_color: gpui::Rgba,
    site: BookSite,
    idx: usize,
    tag: &'static str,
    ghost: Option<(f64, f32)>,
) -> Div {
    let t = cur();
    // 显示用 f64；点击填单必须用 Decimal 原文——dp 回退（规则未加载时=2）
    // 会把 0.00001234 这类价格四舍五入成 "0.00"
    let price_f = widgets::f64_decimal(price);
    // 价格/数量变动高亮：500ms 内行底色渐隐（先于深度条绘制，压在其下层）。
    // 颜色固定用行方向色（卖盘跌色/买盘涨色），不按数值涨跌翻转：
    // 位点按显示行观测，吃单使整列价格平移时"涨跌"判定会系统性反向
    // （卖盘价格整体上移→误闪涨色），故与行静态颜色保持一致
    let (p_int, _) = app.flash.book(site, idx, price_f);
    let (q_int, _) = app.flash.book(site.qty_site(), idx, qty_f);
    let intensity = p_int.max(q_int);

    let mut row = div().relative().w_full().h(px(row_h)).overflow_hidden();
    if intensity > 0.0 {
        row = row.child(
            div()
                .absolute()
                .inset_0()
                .bg(faded(price_color, BOOK_FLASH_ALPHA * intensity)),
        );
    }
    let bar_w = (w as f64 * ratio.clamp(0.0, 1.0)) as f32;
    if bar_w > 0.5 {
        row = row.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(bar_w))
                .bg(bar_color),
        );
    }
    // 残影淡出：在消失档位的显示槽位上叠加渐隐底色与深度条。
    // 不绘制文字（该槽位现已被平移后的新档位占据，文字会重叠花屏）
    if let Some((g_ratio, k)) = ghost {
        let base = if matches!(site, BookSite::AskPrice | BookSite::AskQty) {
            t.down
        } else {
            t.up
        };
        row = row.child(
            div()
                .absolute()
                .inset_0()
                .bg(faded(base, BOOK_FLASH_ALPHA * k * 0.7)),
        );
        let bw = (w as f64 * g_ratio.clamp(0.0, 1.0)) as f32;
        if bw > 0.5 {
            row = row.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(bw))
                    .bg(faded(base, BOOK_FLASH_ALPHA * k)),
            );
        }
    }
    // 交互层：悬停高亮画在深度条之上（右半区悬停反馈不被半透明深度条冲淡），
    // 文字在其后再画一层保证可读；点击价格填入下单表单
    let price_str = price.to_string();
    row = row.child(
        div()
            .id(ElementId::from((tag, idx)))
            .absolute()
            .inset_0()
            .cursor_pointer()
            .hover(|s| s.bg(t.row_hover))
            .on_click(cx.listener(move |app: &mut BnApp, _: &ClickEvent, _, cx| {
                // 点击填单用交易所精度原文（Decimal），不能用 UI 四舍五入后的值
                app.page.form.set_price(&price_str);
                cx.notify();
            }))
            .tooltip(move |_, cx| {
                widgets::tooltip_view(crate::i18n::tr("点击填入下单价格").to_string(), cx)
            }),
    );
    // 文字层（最上层）：价格左 / 数量右，宽度不足时按边界裁剪
    row.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h_full()
            .child(
                div()
                    .pl(px(theme::SPACE_XS))
                    .font_family(FONT_MONO)
                    .text_size(font_md())
                    .text_color(Hsla::from(price_color))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(format!("{:.*}", dp, price_f)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .pr(px(theme::SPACE_XS))
                    .font_family(FONT_MONO)
                    .text_size(font_md())
                    .text_color(t.text)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(format!("{:.*}", qdp, qty_f)),
            ),
    )
}

/// 盘口聚合档位步长胶囊（Binance 同款）：tick 的 1/10/100/1000 倍，
/// 钉在盘口面板最下方。与页签行同样独立一行而非塞进页签行——面板窄时
/// 溢出会叠到「成交明细」页签上
fn show_book_group(window: &mut Window, app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let t = cur();
    // 上分隔线（边框）+ 固定高槽位：内容纵向居中
    let slot = div()
        .w_full()
        .h(px(BOOK_GROUP_H))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .border_t_1()
        .border_color(t.border);
    let tick = app.state.with_rule(app.market, &app.page.symbol, |r| {
        r.map(|x| x.tick).unwrap_or_default()
    });
    if tick.is_zero() {
        return slot;
    }
    let steps = [
        tick,
        tick * Decimal::from(10),
        tick * Decimal::from(100),
        tick * Decimal::from(1000),
    ];
    let group_idx = app.page.book_group.min(steps.len() - 1);
    // 等宽胶囊：四个步长标签自然宽不一（"0.1" vs "100"），按最宽标签
    // 统一槽宽，选中态同色块，视觉上才成一组协调的切换条
    let labels: Vec<String> = steps
        .iter()
        .map(|s| widgets::fmt_dec_fixed(s, s.normalize().scale() as usize))
        .collect();
    let widest = labels
        .iter()
        .map(|l| measure(l, f32::from(font_sm()), true, window))
        .fold(0.0f32, f32::max);
    let btn_w = widest + 12.0;
    let mut group = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_SM))
        .child(
            div()
                .text_size(font_sm())
                .text_color(t.text_dim)
                .whitespace_nowrap()
                .child(crate::i18n::tr("档位").to_string()),
        );
    for (i, label) in labels.iter().enumerate() {
        let selected = i == group_idx;
        group = group.child(
            div()
                .id(ElementId::from(("bkg", i)))
                .w(px(btn_w))
                .h(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::R_SM))
                .text_size(font_sm())
                .whitespace_nowrap()
                // 数值标签用等宽字体：四档数字小数点纵向成列
                .font_family(theme::FONT_MONO)
                .text_color(if selected { t.accent } else { t.text_dim })
                // 选中态 accent 淡底：与有效期 chips / 页签同一套选中语言
                .when(selected, |d| d.bg(faded(t.accent, theme::TINT_FILL)))
                .when(!selected, |d| d.hover(|s| s.bg(t.row_hover)))
                .cursor_pointer()
                .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                    if app.page.book_group != i {
                        app.page.book_group = i;
                        // 桶价与原始价不同源：切换聚合后闪光/残影基线一并重置
                        app.flash.book = Default::default();
                        app.book_anim.clear();
                    }
                    cx.notify();
                }))
                .child(label.clone()),
        );
    }
    slot.child(group)
}

/// 逐笔成交：列宽 = 列内全部单元格实宽（含表头）统一测宽再绘制，同列所有
/// 单元格文字缘严格成列；窄面板时间降级（省略秒）→ 等比压缩列宽，绝不叠印
fn show_trades(
    app: &mut BnApp,
    window: &mut Window,
    _cx: &mut Context<BnApp>,
    avail_h: f32,
    avail_w: f32,
) -> Div {
    let t = cur();
    let state = Arc::clone(&app.state);
    // 精度与盘口同源：价格/数量小数位取交易对规则配置（规则未加载时回退 2/4）
    let dp = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.price_dp() as usize)
        })
        .unwrap_or(2);
    let qdp = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.qty_dp() as usize)
        })
        .unwrap_or(4);

    // 行数随面板高度自适应（同盘口思路：内容刚好占满），
    // 上限为行情缓冲的 100 笔；留 4px 余量防止取整溢出
    const TRADES_ROW_H: f32 = 17.0; // 数据行内容高（FONT_MD 文本行）
    const HEADER_H: f32 = 14.0; // 表头内容高（FONT_SM 文本行）
    let row_pitch = TRADES_ROW_H + 2.0;
    let n = (((avail_h - HEADER_H - 4.0) / row_pitch).floor() as usize).clamp(1, 100);

    // 短锁拷贝最近 n 笔后绘制
    let trades: Vec<crate::binance::model::TradeTick> = {
        let md = state.md.lock();
        let Some(md) = md.get(app.market, &app.page.symbol) else {
            return loading_slot(crate::i18n::tr("加载中…"));
        };
        md.trades.iter().rev().take(n).cloned().collect()
    };
    if trades.is_empty() {
        return div()
            .w_full()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(widgets::empty_state("◍", crate::i18n::tr("暂无成交"), ""));
    }

    // 表头/数据两套字体：表头小号正体，数据等宽正文
    let head_sz = f32::from(font_sm());
    let data_sz = f32::from(font_md());
    let mut w_price = measure(crate::i18n::tr("价格"), head_sz, false, window);
    let mut w_qty = measure(crate::i18n::tr("数量"), head_sz, false, window);
    let mut w_time = measure(crate::i18n::tr("时间"), head_sz, false, window);
    for tr in &trades {
        w_price = w_price.max(measure(
            &format!("{:.*}", dp, widgets::f64_decimal(&tr.price)),
            data_sz,
            true,
            window,
        ));
        w_qty = w_qty.max(measure(
            &format!("{:.*}", qdp, widgets::f64_decimal(&tr.qty)),
            data_sz,
            true,
            window,
        ));
        w_time = w_time.max(measure(&widgets::fmt_time_short(tr.time), data_sz, true, window));
    }
    // 窄面板时间降级：列宽绝不压到内容实宽以下（压缩会让文字向左溢出、
    // 与数量列叠印）。放不下完整 HH:MM:SS 时省略秒，再不够才等比压缩
    let tw_short = measure("00:00", data_sz, true, window);
    if w_price + w_qty + w_time + 20.0 > avail_w && w_price + w_qty + tw_short + 20.0 <= avail_w {
        w_time = tw_short;
    }
    let time_full = w_time > tw_short;
    // 列布局：价格贴左（与盘口行一致）、时间贴右、数量左对齐紧跟价格之后。
    // 两侧间距 = 剩余空间均分：价格↔数量 与 数量↔时间 永远等宽，视觉平衡。
    // 内宽不足时的压缩次序：价格列（信息核心，被裁即叠印）永远保自然宽，
    // 时间+数量两列等比收缩——数量列贴着价格，最先被挤压就会盖住价格
    // 小数尾数（遮挡优于叠印的既有取舍）
    const MIN_GAP: f32 = 8.0;
    let content = w_price + w_qty + w_time;
    if content + 4.0 + 2.0 * MIN_GAP > avail_w && avail_w > 0.0 {
        // 价格列（信息核心，被裁即叠印）保持自然宽，时间+数量两列按比例
        // 缩到放得下——不可把某列压到 0（整列消失/边缘宽度闪烁）
        let budget = (avail_w - 4.0 - 2.0 * MIN_GAP).max(w_price);
        let shrink_total = (budget - w_price).max(0.0);
        let shrinkable = w_qty + w_time;
        if shrinkable > 0.0 {
            let k = (shrink_total / shrinkable).min(1.0);
            w_qty *= k;
            w_time *= k;
        }
    }
    let gap = ((avail_w - 4.0 - w_price - w_qty - w_time) * 0.5).max(MIN_GAP);

    // 行构建器：表头与数据行共用同一组锚位；行距 2pt 由列表 gap 提供
    let mk_row = move |price: String,
                       qty: String,
                       time: String,
                       h: f32,
                       color: Hsla,
                       head: bool|
            -> Div {
        let (sz, mono): (f32, Option<SharedString>) = if head {
            (head_sz, None)
        } else {
            (data_sz, Some(FONT_MONO.into()))
        };
        let base = div()
            .w_full()
            .h(px(h))
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden();
        let base = match mono {
            Some(f) => base.font_family(f),
            None => base,
        };
        base.child(
            div()
                .pl(px(theme::SPACE_XS))
                // 宽 = 文字实宽 + 左内边距：GPUI 固定宽按 border-box 解释
                // （taffy 默认 BorderBox），定宽 w_price 会让 2px 内边距吃进
                // 文字区，价格尾数被裁——视觉上像被数量列挡住。内边距必须
                // 落在测量宽之外
                .w(px(w_price + theme::SPACE_XS))
                // 价格列禁收缩：行内总宽超预算时 taffy 会按比例压缩各固定宽
                // 子项，价格小数尾数最先被数量列盖住（遮挡优于叠印的取舍）
                .flex_none()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(px(sz))
                .text_color(color)
                .child(price),
        )
        .child(div().w(px(gap)))
        .child(
            div()
                .w(px(w_qty))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(px(sz))
                .text_color(if head { t.text_dim } else { t.text })
                .child(qty),
        )
        .child(div().flex_1())
        .child(
            div()
                .pr(px(theme::SPACE_XS))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(px(sz))
                .text_color(t.text_dim)
                .child(time),
        )
    };

    let mut list = div().w_full().flex().flex_col().gap(px(theme::SPACE_XS));
    list = list.child(mk_row(
        crate::i18n::tr("价格").to_string(),
        crate::i18n::tr("数量").to_string(),
        crate::i18n::tr("时间").to_string(),
        HEADER_H,
        Hsla::from(t.text_dim),
        true,
    ));
    for (row, tr) in trades.iter().enumerate() {
        // 买方主动（buyer_maker=true）→ 主动卖单 → 跌色；反之为涨色。
        // 与盘口涨跌色同源（theme::trade_color），随 price_dir 翻转
        let mut col = theme::trade_color(tr.buyer_maker);
        // 最新一行（row 0）：新成交到达时文字向主题文字色短暂提亮后渐回原色。
        // 不混入行底色，混色会红绿互相污染
        if row == 0 {
            let (intensity, _) = app.flash.trade_top(widgets::f64_decimal(&tr.price));
            if intensity > 0.0 {
                col = theme::lerp_color(col, t.text, intensity * theme::FLASH_BRIGHTEN);
            }
        }
        let ts = widgets::fmt_time_short(tr.time);
        // 窄面板时间降级：省略秒（HH:MM），与列宽决策一致。
        // 越界时间戳 fmt_time_short 返回 "—"：用 get 而非固定切片，
        // "—".len() < 5 时固定切片会 byte-index panic 整窗崩溃
        let time_txt = if time_full {
            ts.clone()
        } else {
            ts.get(..5).unwrap_or(&ts).to_string()
        };
        list = list.child(mk_row(
            // 不做千分位分组，与盘口一致按规则精度定长显示，纵向小数点对齐
            format!("{:.*}", dp, widgets::f64_decimal(&tr.price)),
            format!("{:.*}", qdp, widgets::f64_decimal(&tr.qty)),
            time_txt,
            TRADES_ROW_H,
            Hsla::from(col),
            false,
        ));
    }
    list
}

/// 页头：交易对 / 最新价与涨跌 / 24h 统计（合约附资金费率）。
/// 周期直选在图表工具行（chart.rs，里程碑 6），页头只保留行情信息
/// 内容自适应高度，此常量是典型行情下的近似实测值（预警浮层锚定用）
pub const MARKET_HEADER_H: f32 = 47.0;

pub fn header(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let t = cur();
    let state = Arc::clone(&app.state);
    let dp = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.price_dp() as usize)
        })
        .unwrap_or(2);
    let (base, quote) = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| (x.base.clone(), x.quote.clone()))
        })
        .unwrap_or((String::new(), String::new()));
    // 单次短锁取齐本帧所需的 ticker 与资金费率
    let (tick, funding, funding_at) = {
        let md = state.md.lock();
        match md.get(app.market, &app.page.symbol) {
            Some(md) => (
                md.ticker.clone(),
                md.mark.as_ref().and_then(|m| m.funding_rate),
                md.mark.as_ref().map(|m| m.next_funding_time),
            ),
            None => (None, None, None),
        }
    };

    let vsep = || {
        div()
            .w(px(1.0))
            .h(px(14.0))
            .bg(t.border)
            .flex_shrink_0()
    };
    // 统计项 = 紧凑 [标签 + 值] 横向对，格间细分隔线收束视线
    let stat = |label: &'static str, value: String, color: gpui::Rgba| -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::SPACE_SM))
            .child(widgets::card_label(label))
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(font_md())
                    .text_color(Hsla::from(color))
                    .whitespace_nowrap()
                    .child(value),
            )
    };

    // 第一行：交易对 | 最新价/涨跌 | 自选星标
    let sym_title = if !base.is_empty() && app.page.symbol.starts_with(&base) {
        // 交易对双色标题：base（标的）强调、quote（计价）弱化，
        // 与交易所惯例一致；规则未加载/结构不符时整体回退单色原文
        div()
            .flex()
            .flex_row()
            .items_center()
            .child(
                div()
                    .text_size(font_lg())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .whitespace_nowrap()
                    .child(base.clone()),
            )
            .child(
                div()
                    .text_size(font_lg())
                    .text_color(t.text_dim)
                    .whitespace_nowrap()
                    .child(app.page.symbol[base.len()..].to_string()),
            )
    } else {
        div()
            .text_size(font_lg())
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.text)
            .whitespace_nowrap()
            .child(app.page.symbol.clone())
    };
    let mut r1 = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_LG))
        .overflow_hidden()
        .child(sym_title)
        .child(vsep());
    if let Some(tk) = &tick {
        let chg = widgets::f64_decimal(&tk.price_change_percent);
        let mut c = theme::signed_color(chg);
        let px_f = widgets::f64_decimal(&tk.last_price);
        // 最新价变动闪烁：强度 1 → 0 期间把文字色向闪烁色（涨/跌）插值
        let (fi, fd) = app.flash.header_px(px_f);
        if fi > 0.0 {
            c = theme::lerp_color(c, flash::flash_color(fd), fi);
        }
        r1 = r1
            // 大价格走真字重：数字在等宽字体里有 700 字重才压得住一整行统计
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(font_lg())
                    .font_weight(FontWeight::BOLD)
                    .text_color(Hsla::from(c))
                    .whitespace_nowrap()
                    .child(widgets::fmt_thousands(px_f, dp)),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(font_body())
                    .text_color(Hsla::from(c))
                    .whitespace_nowrap()
                    .child(format!("{chg:+.2}%")),
            );
    }
    // 自选星标（价格预警入口随预警弹窗在里程碑 9 落地）
    let is_fav = app.state.favorites(app.market).contains(&app.page.symbol);
    r1 = r1.child(
        widgets::glyph_button(
            "hdr_star",
            if is_fav { "★" } else { "☆" },
            f32::from(font_body()),
            if is_fav { t.accent } else { t.text_dim },
            if is_fav {
                crate::i18n::tr("取消自选")
            } else {
                crate::i18n::tr("加入自选")
            },
        )
        .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
            app.state.toggle_favorite(app.market, &app.page.symbol);
            cx.notify();
        })),
    );

    // 预警入口：有预警或面板展开时点亮（本地触发，到价弹提示）
    let alert_n = app.state.cfg.read().alerts.len();
    let bell_txt = if alert_n > 0 {
        format!("{}({alert_n})", crate::i18n::tr("预警"))
    } else {
        crate::i18n::tr("预警").to_string()
    };
    r1 = r1.child(
        widgets::glyph_button(
            "hdr_bell",
            &bell_txt,
            f32::from(font_sm()),
            if alert_n > 0 || app.alerts_open {
                t.accent
            } else {
                t.text_dim
            },
            crate::i18n::tr("价格预警（本地触发，到价弹提示）"),
        )
        .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
            app.alerts_open = !app.alerts_open;
            cx.notify();
        })),
    );

    // 第三行：24h 开/高/低/收（行情四要素独占一行，价格动得最快、最常被扫读）
    let mut r3 = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_MD))
        .overflow_hidden();
    if let Some(tk) = &tick {
        let o = widgets::fmt_thousands(widgets::f64_decimal(&tk.open_price), dp);
        let h = widgets::fmt_thousands(widgets::f64_decimal(&tk.high_price), dp);
        let l = widgets::fmt_thousands(widgets::f64_decimal(&tk.low_price), dp);
        let c = widgets::fmt_thousands(widgets::f64_decimal(&tk.last_price), dp);
        r3 = r3
            .child(stat(crate::i18n::tr("24h 开"), o, t.text))
            .child(vsep())
            .child(stat(crate::i18n::tr("24h 高"), h, t.up))
            .child(vsep())
            .child(stat(crate::i18n::tr("24h 低"), l, t.down))
            .child(vsep())
            .child(stat(crate::i18n::tr("24h 收"), c, t.text));
    }

    // 第四行：24h 量/额（成交宏观指标），合约再追加资金费率 + 下次结算倒计时
    let mut r4 = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_MD))
        .overflow_hidden();
    if let Some(tk) = &tick {
        let base_txt = if base.is_empty() {
            String::new()
        } else {
            format!(" {base}")
        };
        let quote_txt = if quote.is_empty() {
            String::new()
        } else {
            format!(" {quote}")
        };
        r4 = r4
            .child(stat(
                crate::i18n::tr("24h 量"),
                format!(
                    "{}{base_txt}",
                    widgets::fmt_compact(widgets::f64_decimal(&tk.volume))
                ),
                t.text,
            ))
            .child(vsep())
            .child(stat(
                crate::i18n::tr("24h 额"),
                format!(
                    "{}{quote_txt}",
                    widgets::fmt_compact(widgets::f64_decimal(&tk.quote_volume))
                ),
                t.text,
            ));
    }
    // 合约：资金费率（正/负着色 + 显式符号）+ 下次结算倒计时
    if let (crate::binance::Market::UmFutures, Some(r)) = (app.market, funding) {
        // Binance 的 lastFundingRate 是名义价值比例（0.0001 = 0.01%），换算成百分比展示
        let rf = widgets::f64_decimal(&r) * 100.0;
        let color = if rf > 0.0 {
            t.up
        } else if rf < 0.0 {
            t.down
        } else {
            t.text_dim
        };
        let mut fund = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::SPACE_SM))
            .child(widgets::card_label(crate::i18n::tr("资金费率")))
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(font_md())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(Hsla::from(color))
                    .whitespace_nowrap()
                    .child(format!("{rf:+.4}%")),
            );
        // 倒计时：费率旁直接可见距离收/付资金费的剩余时间
        let cd = funding_at
            .filter(|&next| next > 0)
            .map(|next| {
                let left = (next - chrono::Utc::now().timestamp_millis()).max(0) / 1000;
                format!(" · {:02}:{:02}:{:02}", left / 3600, (left % 3600) / 60, left % 60)
            })
            .unwrap_or_default();
        if !cd.is_empty() {
            fund = fund.child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(font_md())
                    .text_color(t.text_dim)
                    .whitespace_nowrap()
                    .child(cd),
            );
        }
        r4 = r4.child(vsep()).child(fund);
    }

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .child(r1)
        .child(r3)
        .child(r4)
        .child(widgets::separator())
}

/// 添加行输入 → add_alert，回灌输入框：添加成功被清空、失败保留待改
fn add_alert_from_field(app: &mut BnApp, cx: &mut Context<BnApp>) {
    app.alert_input = app.alert_field.read(cx).text().to_string();
    app.add_alert();
    let cur = app.alert_input.clone();
    app.alert_field.update(cx, |f, _| f.set_text(cur));
    cx.notify();
}

/// 价格预警面板：顶部右侧悬挂（egui 版 RIGHT_TOP [-16, 92] 语义）。
/// 无遮罩背景：面板外点击直达底层 UI（与 egui 窗口可交互一致），Esc 关闭
pub fn alerts_dialog(app: &mut BnApp, cx: &mut Context<BnApp>) -> Option<Div> {
    if !app.alerts_open {
        return None;
    }
    let t = cur();
    let tr = crate::i18n::tr;
    let state = Arc::clone(&app.state);
    let dp = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.price_dp() as usize)
        })
        .unwrap_or(2);
    // 短锁取当前价显示（添加行的参考价）
    let last: Option<String> = {
        let md = state.md.lock();
        md.get(app.market, &app.page.symbol)
            .and_then(|m| m.ticker.clone())
            .map(|tk| widgets::fmt_dec_fixed(&tk.last_price, dp))
    };
    let alerts = state.cfg.read().alerts.clone();

    // 添加行：目标价（方向按相对最新价自动判定）
    let add_row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_MD))
        .child(div().w(px(110.0)).child(app.alert_field.clone()))
        .child(
            widgets::btn_default("alert_add", tr("添加"), true).on_click(cx.listener(
                |app: &mut BnApp, _, _, cx| add_alert_from_field(app, cx),
            )),
        );

    // 预警列表（≤180pt 高滚动；短锁拷贝 + with_rule 逐条取显示精度）
    let adp = |mkt: u8, sym: &str| -> usize {
        let m = if mkt == 0 {
            crate::binance::Market::Spot
        } else {
            crate::binance::Market::UmFutures
        };
        state.with_rule(m, sym, |r| {
            r.map(|x| x.price_dp() as usize).unwrap_or(2)
        })
    };
    let rows: Vec<Div> = alerts
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let px_txt = format!(
                "{} {}",
                if a.above { "≥" } else { "≤" },
                widgets::fmt_dec_fixed(&a.price, adp(a.market, &a.symbol))
            );
            let color = if a.above { t.up } else { t.down };
            let sym = a.symbol.clone();
            div()
                .h(px(24.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_SM))
                .child(
                    div()
                        .text_size(font_md())
                        .text_color(t.text)
                        .whitespace_nowrap()
                        .child(sym),
                )
                .when(a.market != 0, |d| {
                    d.child(
                        div()
                            .text_size(font_sm())
                            .text_color(t.text_dim)
                            .whitespace_nowrap()
                            .child(tr("· 合约")),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(font_md())
                        .text_color(Hsla::from(color))
                        .whitespace_nowrap()
                        .child(px_txt),
                )
                .child(
                    div()
                        .id(("alert_del", i))
                        .px(px(theme::SPACE_SM))
                        .text_size(font_sm())
                        .text_color(t.text_dim)
                        .cursor_pointer()
                        .hover(|s| s.text_color(t.err))
                        .child("×")
                        .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                            app.remove_alert(i);
                            cx.notify();
                        })),
                )
        })
        .collect();

    let list = if rows.is_empty() {
        div()
            .id("alerts_empty")
            .text_size(font_sm())
            .text_color(t.text_dim)
            .child(tr("暂无预警，输入目标价添加"))
    } else {
        div()
            .id("alerts_list")
            .max_h(px(180.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .children(rows)
    };

    let card = widgets::modal_card().w(px(272.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_MD))
                .child(
                    div()
                        .text_size(font_lg())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(tr("价格预警")),
                )
                .when_some(last.clone(), |d, v| {
                    d.child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(font_sm())
                            .text_color(t.text_dim)
                            .whitespace_nowrap()
                            .child(v),
                    )
                }),
        )
        .child(add_row)
        .child(widgets::separator())
        .child(list);

    // 锚定：顶栏 + 右栏页头高度（面板 p(8) + 页头内容 39）→ 页头正下方。
    // 卡片 occlude：面板悬浮在图表/盘口上方，不遮挡则点击穿透到底层元素
    Some(
        div()
            .absolute()
            .top(px(theme::TOPBAR_H + theme::SPACE_LG + MARKET_HEADER_H))
            .right(px(theme::SPACE_XXL))
            .child(card.occlude()),
    )
}
