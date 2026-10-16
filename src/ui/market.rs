//! 盘口（深度）与逐笔成交列表
//!
//! 锁纪律：md 是全系统最热的锁（depth20@100ms / aggTrade 每条 WS 消息都要写），
//! 这里统一"短锁拷贝 → 释放 → 绘制"，绝不在面板构建期间持有锁。

use crate::app::BnApp;
use crate::ui::flash;
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;
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
const BOOK_FLASH_ALPHA: f32 = theme::FLASH_ALPHA * 0.5;

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

/// 右栏：页签 + 内容双页签，互斥显示。盘口页 = 页签 / 盘口（滚动兜底）/
/// 底部钉住的档位胶囊；成交明细页行数已随面板高度自适应，直接绘制
pub fn show_book_panel(app: &mut BnApp, ui: &mut egui::Ui) {
    show_book_tabs(app, ui);
    match app.book_tab {
        BookTab::Trades => show_trades(app, ui),
        BookTab::Book => {
            // 先扣除底部胶囊条的固定高度，盘口在剩余高度内做档数自适应；
            // 不扣除会把胶囊顶出面板底缘（egui 单遍布局不知道后续控件）
            let body_h = (ui.available_height() - BOOK_GROUP_H).max(0.0);
            ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| {
                // 下限 12 档在极矮面板仍会溢出，保留滚动兜底
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| show_book(app, ui));
            });
            show_book_group(app, ui);
        }
    }
}

/// 页签行：盘口 / 成交明细。切页签会重置闪光/残影基线——旧页的
/// 「上一帧」属于旧内容，切回时不应把缺失时段累积的价格变化整列闪一遍
fn show_book_tabs(app: &mut BnApp, ui: &mut egui::Ui) {
    let prev = app.book_tab;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if theme::nav_tab(
            ui,
            crate::i18n::tr("盘口"),
            app.book_tab == BookTab::Book,
        ) {
            app.book_tab = BookTab::Book;
        }
        if theme::nav_tab(
            ui,
            crate::i18n::tr("成交明细"),
            app.book_tab == BookTab::Trades,
        ) {
            app.book_tab = BookTab::Trades;
        }
    });
    ui.separator();
    if app.book_tab != prev {
        app.flash_book = Default::default();
        app.flash_trade = Default::default();
        app.book_anim.clear();
    }
}

/// 盘口聚合档位步长胶囊（Binance 同款）：tick 的 1/10/100/1000 倍，
/// 钉在盘口面板最下方。与页签行同样独立一行而非塞进页签行——面板窄时
/// RTL 溢出会叠到「成交明细」页签上
pub fn show_book_group(app: &mut BnApp, ui: &mut egui::Ui) {
    // 上分隔线手绘 + 固定高槽位：ui.separator() 的占位含 item_spacing，
    // 与盘口扣除的 BOOK_GROUP_H 对不齐
    let cur = ui.cursor().min;
    ui.painter().line_segment(
        [cur, egui::pos2(cur.x + ui.available_width(), cur.y)],
        egui::Stroke::new(1.0, theme::cur().border),
    );
    ui.allocate_ui(egui::vec2(ui.available_width(), BOOK_GROUP_H), |ui| {
        ui.add_space(7.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let tick = app.state.with_rule(app.market, &app.page.symbol, |r| {
                r.map(|x| x.tick).unwrap_or_default()
            });
            if tick.is_zero() {
                return;
            }
            let steps = [
                tick,
                tick * Decimal::from(10),
                tick * Decimal::from(100),
                tick * Decimal::from(1000),
            ];
            let group_idx = app.page.book_group.min(steps.len() - 1);
            ui.label(
                egui::RichText::new(crate::i18n::tr("档位"))
                    .size(theme::FONT_SM)
                    .color(theme::cur().text_dim),
            );
            // 等宽胶囊：四个步长标签自然宽不一（"0.1" vs "100"），按最宽标签
            // 统一槽宽，选中态同色块，视觉上才成一组协调的切换条
            let labels: Vec<String> = steps
                .iter()
                .map(|s| widgets::fmt_dec_fixed(s, s.normalize().scale() as usize))
                .collect();
            let head = egui::FontId::proportional(theme::FONT_SM);
            let widest = labels
                .iter()
                .map(|t| {
                    ui.painter()
                        .layout_no_wrap(t.clone(), head.clone(), theme::cur().text)
                        .size()
                        .x
                })
                .fold(0.0f32, f32::max);
            let btn_w = widest + 12.0;
            for (i, label) in labels.iter().enumerate() {
                let selected = i == group_idx;
                let txt = egui::RichText::new(label)
                    .size(theme::FONT_SM)
                    .color(if selected {
                        theme::cur().accent
                    } else {
                        theme::cur().text_dim
                    });
                let btn = egui::Button::new(txt)
                    .fill(if selected {
                        theme::cur().row_hover
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(theme::R_SM)
                    .min_size(egui::vec2(btn_w, 20.0));
                if ui
                    .add(btn)
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    app.page.book_group = i;
                    // 桶价与原始价不同源：切换聚合后闪光/残影基线一并重置
                    app.flash_book = Default::default();
                    app.book_anim.clear();
                }
            }
        });
    });
}

