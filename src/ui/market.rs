//! 盘口（深度）与逐笔成交列表
//!
//! 锁纪律：md 是全系统最热的锁（depth20@100ms / aggTrade 每条 WS 消息都要写），
//! 这里统一"短锁拷贝 → 释放 → 绘制"，绝不在面板构建期间持有锁。

use crate::app::BnApp;
use crate::binance::Interval;
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;
use rust_decimal::Decimal;
use std::sync::Arc;

/// 每侧显示档数
const BOOK_LEVELS: usize = 10;

/// 盘口面板：点击价格 → 填入下单价格
pub fn show_book(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let dp = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.price_dp() as usize)
        })
        .unwrap_or(2);
    let qdp = state
        .with_rule(app.market, &app.page.symbol, |r| r.map(|x| x.qty_dp() as usize))
        .unwrap_or(4);

    ui.add_space(4.0);
    ui.heading(
        egui::RichText::new(crate::i18n::tr("盘口 10 档"))
            .size(theme::FONT_BODY)
            .color(theme::cur().text),
    );
    ui.separator();

    // 短锁拷贝（价格, 数量）
    let (asks, bids) = {
        let md = state.md.lock();
        let Some(md) = md.get(app.market, &app.page.symbol) else {
            ui.label(egui::RichText::new(crate::i18n::tr("加载中…")).color(theme::cur().text_dim));
            return;
        };
        if md.depth.asks.is_empty() && md.depth.bids.is_empty() {
            ui.label(
                egui::RichText::new(crate::i18n::tr("等待深度数据…")).color(theme::cur().text_dim),
            );
            return;
        }
        let take = |side: &Vec<crate::binance::model::Level>| {
            side.iter()
                .take(BOOK_LEVELS)
                .map(|l| (l.price, l.qty))
                .collect::<Vec<(Decimal, Decimal)>>()
        };
        (take(&md.depth.asks), take(&md.depth.bids))
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

    let row_h = 18.0;
    // egui 的滚动条悬浮在内容上层，行宽要主动让出滚动条宽度，否则数量列被压住
    let w = ui.available_width() - 10.0;

    // 卖盘（高→低）：asks 升序（最近档在前），显示时倒序遍历
    for (i, (price, qty)) in asks.iter().enumerate().rev() {
        book_row(
            ui,
            app,
            row_h,
            w,
            ask_cums[i] / max_cum,
            theme::cur().down.gamma_multiply(0.28),
            &widgets::f64_decimal(price),
            dp,
            &widgets::f64_decimal(qty),
            qdp,
            theme::cur().down,
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
    let spread_pct = if mid_f > 0.0 { spread_f / mid_f * 100.0 } else { 0.0 };
    let one_sided = best_bid.is_none() || best_ask.is_none();
    let spread_txt = if spread.is_zero() && one_sided {
        String::new()
    } else {
        format!(
            "{} {:.*} ({:.3}%)",
            crate::i18n::tr("价差"),
            dp,
            spread_f,
            spread_pct
        )
    };
    ui.allocate_ui(egui::vec2(w, 24.0), |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{:.*}", dp, mid_f))
                    .monospace()
                    .strong()
                    .size(theme::FONT_BODY)
                    .color(theme::cur().text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(spread_txt)
                        .size(theme::FONT_SM)
                        .color(theme::cur().text_dim),
                );
            });
        });
    });

    // 买盘（低→高）
    for (i, (price, qty)) in bids.iter().enumerate() {
        book_row(
            ui,
            app,
            row_h,
            w,
            bid_cums[i] / max_cum,
            theme::cur().up.gamma_multiply(0.28),
            &widgets::f64_decimal(price),
            dp,
            &widgets::f64_decimal(qty),
            qdp,
            theme::cur().up,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn book_row(
    ui: &mut egui::Ui,
    app: &mut BnApp,
    row_h: f32,
    w: f32,
    ratio: f64,
    bar_color: egui::Color32,
    price_f: &f64,
    dp: usize,
    qty_f: &f64,
    qdp: usize,
    price_color: egui::Color32,
) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, row_h), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, theme::cur().row_hover);
    }
    // 深度条
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
        // 点击填单要用交易所精度原文，不能用 UI 四舍五入后的值
        app.page.form.set_price(&format!("{price_f:.*}", dp));
    }
    // 明示"点价格即填单"这一核心交互，并给出悬停提示
    resp.on_hover_text(crate::i18n::tr("点击填入下单价格"));
}

