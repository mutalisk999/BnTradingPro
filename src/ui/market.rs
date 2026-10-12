//! 盘口（深度）与逐笔成交列表

use crate::app::BnApp;
use crate::ui::theme;
use eframe::egui;
use rust_decimal::Decimal;
use std::sync::Arc;

/// 盘口面板：点击价格 → 填入下单价格
pub fn show_book(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let dp = state
        .rule_of(app.market, &app.symbol)
        .map(|r| r.price_dp() as usize)
        .unwrap_or(2);
    let qdp = state
        .rule_of(app.market, &app.symbol)
        .map(|r| r.qty_dp() as usize)
        .unwrap_or(4);

    ui.add_space(4.0);
    ui.heading(
        egui::RichText::new(crate::i18n::tr("盘口 20 档"))
            .size(13.0)
            .color(theme::cur().text),
    );
    ui.separator();

    let md = state.md.lock();
    let Some(md) = md.get(app.market, &app.symbol) else {
        ui.label(egui::RichText::new(crate::i18n::tr("加载中…")).color(theme::cur().text_dim));
        return;
    };

    let asks = &md.depth.asks;
    let bids = &md.depth.bids;
    if asks.is_empty() && bids.is_empty() {
        ui.label(
            egui::RichText::new(crate::i18n::tr("等待深度数据…")).color(theme::cur().text_dim),
        );
        return;
    }

    let n = 9; // 每侧显示档数
    let ask_show: Vec<&crate::binance::model::Level> = asks.iter().take(n).rev().collect();
    let bid_show: Vec<&crate::binance::model::Level> = bids.iter().take(n).collect();

    // 累计量用于深度条
    let mut cum = 0.0f64;
    let mut ask_cums = Vec::with_capacity(ask_show.len());
    for a in &ask_show {
        cum += a.qty.to_string().parse::<f64>().unwrap_or(0.0);
        ask_cums.push(cum);
    }
    let ask_total = cum;
    cum = 0.0;
    let mut bid_cums = Vec::with_capacity(bid_show.len());
    for b in &bid_show {
        cum += b.qty.to_string().parse::<f64>().unwrap_or(0.0);
        bid_cums.push(cum);
    }
    let bid_total = cum;
    let max_cum = ask_total.max(bid_total).max(0.000001);

    let row_h = 18.0;
    let w = ui.available_width();

    // 卖盘（高→低）
    for (i, a) in ask_show.iter().enumerate() {
        let ratio = ask_cums[i] / max_cum;
        book_row(
            ui,
            app,
            row_h,
            w,
            ratio,
            theme::cur().down.gamma_multiply(0.28),
            &a.price.to_string(),
            dp,
            &a.qty.to_string(),
            qdp,
            theme::cur().down,
        );
    }

    // 价差行（单边盘口时直接显示可得一侧价格，避免中间价=半价的错误数值）
    let (best_bid, best_ask) = match (bids.first(), asks.first()) {
        (Some(b), Some(a)) => (b.price, a.price),
        _ => (Decimal::ZERO, Decimal::ZERO),
    };
    let (mid, spread) = if best_bid.is_zero() || best_ask.is_zero() {
        let only = if best_bid.is_zero() {
            best_ask
        } else {
            best_bid
        };
        (only, Decimal::ZERO)
    } else {
        (
            (best_bid + best_ask) / Decimal::from(2),
            best_ask - best_bid,
        )
    };
    let mid_f = mid.to_string().parse::<f64>().unwrap_or(0.0);
    let spread_pct = if mid_f > 0.0 {
        spread.to_string().parse::<f64>().unwrap_or(0.0) / mid_f * 100.0
    } else {
        0.0
    };
    let spread_txt = if spread.is_zero() && (best_bid.is_zero() || best_ask.is_zero()) {
        String::new()
    } else {
        format!(
            "{} {:.*} ({:.3}%)",
            crate::i18n::tr("价差"),
            dp,
            spread.to_string().parse::<f64>().unwrap_or(0.0),
            spread_pct
        )
    };
    ui.allocate_ui(egui::vec2(w, 24.0), |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{:.*}", dp, mid_f))
                    .monospace()
                    .strong()
                    .size(13.5)
                    .color(theme::cur().text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(spread_txt)
                        .size(11.0)
                        .color(theme::cur().text_dim),
                );
            });
        });
    });

    for (i, b) in bid_show.iter().enumerate() {
        let ratio = bid_cums[i] / max_cum;
        book_row(
            ui,
            app,
            row_h,
            w,
            ratio,
            theme::cur().up.gamma_multiply(0.28),
            &b.price.to_string(),
            dp,
            &b.qty.to_string(),
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
    price_s: &str,
    dp: usize,
    qty_s: &str,
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
    let price_txt: String = format!("{:.*}", dp, price_s.parse::<f64>().unwrap_or(0.0));
    ui.painter().text(
        egui::Pos2::new(rect.left() + 2.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        price_txt,
        egui::FontId::monospace(12.0),
        price_color,
    );
    // 数量（右）
    let qty_txt: String = format!("{:.*}", qdp, qty_s.parse::<f64>().unwrap_or(0.0));
    ui.painter().text(
        egui::Pos2::new(rect.right() - 2.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        qty_txt,
        egui::FontId::monospace(12.0),
        theme::cur().text,
    );
    if resp.clicked() {
        app.form.set_price(price_s);
    }
}

/// 逐笔成交
pub fn show_trades(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let dp = state
        .rule_of(app.market, &app.symbol)
        .map(|r| r.price_dp() as usize)
        .unwrap_or(2);

    ui.add_space(4.0);
    ui.heading(
        egui::RichText::new(crate::i18n::tr("成交明细"))
            .size(13.0)
            .color(theme::cur().text),
    );
    ui.separator();

    let md = state.md.lock();
    let Some(md) = md.get(app.market, &app.symbol) else {
        ui.label(egui::RichText::new(crate::i18n::tr("加载中…")).color(theme::cur().text_dim));
        return;
    };

    ui.set_min_width(0.0);
    egui::Grid::new("trades_grid")
        .num_columns(3)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::tr("价格"))
                    .size(11.0)
                    .color(theme::cur().text_dim),
            );
            ui.label(
                egui::RichText::new(crate::i18n::tr("数量"))
                    .size(11.0)
                    .color(theme::cur().text_dim),
            );
            ui.label(
                egui::RichText::new(crate::i18n::tr("时间"))
                    .size(11.0)
                    .color(theme::cur().text_dim),
            );
            ui.end_row();

            for t in md.trades.iter().rev().take(30) {
                let price_f: f64 = t.price.to_string().parse().unwrap_or(0.0);
                let col = if t.buyer_maker {
                    theme::cur().down
                } else {
                    theme::cur().up
                };
                ui.label(
                    egui::RichText::new(format!("{:.*}", dp, price_f))
                        .monospace()
                        .size(12.0)
                        .color(col),
                );
                ui.label(
                    egui::RichText::new(t.qty.normalize().to_string())
                        .monospace()
                        .size(12.0)
                        .color(theme::cur().text),
                );
                ui.label(
                    egui::RichText::new(crate::ui::widgets::fmt_time_short(t.time))
                        .monospace()
                        .size(12.0)
                        .color(theme::cur().text_dim),
                );
                ui.end_row();
            }
        });
}