/// 盘口面板：点击价格 → 填入下单价格
pub fn show_book(app: &mut BnApp, ui: &mut egui::Ui) {
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

    // 行距收紧为无缝 + 档位随面板高度自适应（同成交明细思路：内容恰好占满、
    // 无需滚动条）。上限 20 档 = depth20 推送的完整档数，下限 12 档保底。
    // 档数 n 取「最接近可用高度」的整数（四舍五入，而非向下取整），
    // 余数由行高分摊：行高在基准 17pt 上下微调（±<1pt），面板被恰好填满，
    // 不再在卖盘顶部或买盘底部留下整条空档
    ui.spacing_mut().item_spacing.y = 0.0;
    const SPREAD_H: f32 = 24.0;
    const SPREAD_PAD: f32 = 8.0; // 价差行上下呼吸
    // 基准行高与成交明细列表一致（同一字号的等宽行）；两侧各自再增一档后
    // 由行高吸收高度零头，视觉密度与成交明细保持对齐
    const ROW_H: f32 = 17.0;
    const ROW_H_MIN: f32 = ROW_H * 0.75; // 面板过矮（n 触底下限）时允许压缩
    const ROW_H_MAX: f32 = ROW_H * 1.35; // n 触顶 20 档时允许拉伸
    let avail = ui.available_height();
    let budget = (avail - SPREAD_H - SPREAD_PAD).max(2.0 * ROW_H_MIN);
    let n = ((budget / (2.0 * ROW_H)).round() as usize).clamp(BOOK_LEVELS, BOOK_MAX_LEVELS);
    let row_h = (budget / (2.0 * n as f32)).clamp(ROW_H_MIN, ROW_H_MAX);

    // 短锁拷贝（价格, 数量）。开启聚合时取全部原始档位按步长分桶：
    // bids 向下取整、asks 向上取整到桶边界，同桶数量合并（桶价与
    // Binance 深度聚合一致）；桶数少于 n 时面板留白
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
    let (asks, bids) = {
        let md = state.md.lock();
        let Some(md) = md.get(app.market, &app.page.symbol) else {
            widgets::loading_state(ui, crate::i18n::tr("加载中…"));
            return;
        };
        if md.depth.asks.is_empty() && md.depth.bids.is_empty() {
            widgets::loading_state(ui, crate::i18n::tr("等待深度数据…"));
            return;
        }
        let take = |side: &Vec<crate::binance::model::Level>, up: bool| {
            let raw: Vec<(Decimal, Decimal)> = side
                .iter()
                .map(|l| (l.price, l.qty))
                .collect();
            if group_step.is_zero() {
                return raw.into_iter().take(n).collect::<Vec<_>>();
            }
            // asks 升序（最优在前）桶边界向上取整；bids 降序向下取整。
            // 合并后保持原排序方向，同桶数量累加
            let mut merged: Vec<(Decimal, Decimal)> = Vec::with_capacity(raw.len());
            for (p, q) in raw {
                let buckets = p / group_step;
                let b = (if up {
                    buckets.ceil()
                } else {
                    buckets.floor()
                }) * group_step;
                if let Some(last) = merged.last_mut() {
                    if last.0 == b {
                        last.1 += q;
                        continue;
                    }
                }
                merged.push((b, q));
            }
            merged.into_iter().take(n).collect::<Vec<_>>()
        };
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
    let now = ui.ctx().input(|i| i.time);

    // egui 的滚动条悬浮在内容上层，行宽要主动让出滚动条宽度，否则数量列被压住
    let w = ui.available_width() - 10.0;

    // 残影更新：与上一帧价格集合 diff，消失的档位记为淡出残影
    app.book_anim.update(now, true, &asks, &ask_cums, max_cum);
    app.book_anim.update(now, false, &bids, &bid_cums, max_cum);

    // 各档矩形（按升序下标存），残影据此定位
    let mut ask_rects = [egui::Rect::NOTHING; BOOK_MAX_LEVELS];
    let mut bid_rects = [egui::Rect::NOTHING; BOOK_MAX_LEVELS];

    // 卖盘（高→低）：asks 升序（最近档在前），显示时倒序遍历。
    // 两侧最优价都贴着价差行；行高已吸收高度零头，残余留白（仅 n 触顶/
    // 触底时存在）一分为二：一半顶到卖盘上方，另一半自然沉到买盘下方
    let gap = (budget - 2.0 * n as f32 * row_h).max(0.0) / 2.0;
    if gap > 0.5 {
        ui.add_space(gap);
    }
    for (i, (price, qty)) in asks.iter().enumerate().rev() {
        ask_rects[i] = book_row(
            ui,
            app,
            row_h,
            w,
            ask_cums[i] / max_cum,
            theme::cur().down.gamma_multiply(0.28),
            price,
            dp,
            &widgets::f64_decimal(qty),
            qdp,
            theme::cur().down,
            flash::BookSite::AskPrice,
            i,
        );
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
    let spread_pct = if mid_f > 0.0 {
        spread_f / mid_f * 100.0
    } else {
        0.0
    };
    let one_sided = best_bid.is_none() || best_ask.is_none();
    let spread_txt = if spread.is_zero() && one_sided {
        String::new()
    } else {
        format!("{:.*} ({:.3}%)", dp, spread_f, spread_pct)
    };
    // 弹性留白已移到卖盘之前（见上方 gap 注释），价差行与买盘直接相接
    ui.add_space(SPREAD_PAD / 2.0);
    ui.allocate_ui(egui::vec2(w, SPREAD_H), |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{:.*}", dp, mid_f))
                    .monospace()
                    .strong()
                    .size(theme::FONT_BODY)
                    .color(theme::cur().text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 数值走等宽 + 正文色（信息数据不用暗色），"价差"标签才是辅助暗字
                if !spread_txt.is_empty() {
                    ui.label(
                        egui::RichText::new(&spread_txt)
                            .monospace()
                            .size(theme::FONT_SM)
                            .color(theme::cur().text),
                    );
                }
                ui.label(
                    egui::RichText::new(crate::i18n::tr("价差"))
                        .size(theme::FONT_SM)
                        .color(theme::cur().text_dim),
                );
            });
        });
    });
    ui.add_space(SPREAD_PAD / 2.0);

    // 买盘（低→高）
    for (i, (price, qty)) in bids.iter().enumerate() {
        bid_rects[i] = book_row(
            ui,
            app,
            row_h,
            w,
            bid_cums[i] / max_cum,
            theme::cur().up.gamma_multiply(0.28),
            price,
            dp,
            &widgets::f64_decimal(qty),
            qdp,
            theme::cur().up,
            flash::BookSite::BidPrice,
            i,
        );
    }

    // 残影淡出：在消失档位的排序槽位上叠加渐隐底色与深度条。
    // 不绘制文字（该槽位现已被平移后的新档位占据，文字会重叠花屏）
    for g in &app.book_anim.ghosts {
        let age = now - g.t0;
        if !(0.0..theme::GHOST_SECS).contains(&age) {
            continue;
        }
        let k = (1.0 - age / theme::GHOST_SECS) as f32;
        let n = if g.is_ask { asks.len() } else { bids.len() };
        let (i, rect) = if g.is_ask {
            // ask 显示倒序：显示槽位 s ↔ 升序下标 n-1-s
            let Some(j) = n.checked_sub(1 + g.slot) else {
                continue;
            };
            (j, ask_rects[j])
        } else {
            if g.slot >= n {
                continue;
            }
            (g.slot, bid_rects[g.slot])
        };
        if i >= BOOK_MAX_LEVELS || rect == egui::Rect::NOTHING {
            continue;
        }
        let base = if g.is_ask {
            theme::cur().down
        } else {
            theme::cur().up
        };
        let p = ui.painter();
        p.rect_filled(rect, 0.0, base.gamma_multiply(BOOK_FLASH_ALPHA * k * 0.7));
        let bw = (rect.width() as f64 * g.ratio.clamp(0.0, 1.0)) as f32;
        if bw > 0.5 {
            p.rect_filled(
                egui::Rect::from_min_size(
                    egui::Pos2::new(rect.right() - bw, rect.top()),
                    egui::vec2(bw, rect.height()),
                ),
                0.0,
                base.gamma_multiply(BOOK_FLASH_ALPHA * k),
            );
        }
    }
}