/// 逐笔成交
pub fn show_trades(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);

    ui.add_space(4.0);
    ui.heading(
        egui::RichText::new(crate::i18n::tr("成交明细"))
            .size(theme::FONT_BODY)
            .color(theme::cur().text),
    );
    ui.separator();

    // 短锁拷贝最近 30 笔后绘制
    let trades: Vec<crate::binance::model::TradeTick> = {
        let md = state.md.lock();
        let Some(md) = md.get(app.market, &app.page.symbol) else {
            ui.label(egui::RichText::new(crate::i18n::tr("加载中…")).color(theme::cur().text_dim));
            return;
        };
        md.trades.iter().rev().take(30).cloned().collect()
    };

    ui.set_min_width(0.0);
    // 数字列右对齐 + 最小列宽：价格/数量纵向成列，便于扫读
    let num_cell = |ui: &mut egui::Ui, text: egui::RichText, w: f32| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_min_width(w);
            ui.label(text);
        });
    };
    egui::Grid::new("trades_grid")
        .num_columns(3)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            for t in ["价格", "数量", "时间"] {
                ui.label(
                    egui::RichText::new(crate::i18n::tr(t))
                        .size(theme::FONT_SM)
                        .color(theme::cur().text_dim),
                );
            }
            ui.end_row();

            for t in &trades {
                let col = if t.buyer_maker {
                    theme::cur().down
                } else {
                    theme::cur().up
                };
                num_cell(
                    ui,
                    egui::RichText::new(widgets::fmt_dec_grouped(&t.price))
                        .monospace()
                        .size(theme::FONT_MD)
                        .color(col),
                    72.0,
                );
                num_cell(
                    ui,
                    egui::RichText::new(widgets::fmt_dec_grouped(&t.qty))
                        .monospace()
                        .size(theme::FONT_MD)
                        .color(theme::cur().text),
                    64.0,
                );
                ui.label(
                    egui::RichText::new(widgets::fmt_time_short(t.time))
                        .monospace()
                        .size(theme::FONT_MD)
                        .color(theme::cur().text_dim),
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
    let (tick, funding) = {
        let md_guard = state.md.lock();
        match md_guard.get(app.market, &app.page.symbol) {
            Some(md) => (
                md.ticker.clone(),
                md.mark.as_ref().and_then(|m| m.funding_rate),
            ),
            None => (None, None),
        }
    };

    // 第一行：交易对 | 最新价/涨跌 | 周期直选按钮 | 自选星标
    // wrapped：窄窗口时周期按钮换行而不是被裁掉
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(app.page.symbol.clone())
                .strong()
                .size(theme::FONT_LG),
        );
        ui.separator();
        if let Some(t) = &tick {
            let chg = widgets::f64_decimal(&t.price_change_percent);
            let c = theme::signed_color(chg);
            let px = widgets::f64_decimal(&t.last_price);
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
        // F1：自选星标固定在行最右
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let is_fav = app.state.favorites(app.market).contains(&app.page.symbol);
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
        });
    });

    // 第二行：24h OHLC / 交易量 / 交易额 /（合约）资金费率（wrapped 防窄窗截断）
    ui.horizontal_wrapped(|ui| {
        if let Some(t) = &tick {
            // Extend：值标签不在数字内部断行，horizontal_wrapped 会把整个标签挪到下一行
            let ext = |rt: egui::RichText| egui::Label::new(rt).wrap_mode(egui::TextWrapMode::Extend);
            let stat = |ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32| {
                ui.add(ext(theme::card_label(label)));
                ui.add(ext(
                    egui::RichText::new(value)
                        .monospace()
                        .size(theme::FONT_MD)
                        .color(color),
                ));
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
        }
    });
    ui.separator();
}
