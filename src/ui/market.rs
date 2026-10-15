//! 盘口（深度）与逐笔成交列表
//!
//! 锁纪律：md 是全系统最热的锁（depth20@100ms / aggTrade 每条 WS 消息都要写），
//! 这里统一"短锁拷贝 → 释放 → 绘制"，绝不在面板构建期间持有锁。

use crate::app::BnApp;
use crate::binance::{Interval, Market};
use crate::ui::flash;
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;
use rust_decimal::Decimal;
use std::sync::Arc;

/// 每侧显示档数下限（面板过矮时的保底）
pub const BOOK_LEVELS: usize = 10;
/// 每侧显示档数上限：订阅的 depth20@100ms 推满 20 档，
/// 实际档位数随面板高度在两者之间自适应
pub const BOOK_MAX_LEVELS: usize = 20;

/// 右栏内容页签：盘口 / 成交明细
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookTab {
    /// 盘口（档位随面板高度自适应，10~20 档）
    Book,
    /// 成交明细
    Trades,
}

/// 右栏：盘口 / 成交明细 双页签，互斥显示
pub fn show_book_panel(app: &mut BnApp, ui: &mut egui::Ui) {
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
    // 盘口聚合档位步长（Binance 同款）：tick 的 1/10/100/1000 倍。独立一行
    // 而非塞进页签行——面板窄时 RTL 溢出会叠到"成交明细"页签上
    if app.book_tab == BookTab::Book {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
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
                for (i, s) in steps.iter().enumerate().rev() {
                    let label =
                        widgets::fmt_dec_fixed(s, s.normalize().scale() as usize);
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
                        .corner_radius(3)
                        .min_size(egui::vec2(0.0, 18.0));
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
        ui.separator();
    }
    // 切页签后闪光/残影的"上一帧"属于旧页内容：重置基线，
    // 避免切回时把缺失时段累积的价格变化整列闪一遍
    if app.book_tab != prev {
        app.flash_book = Default::default();
        app.flash_trade = Default::default();
        app.book_anim.clear();
    }
    match app.book_tab {
        BookTab::Book => show_book(app, ui),
        BookTab::Trades => show_trades(app, ui),
    }
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
    // 无需滚动条）。上限 20 档 = depth20 推送的完整档数，下限 10 档保底；
    // SAFETY 防取整溢出顶出滚动条
    ui.spacing_mut().item_spacing.y = 0.0;
    const SPREAD_H: f32 = 24.0;
    const SPREAD_PAD: f32 = 8.0; // 价差行上下呼吸
    const SAFETY: f32 = 6.0;
    const ROW_H: f32 = 18.0;
    let avail = ui.available_height();
    let n = (((avail - SPREAD_H - SPREAD_PAD - SAFETY) / (2.0 * ROW_H)).floor() as usize)
        .clamp(BOOK_LEVELS, BOOK_MAX_LEVELS);

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

    // 卖盘（高→低）：asks 升序（最近档在前），显示时倒序遍历
    for (i, (price, qty)) in asks.iter().enumerate().rev() {
        ask_rects[i] = book_row(
            ui,
            app,
            ROW_H,
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
    // 弹性留白：价差行与买盘贴面板底部，剩余空间集中在其上方——
    // 最优价/价差/买盘（交易重心）聚在底部，远端卖档留在顶部作参考
    // （主流交易所布局）；档位未到上限时 gap≈0，布局退化为自上而下顺排
    let gap = (avail - SAFETY - SPREAD_PAD - SPREAD_H - 2.0 * n as f32 * ROW_H).max(0.0);
    if gap > 0.5 {
        ui.add_space(gap);
    }
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
            ROW_H,
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
        p.rect_filled(rect, 0.0, base.gamma_multiply(theme::FLASH_ALPHA * k * 0.7));
        let bw = (rect.width() as f64 * g.ratio.clamp(0.0, 1.0)) as f32;
        if bw > 0.5 {
            p.rect_filled(
                egui::Rect::from_min_size(
                    egui::Pos2::new(rect.right() - bw, rect.top()),
                    egui::vec2(bw, rect.height()),
                ),
                0.0,
                base.gamma_multiply(theme::FLASH_ALPHA * k),
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
            price_color.gamma_multiply(theme::FLASH_ALPHA * intensity),
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

    ui.set_min_width(0.0);
    // 数字列右对齐 + 最小列宽：价格/数量纵向成列，便于扫读。
    // 列宽 = max(设定宽, 列内全部单元格实宽)——含表头。Grid 列宽取列内
    // 最宽单元格，但各单元格若只按自身文本设宽，窄单元格（如表头"时间"）
    // 会停在自身右缘而非列右缘，出现 1-2pt 的错位；先整列统一测宽再绘制，
    // 同列所有单元格右缘严格一致
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
        measure(crate::i18n::tr("价格"), &head_font).max(72.0),
        measure(crate::i18n::tr("数量"), &head_font).max(64.0),
        measure(crate::i18n::tr("时间"), &head_font).max(58.0),
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
    if w_price + w_qty + w_time + 24.0 > avail && w_price + w_qty + tw_short + 24.0 <= avail {
        w_time = tw_short;
    }
    let time_full = w_time > tw_short;
    let total = w_price + w_qty + w_time + 24.0;
    if total > avail {
        let k = avail / total;
        w_price *= k;
        w_qty *= k;
        w_time *= k;
    }
    let num_cell =
        |ui: &mut egui::Ui, txt: String, font: egui::FontId, color: egui::Color32, w: f32, h: f32| {
            // 精确占位 + 手绘 galley：单元格右缘 = 列右缘，文本右对齐到
            // rect.right()，不依赖 RTL 布局的落位细节（Grid 按各单元格
            // 自身内容定列宽时，set_min_size 的窄单元格会差出几 pt）
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
            let galley = ui.painter().layout_no_wrap(txt.clone(), font, color);
            let pos = egui::pos2(
                rect.right() - galley.size().x,
                rect.center().y - galley.size().y * 0.5,
            );
            ui.painter().galley(pos, galley, color);
        };
    if trades.is_empty() {
        widgets::empty_state(ui, "◍", crate::i18n::tr("暂无成交"), "");
        return;
    }
    // 列宽随面板可用宽等比收缩：Grid 最小宽一旦超过面板内宽，egui 会把内容
    // 宽度持久化进 PanelState，切到成交页签面板就被撑宽且切不回去。
    // 表头与数据行用同一组列宽，纵向对齐不受影响
    let col_k = (ui.available_width() / (w_price + w_qty + w_time + 24.0)).min(1.0);
    w_price *= col_k;
    w_qty *= col_k;
    w_time *= col_k;
    egui::Grid::new("trades_grid")
        .num_columns(3)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            // 表头与数据同列对齐：价格/数量/时间均右对齐（与 num_cell 同宽）
            num_cell(
                ui,
                crate::i18n::tr("价格").to_string(),
                head_font.clone(),
                theme::cur().text_dim,
                w_price,
                HEADER_H,
            );
            num_cell(
                ui,
                crate::i18n::tr("数量").to_string(),
                head_font.clone(),
                theme::cur().text_dim,
                w_qty,
                HEADER_H,
            );
            num_cell(
                ui,
                crate::i18n::tr("时间").to_string(),
                head_font.clone(),
                theme::cur().text_dim,
                w_time,
                HEADER_H,
            );
            ui.end_row();

            for (row, t) in trades.iter().enumerate() {
                // 买方主动（buyer_maker=true）→ 主动卖单 → 跌色；反之为涨色。
                // 与盘口涨跌色同源（theme::trade_color），随 price_dir 翻转
                let mut col = theme::trade_color(t.buyer_maker);
                // 最新一行（row 0）：新成交到达时文字向主题文字色短暂提亮后渐回原色。
                // 不混入涨跌方向色（Grid 无法铺行底色，混色会红绿互相污染）
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
                num_cell(
                    ui,
                    // 不做千分位分组，与盘口一致按规则精度定长显示，纵向小数点对齐
                    format!("{:.*}", dp, widgets::f64_decimal(&t.price)),
                    data_font.clone(),
                    col,
                    w_price,
                    ROW_H,
                );
                num_cell(
                    ui,
                    format!("{:.*}", qdp, widgets::f64_decimal(&t.qty)),
                    data_font.clone(),
                    theme::cur().text,
                    w_qty,
                    ROW_H,
                );
                let ts = widgets::fmt_time_short(t.time);
                num_cell(
                    ui,
                    // 窄面板时间降级：省略秒（HH:MM），与列宽决策一致
                    if time_full { ts } else { ts[..5].to_string() },
                    data_font.clone(),
                    theme::cur().text_dim,
                    w_time,
                    ROW_H,
                );
                ui.end_row();
            }
        });
}

/// 页头：交易对 / 最新价与涨跌 / 周期直选 + 24h 统计（合约附资金费率）
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

    // 第一行：交易对 | 最新价/涨跌 | 周期直选按钮 | 自选星标
    // wrapped：窄窗口时周期按钮换行而不是被裁掉
    ui.horizontal_wrapped(|ui| {
        // 交易对双色标题：base（标的）强调、quote（计价）弱化，
        // 与交易所惯例一致；规则未加载/结构不符时整体回退单色原文
        if !base.is_empty() && app.page.symbol.starts_with(&base) {
            ui.label(
                egui::RichText::new(&base)
                    .strong()
                    .size(theme::FONT_LG),
            );
            ui.label(
                egui::RichText::new(&app.page.symbol[base.len()..])
                    .size(theme::FONT_LG)
                    .color(theme::cur().text_dim),
            );
        } else {
            ui.label(
                egui::RichText::new(app.page.symbol.clone())
                    .strong()
                    .size(theme::FONT_LG),
            );
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
            ui.label(
                egui::RichText::new(widgets::fmt_thousands(px, dp))
                    .monospace()
                    .strong()
                    .size(theme::FONT_LG)
                    .color(c),
            );
            ui.label(
                egui::RichText::new(format!("{chg:+.2}%"))
                    .monospace()
                    .size(theme::FONT_BODY)
                    .color(c),
            );
        }
        ui.separator();
        // 周期直选（胶囊页签）
        for iv in Interval::all() {
            if theme::pill_tab(ui, iv.label(), app.page.interval == iv).clicked() {
                app.switch_interval(iv);
            }
        }
        // F1：自选星标固定在行最右；其左侧为账户条（钱包/可用/未实现盈亏）
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
            let bell_txt = egui::RichText::new(if alert_n > 0 {
                format!("{}({alert_n})", crate::i18n::tr("预警"))
            } else {
                crate::i18n::tr("预警").to_string()
            })
            .size(theme::FONT_SM)
            .color(if alert_n > 0 || app.alerts_open {
                theme::cur().accent
            } else {
                theme::cur().text_dim
            });
            if ui
                .add(
                    egui::Button::new(bell_txt)
                        .fill(egui::Color32::TRANSPARENT)
                        .corner_radius(3),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(crate::i18n::tr("价格预警（本地触发，到价弹提示）"))
                .clicked()
            {
                app.alerts_open = !app.alerts_open;
            }
            let star_txt = egui::RichText::new(if is_fav { "★" } else { "☆" })
                .size(theme::FONT_BODY)
                .color(if is_fav {
                    theme::cur().accent
                } else {
                    theme::cur().text_dim
                });
            if ui
                .add(
                    egui::Button::new(star_txt)
                        .fill(egui::Color32::TRANSPARENT)
                        .corner_radius(3),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(if is_fav {
                    crate::i18n::tr("取消自选")
                } else {
                    crate::i18n::tr("加入自选")
                })
                .clicked()
            {
                app.state.toggle_favorite(app.market, &app.page.symbol);
            }
            // 账户条：交易页抬头直接可见资金（WS 事件驱动刷新，准实时）
            let acct = app.state.acct.lock();
            let chip = |ui: &mut egui::Ui, label: &str, v: String, c: egui::Color32| {
                ui.label(theme::card_label(label));
                ui.label(
                    egui::RichText::new(v)
                        .monospace()
                        .size(theme::FONT_MD)
                        .color(c),
                );
                ui.separator();
            };
            match app.market {
                Market::UmFutures => {
                    if let Some(a) = &acct.fut {
                        let f = widgets::f64_decimal;
                        chip(
                            ui,
                            crate::i18n::tr("钱包"),
                            widgets::fmt_compact(f(&a.total_wallet)),
                            theme::cur().text,
                        );
                        chip(
                            ui,
                            crate::i18n::tr("可用"),
                            widgets::fmt_compact(f(&a.available)),
                            theme::cur().text,
                        );
                        chip(
                            ui,
                            crate::i18n::tr("未实现"),
                            format!(
                                "{:+.2}",
                                f(&a.total_unrealized)
                            ),
                            theme::signed_color(f(&a.total_unrealized)),
                        );
                    }
                }
                Market::Spot => {
                    if let Some(a) = &acct.spot {
                        if let Some(b) = a.balances.iter().find(|b| b.asset == "USDT") {
                            chip(
                                ui,
                                crate::i18n::tr("USDT 可用"),
                                widgets::fmt_compact(widgets::f64_decimal(&b.free)),
                                theme::cur().text,
                            );
                        }
                    }
                }
            }
        });
    });

    // 第二行：24h OHLC / 交易量 / 交易额 /（合约）资金费率（wrapped 防窄窗截断）
    ui.horizontal_wrapped(|ui| {
        if let Some(t) = &tick {
            // Extend：值标签不在数字内部断行，horizontal_wrapped 会把整个标签挪到下一行
            let ext =
                |rt: egui::RichText| egui::Label::new(rt).wrap_mode(egui::TextWrapMode::Extend);
            let stat = |ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32| {
                ui.add(ext(theme::card_label(label)));
                ui.add(ext(egui::RichText::new(value)
                    .monospace()
                    .size(theme::FONT_MD)
                    .color(color)));
                ui.add_space(4.0);
            };
            let o = widgets::fmt_thousands(widgets::f64_decimal(&t.open_price), dp);
            let h = widgets::fmt_thousands(widgets::f64_decimal(&t.high_price), dp);
            let l = widgets::fmt_thousands(widgets::f64_decimal(&t.low_price), dp);
            let c = widgets::fmt_thousands(widgets::f64_decimal(&t.last_price), dp);
            stat(ui, crate::i18n::tr("24h 开"), o, theme::cur().text);
            stat(ui, crate::i18n::tr("24h 高"), h, theme::cur().up);
            stat(ui, crate::i18n::tr("24h 低"), l, theme::cur().down);
            stat(ui, crate::i18n::tr("24h 收"), c, theme::cur().text);
            ui.separator();
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
        // 合约：资金费率（正/负着色 + 显式符号）
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
            ui.separator();
            ui.add(
                egui::Label::new(theme::card_label(crate::i18n::tr("资金费率")))
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{rf:+.4}%"))
                        .monospace()
                        .size(theme::FONT_MD)
                        .strong()
                        .color(color),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
            // 下次结算倒计时：费率旁直接可见距离收/付资金费的剩余时间
            if let Some(next) = funding_at {
                if next > 0 {
                    let left = (next - chrono::Utc::now().timestamp_millis()).max(0) / 1000;
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                " · {:02}:{:02}:{:02}",
                                left / 3600,
                                (left % 3600) / 60,
                                left % 60
                            ))
                            .monospace()
                            .size(theme::FONT_MD)
                            .color(theme::cur().text_dim),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                }
            }
        }
    });

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
                        egui::RichText::new(widgets::fmt_thousands(
                            widgets::f64_decimal(&t.last_price),
                            dp,
                        ))
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
                                let mkt_label = if a.market == 0 { "" } else { "· 合约" };
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
                                                .corner_radius(3),
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
                                                widgets::fmt_thousands(a.price, adp(a.market, &a.symbol))
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