/// 已消失盘口档位的残影
#[derive(Debug, Clone, Copy)]
struct BookGhost {
    /// true = 卖盘，false = 买盘
    is_ask: bool,
    /// 显示行槽位（0 = 该侧块顶部行）
    slot: usize,
    /// 消失时的深度条占比
    ratio: f64,
    /// 消失时刻（`ctx.input(|i| i.time)` 域，秒）
    t0: f64,
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

    /// 与上一帧 diff：消失的档位记为残影，过期残影清除。
    fn update(
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

/// 盘口单行：悬停/闪烁底色 + 右锚定深度条 + 价格（左）/数量（右），点击价格填单
#[allow(clippy::too_many_arguments)]
fn book_row(
    ui: &mut egui::Ui,
    app: &mut BnApp,
    row_h: f32,
    w: f32,
    ratio: f64,
    bar_color: egui::Color32,
    price: &Decimal,
    dp: usize,
    qty_f: &f64,
    qdp: usize,
    price_color: egui::Color32,
    site: flash::BookSite,
    idx: usize,
) -> egui::Rect {
    // 显示用 f64；点击填单必须用 Decimal 原文——dp 回退（规则未加载时=2）
    // 会把 0.00001234 这类价格四舍五入成 "0.00"
    let price_f = widgets::f64_decimal(price);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, row_h), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    // 价格/数量变动高亮：500ms 内行底色渐隐（先于深度条绘制，压在其下层）。
    // 颜色固定用行方向色（卖盘跌色/买盘涨色），不按数值涨跌翻转：
    // 位点按显示行观测，吃单使整列价格平移时"涨跌"判定会系统性反向
    // （卖盘价格整体上移→误闪涨色），故与行静态颜色保持一致
    let (p_int, _) = app.flash_book(ui.ctx(), site, idx, price_f);
    let (q_int, _) = app.flash_book(ui.ctx(), site.qty_site(), idx, *qty_f);
    let intensity = p_int.max(q_int);
    if intensity > 0.0 {
        ui.painter().rect_filled(
            rect,
            0.0,
            price_color.gamma_multiply(BOOK_FLASH_ALPHA * intensity),
        );
    }
    // 深度条（悬停高亮画在其上层，避免右半区悬停反馈被半透明深度条冲淡）
    let bar_w = (w as f64 * ratio.clamp(0.0, 1.0)) as f32;
    if bar_w > 0.5 {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::Pos2::new(rect.right() - bar_w, rect.top()),
                egui::vec2(bar_w, row_h),
            ),
            0.0,
            bar_color,
        );
    }
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, theme::cur().row_hover);
    }
    // 价格（左）
    ui.painter().text(
        egui::Pos2::new(rect.left() + 2.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{price_f:.*}", dp),
        egui::FontId::monospace(theme::FONT_MD),
        price_color,
    );
    // 数量（右）
    ui.painter().text(
        egui::Pos2::new(rect.right() - 2.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("{qty_f:.*}", qdp),
        egui::FontId::monospace(theme::FONT_MD),
        theme::cur().text,
    );
    if resp.clicked() {
        // 点击填单用交易所精度原文（Decimal），不能用 UI 四舍五入后的值
        app.page.form.set_price(&price.to_string());
    }
    // 明示"点价格即填单"这一核心交互，并给出悬停提示
    resp.on_hover_text(crate::i18n::tr("点击填入下单价格"));
    rect
}

/// 逐笔成交
pub fn show_trades(app: &mut BnApp, ui: &mut egui::Ui) {
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

    // 行数随面板高度自适应（同盘口思路：内容刚好占满、无需滚动条），
    // 上限为行情缓冲的 100 笔；留 4px 余量防止取整溢出顶出滚动条
    const ROW_H: f32 = 17.0; // 数据行内容高（FONT_MD 文本行）
    const HEADER_H: f32 = 14.0; // 表头内容高（FONT_SM 文本行）
    let row_pitch = ROW_H + 2.0; // Grid spacing.y = 2
    let n = (((ui.available_height() - HEADER_H - 4.0) / row_pitch).floor() as usize)
        .clamp(1, 100);

    // 短锁拷贝最近 n 笔后绘制
    let trades: Vec<crate::binance::model::TradeTick> = {
        let md = state.md.lock();
        let Some(md) = md.get(app.market, &app.page.symbol) else {
            widgets::loading_state(ui, crate::i18n::tr("加载中…"));
            return;
        };
        md.trades.iter().rev().take(n).cloned().collect()
    };

    // 列宽 = 列内全部单元格实宽（含表头）——先整列统一测宽再绘制，同列所有
    // 单元格文字缘严格成列，便于纵向扫读；不再设固定下限宽（72/64/58），
    // 下限会把列撑过内容、把数量推离价格
    ui.set_min_width(0.0);
    let measure =
        |txt: &str, font: &egui::FontId| -> f32 {
            ui.painter()
                .layout_no_wrap(txt.to_string(), font.clone(), theme::cur().text)
                .size()
                .x
        };
    // 表头/数据两套字体：表头小号正体，数据等宽正文
    let head_font = egui::FontId::proportional(theme::FONT_SM);
    let data_font = egui::FontId::monospace(theme::FONT_MD);
    let (mut w_price, mut w_qty, mut w_time) = (
        measure(crate::i18n::tr("价格"), &head_font),
        measure(crate::i18n::tr("数量"), &head_font),
        measure(crate::i18n::tr("时间"), &head_font),
    );
    for t in &trades {
        w_price = w_price.max(measure(&format!("{:.*}", dp, widgets::f64_decimal(&t.price)), &data_font));
        w_qty = w_qty.max(measure(&format!("{:.*}", qdp, widgets::f64_decimal(&t.qty)), &data_font));
        w_time = w_time.max(measure(&widgets::fmt_time_short(t.time), &data_font));
    }
    // 窄面板时间降级：列宽绝不压到内容实宽以下（压缩会让手绘 galley 向左
    // 溢出、与数量列叠印）。放不下完整 HH:MM:SS 时省略秒，再不够才等比压缩
    let avail = ui.available_width();
    let tw_short = measure("00:00", &data_font);
    if w_price + w_qty + w_time + 20.0 > avail && w_price + w_qty + tw_short + 20.0 <= avail {
        w_time = tw_short;
    }
    let time_full = w_time > tw_short;
    // 列布局：价格贴左（与盘口行一致）、时间贴右、数量左对齐紧跟价格之后。
    // 两侧间距 = 剩余空间均分：价格↔数量 与 数量↔时间 永远等宽，视觉平衡；
    // 内宽不足时先按预算（两侧各留 MIN_GAP）等比压列宽，绝不叠印
    const MIN_GAP: f32 = 8.0;
    let content = w_price + w_qty + w_time;
    if content + 4.0 + 2.0 * MIN_GAP > avail && avail > 0.0 {
        let k = (avail - 4.0 - 2.0 * MIN_GAP) / content;
        w_price *= k;
        w_qty *= k;
        w_time *= k;
    }
    let gap = ((avail - 4.0 - w_price - w_qty - w_time) * 0.5).max(MIN_GAP);
    let paint_row =
        |ui: &mut egui::Ui, price: String, qty: String, time: String, h: f32, col: egui::Color32| {
            // 整行占位：三列文字按固定锚位手绘，不依赖 Grid 列分配
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), h),
                egui::Sense::hover(),
            );
            let p = ui.painter();
            let cy = rect.center().y;
            // 数量左缘 = 价格列右缘 + gap；时间右缘 = 行右缘 - 2
            let x_qty = rect.left() + 2.0 + w_price + gap;
            p.text(
                egui::Pos2::new(rect.left() + 2.0, cy),
                egui::Align2::LEFT_CENTER,
                price,
                data_font.clone(),
                col,
            );
            p.text(
                egui::Pos2::new(x_qty, cy),
                egui::Align2::LEFT_CENTER,
                qty,
                data_font.clone(),
                theme::cur().text,
            );
            p.text(
                egui::Pos2::new(rect.right() - 2.0, cy),
                egui::Align2::RIGHT_CENTER,
                time,
                data_font.clone(),
                theme::cur().text_dim,
            );
        };
    if trades.is_empty() {
        widgets::empty_state(ui, "◍", crate::i18n::tr("暂无成交"), "");
        return;
    }
    // 表头与数据行共用同一组锚位；行距 2pt
    ui.spacing_mut().item_spacing.y = 2.0;
    {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), HEADER_H),
            egui::Sense::hover(),
        );
        let p = ui.painter();
        let cy = rect.center().y;
        let x_qty = rect.left() + 2.0 + w_price + gap;
        p.text(
            egui::Pos2::new(rect.left() + 2.0, cy),
            egui::Align2::LEFT_CENTER,
            crate::i18n::tr("价格").to_string(),
            head_font.clone(),
            theme::cur().text_dim,
        );
        p.text(
            egui::Pos2::new(x_qty, cy),
            egui::Align2::LEFT_CENTER,
            crate::i18n::tr("数量").to_string(),
            head_font.clone(),
            theme::cur().text_dim,
        );
        p.text(
            egui::Pos2::new(rect.right() - 2.0, cy),
            egui::Align2::RIGHT_CENTER,
            crate::i18n::tr("时间").to_string(),
            head_font.clone(),
            theme::cur().text_dim,
        );
    }
    for (row, t) in trades.iter().enumerate() {
        // 买方主动（buyer_maker=true）→ 主动卖单 → 跌色；反之为涨色。
        // 与盘口涨跌色同源（theme::trade_color），随 price_dir 翻转
        let mut col = theme::trade_color(t.buyer_maker);
        // 最新一行（row 0）：新成交到达时文字向主题文字色短暂提亮后渐回原色。
        // 不混入行底色，混色会红绿互相污染
        if row == 0 {
            let (intensity, _) =
                app.flash_trade_top(ui.ctx(), widgets::f64_decimal(&t.price));
            if intensity > 0.0 {
                col = flash::lerp_color(
                    col,
                    theme::cur().text,
                    intensity * theme::FLASH_BRIGHTEN,
                );
            }
        }
        let ts = widgets::fmt_time_short(t.time);
        paint_row(
            ui,
            // 不做千分位分组，与盘口一致按规则精度定长显示，纵向小数点对齐
            format!("{:.*}", dp, widgets::f64_decimal(&t.price)),
            format!("{:.*}", qdp, widgets::f64_decimal(&t.qty)),
            // 窄面板时间降级：省略秒（HH:MM），与列宽决策一致。
            // 越界时间戳 fmt_time_short 返回 "—"：用 get 而非固定切片，
            // "—".len() < 5 时固定切片会 byte-index panic 整窗崩溃
            if time_full {
                ts
            } else {
                ts.get(..5).unwrap_or(&ts).to_string()
            },
            ROW_H,
            col,
        );
    }
}

/// 页头：交易对 / 最新价与涨跌 / 24h 统计（合约附资金费率）
pub fn show_header(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = app.state.clone();
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
    // 单次短锁取齐本帧所需的 ticker 与资金费率（原先分 3 次锁、逐字段字符串解析）
    let (tick, funding, funding_at) = {
        let md_guard = state.md.lock();
        match md_guard.get(app.market, &app.page.symbol) {
            Some(md) => (
                md.ticker.clone(),
                md.mark.as_ref().and_then(|m| m.funding_rate),
                md.mark.as_ref().map(|m| m.next_funding_time),
            ),
            None => (None, None, None),
        }
    };

    // 第一行：交易对 | 最新价/涨跌 | 自选星标 / 预警
    ui.horizontal_wrapped(|ui| {
        // 交易对双色标题：base（标的）强调、quote（计价）弱化，
        // 与交易所惯例一致；规则未加载/结构不符时整体回退单色原文。
        // 记录标题文字竖直中线，作为行尾星标/预警 glyph 的对齐锚点
        let mut anchor_y: Option<f32>;
        if !base.is_empty() && app.page.symbol.starts_with(&base) {
            let r = ui.label(
                egui::RichText::new(&base)
                    .strong()
                    .size(theme::FONT_LG),
            );
            anchor_y = Some(r.rect.center().y);
            ui.label(
                egui::RichText::new(&app.page.symbol[base.len()..])
                    .size(theme::FONT_LG)
                    .color(theme::cur().text_dim),
            );
        } else {
            let r = ui.label(
                egui::RichText::new(app.page.symbol.clone())
                    .strong()
                    .size(theme::FONT_LG),
            );
            anchor_y = Some(r.rect.center().y);
        }
        ui.separator();
        if let Some(t) = &tick {
            let chg = widgets::f64_decimal(&t.price_change_percent);
            let mut c = theme::signed_color(chg);
            let px = widgets::f64_decimal(&t.last_price);
            // 最新价变动闪烁：强度 1 → 0 期间把文字色向闪烁色（涨/跌）插值
            let (fi, fd) = app.flash_header_px(ui.ctx(), px);
            if fi > 0.0 {
                c = flash::lerp_color(c, flash::flash_color(fd), fi);
            }
            let r = ui.label(
                egui::RichText::new(widgets::fmt_thousands(px, dp))
                    .monospace()
                    .strong()
                    .size(theme::FONT_LG)
                    .color(c),
            );
            // 最新价是星标/预警的直接参照物，以其中线为准（与标题同为 FONT_LG）
            anchor_y = Some(r.rect.center().y);
            ui.label(
                egui::RichText::new(format!("{chg:+.2}%"))
                    .monospace()
                    .size(theme::FONT_BODY)
                    .color(c),
            );
        }
        // F1：自选星标与价格预警紧跟交易对名称/最新价，与交易对信息成组；
        // 此前置于此行最右、与交易对之间隔着整排周期按钮，关联感太弱。
        // 竖直对齐走 glyph_button 的 anchor_y：Button 的 18pt 最小高与内
        // 边距会把 glyph 压得比同行数字偏低，自绘热区直接钉在数字中线上
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let is_fav = app.state.favorites(app.market).contains(&app.page.symbol);
            // 价格预警入口：本交易对已有 N 条时高亮
            let alert_n = app
                .state
                .cfg
                .read()
                .alerts
                .iter()
                .filter(|a| a.market == app.market as u8 && a.symbol == app.page.symbol)
                .count();
            if widgets::glyph_button(
                ui,
                if is_fav { "★" } else { "☆" },
                theme::FONT_BODY,
                if is_fav {
                    theme::cur().accent
                } else {
                    theme::cur().text_dim
                },
                anchor_y,
                if is_fav {
                    crate::i18n::tr("取消自选")
                } else {
                    crate::i18n::tr("加入自选")
                },
            )
            .clicked()
            {
                app.state.toggle_favorite(app.market, &app.page.symbol);
            }
            let bell_txt = if alert_n > 0 {
                format!("{}({alert_n})", crate::i18n::tr("预警"))
            } else {
                crate::i18n::tr("预警").to_string()
            };
            if widgets::glyph_button(
                ui,
                &bell_txt,
                theme::FONT_SM,
                if alert_n > 0 || app.alerts_open {
                    theme::cur().accent
                } else {
                    theme::cur().text_dim
                },
                anchor_y,
                crate::i18n::tr("价格预警（本地触发，到价弹提示）"),
            )
            .clicked()
            {
                app.alerts_open = !app.alerts_open;
            }
        });
    });

    // 第二行：24h 统计 / 资金费率。每个统计项画成独立分组格 = 紧凑
    // [标签 + 值] 横向对，格间细分隔线收束视线——此前 label value label value
    // 平铺成无结构的一长条，扫读时找不到各项的边界
    // 分组内 label↔value 紧贴（3pt），整格作为一个整体随 wrapped 换行
    let stat = |ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(theme::card_label(label));
            ui.label(
                egui::RichText::new(value)
                    .monospace()
                    .size(theme::FONT_MD)
                    .color(color),
            );
        })
    };
    // 第三行：24h 开/高/低/收（行情四要素独占一行，价格动得最快、也最常被扫读）
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if let Some(t) = &tick {
            let o = widgets::fmt_thousands(widgets::f64_decimal(&t.open_price), dp);
            let h = widgets::fmt_thousands(widgets::f64_decimal(&t.high_price), dp);
            let l = widgets::fmt_thousands(widgets::f64_decimal(&t.low_price), dp);
            let c = widgets::fmt_thousands(widgets::f64_decimal(&t.last_price), dp);
            stat(ui, crate::i18n::tr("24h 开"), o, theme::cur().text);
            ui.separator();
            stat(ui, crate::i18n::tr("24h 高"), h, theme::cur().up);
            ui.separator();
            stat(ui, crate::i18n::tr("24h 低"), l, theme::cur().down);
            ui.separator();
            stat(ui, crate::i18n::tr("24h 收"), c, theme::cur().text);
        }
    });
    // 第四行：24h 量/额（成交宏观指标），合约再追加资金费率 + 下次结算倒计时
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if let Some(t) = &tick {
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
            stat(
                ui,
                crate::i18n::tr("24h 量"),
                format!(
                    "{}{base_txt}",
                    widgets::fmt_compact(widgets::f64_decimal(&t.volume))
                ),
                theme::cur().text,
            );
            ui.separator();
            stat(
                ui,
                crate::i18n::tr("24h 额"),
                format!(
                    "{}{quote_txt}",
                    widgets::fmt_compact(widgets::f64_decimal(&t.quote_volume))
                ),
                theme::cur().text,
            );
        }
        // 合约：资金费率（正/负着色 + 显式符号）+ 下次结算倒计时
        if let (crate::binance::Market::UmFutures, Some(r)) = (app.market, funding) {
            // Binance 的 lastFundingRate 是名义价值比例（0.0001 = 0.01%），换算成百分比展示
            let rf = widgets::f64_decimal(&r) * 100.0;
            let color = if rf > 0.0 {
                theme::cur().up
            } else if rf < 0.0 {
                theme::cur().down
            } else {
                theme::cur().text_dim
            };
            // 倒计时：费率旁直接可见距离收/付资金费的剩余时间
            let cd = funding_at
                .filter(|&next| next > 0)
                .map(|next| {
                    let left = (next - chrono::Utc::now().timestamp_millis()).max(0) / 1000;
                    format!(
                        " · {:02}:{:02}:{:02}",
                        left / 3600,
                        (left % 3600) / 60,
                        left % 60
                    )
                })
                .unwrap_or_default();
            ui.separator();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(theme::card_label(crate::i18n::tr("资金费率")));
                ui.label(
                    egui::RichText::new(format!("{rf:+.4}%"))
                        .monospace()
                        .size(theme::FONT_MD)
                        .strong()
                        .color(color),
                );
                if !cd.is_empty() {
                    ui.label(
                        egui::RichText::new(cd)
                            .monospace()
                            .size(theme::FONT_MD)
                            .color(theme::cur().text_dim),
                    );
                }
            });
        }
    });

    // 周期直选已移到图上方工具行（chart.rs 工具行，MA 开关左侧），
    // 页头不再渲染——页头只保留行情信息，周期切换贴近图表操作

    // 价格预警弹窗：当前交易对添加 + 全部预警列表（一次性触发，到价 toast）
    if app.alerts_open {
        let mut close = false;
        egui::Window::new(
            egui::RichText::new(crate::i18n::tr("价格预警"))
                .strong()
                .size(theme::FONT_LG),
        )
        .id(egui::Id::new("alerts_win"))
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 92.0])
        .collapsible(false)
        .resizable(false)
        .min_width(272.0)
        .default_height(0.0)
        .show(ui.ctx(), |ui| {
            // 添加行：目标价（方向按相对最新价自动判定）
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&app.page.symbol)
                        .strong()
                        .size(theme::FONT_MD),
                );
                if let Some(t) = &tick {
                    ui.label(
                        egui::RichText::new(widgets::fmt_dec_fixed(&t.last_price, dp))
                            .monospace()
                            .size(theme::FONT_SM)
                            .color(theme::cur().text_dim),
                    );
                }
            });
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.alert_input)
                        .id_salt("alert_price_in")
                        .hint_text(crate::i18n::tr("目标价"))
                        .font(egui::TextStyle::Monospace)
                        .desired_width(110.0),
                );
                let enter = resp.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(crate::i18n::tr("添加")).size(theme::FONT_SM))
                            .min_size(egui::vec2(0.0, theme::CONTROL_H)),
                    )
                    .clicked()
                    || enter
                {
                    app.add_alert();
                }
            });
            ui.add_space(2.0);
            ui.separator();
            let alerts = app.state.cfg.read().alerts.clone();
            if alerts.is_empty() {
                ui.label(
                    egui::RichText::new(crate::i18n::tr("暂无预警，输入目标价添加"))
                        .size(theme::FONT_SM)
                        .color(theme::cur().text_dim),
                );
            } else {
                let adp = |mkt: u8, sym: &str| -> usize {
                    let m = if mkt == 0 {
                        crate::binance::Market::Spot
                    } else {
                        crate::binance::Market::UmFutures
                    };
                    state.with_rule(m, sym, |r| r.map(|x| x.price_dp() as usize).unwrap_or(2))
                };
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for (i, a) in alerts.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&a.symbol).size(theme::FONT_MD));
                                let mkt_label = if a.market == 0 { "" } else { crate::i18n::tr("· 合约") };
                                if !mkt_label.is_empty() {
                                    ui.label(
                                        egui::RichText::new(mkt_label)
                                            .size(theme::FONT_SM)
                                            .color(theme::cur().text_dim),
                                    );
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("×")
                                                        .size(theme::FONT_SM)
                                                        .color(theme::cur().text_dim),
                                                )
                                                .fill(egui::Color32::TRANSPARENT)
                                                .corner_radius(theme::R_SM),
                                            )
                                            .on_hover_text(crate::i18n::tr("删除"))
                                            .clicked()
                                        {
                                            app.remove_alert(i);
                                        }
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} {}",
                                                if a.above { "≥" } else { "≤" },
                                                widgets::fmt_dec_fixed(&a.price, adp(a.market, &a.symbol))
                                            ))
                                            .monospace()
                                            .size(theme::FONT_MD)
                                            .color(if a.above {
                                                theme::cur().up
                                            } else {
                                                theme::cur().down
                                            }),
                                        );
                                    },
                                );
                            });
                        }
                    });
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        if close {
            app.alerts_open = false;
        }
    }
    ui.separator();
}
