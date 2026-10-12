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
            .size(13.0)
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
            price,
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

    // 买盘（低→高）
    for (i, (price, qty)) in bids.iter().enumerate() {
        book_row(
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
    price: &Decimal,
    dp: usize,
    qty_f: &f64,
    qdp: usize,
    price_color: egui::Color32,
) {
    // 显示用 f64；点击填单必须用 Decimal 原文——dp 回退（规则未加载时=2）
    // 会把 0.00001234 这类价格四舍五入成 "0.00"
    let price_f = widgets::f64_decimal(price);
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
        egui::FontId::monospace(12.0),
        price_color,
    );
    // 数量（右）
    ui.painter().text(
        egui::Pos2::new(rect.right() - 2.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("{qty_f:.*}", qdp),
        egui::FontId::monospace(12.0),
        theme::cur().text,
    );
    if resp.clicked() {
        // 点击填单用交易所精度原文（Decimal），不能用 UI 四舍五入后的值
        app.page.form.set_price(&price.to_string());
    }
    // 明示"点价格即填单"这一核心交互，并给出悬停提示
    resp.on_hover_text(crate::i18n::tr("点击填入下单价格"));
}

/// 逐笔成交
pub fn show_trades(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let dp = state
        .with_rule(app.market, &app.page.symbol, |r| {
            r.map(|x| x.price_dp() as usize)
        })
        .unwrap_or(2);

    ui.add_space(4.0);
    ui.heading(
        egui::RichText::new(crate::i18n::tr("成交明细"))
            .size(13.0)
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
    egui::Grid::new("trades_grid")
        .num_columns(3)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            for t in ["价格", "数量", "时间"] {
                ui.label(
                    egui::RichText::new(crate::i18n::tr(t))
                        .size(11.0)
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
                ui.label(
                    egui::RichText::new(format!(
                        "{:.*}",
                        dp,
                        widgets::f64_decimal(&t.price)
                    ))
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
                    egui::RichText::new(widgets::fmt_time_short(t.time))
                        .monospace()
                        .size(12.0)
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

    // 第一行：交易对 | 最新价/涨跌 | 周期直选按钮
    // wrapped：窄窗口时周期按钮换行而不是被裁掉
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(app.page.symbol.clone()).strong().size(16.5));
        ui.separator();
        if let Some(t) = &tick {
            let chg = widgets::f64_decimal(&t.price_change_percent);
            let c = theme::signed_color(chg);
            let px = widgets::f64_decimal(&t.last_price);
            ui.label(
                egui::RichText::new(widgets::fmt_thousands(px, dp))
                    .monospace()
                    .strong()
                    .size(21.0)
                    .color(c),
            );
            ui.label(
                egui::RichText::new(format!("{chg:+.2}%"))
                    .monospace()
                    .size(13.5)
                    .color(c),
            );
        }
        ui.separator();
        // 周期直选（无需下拉）；最小高度 24px 保证最常见的点击目标可命中
        for iv in Interval::all() {
            let active = app.page.interval == iv;
            let txt = if active {
                egui::RichText::new(iv.label())
                    .size(12.5)
                    .strong()
                    .color(theme::cur().accent)
            } else {
                egui::RichText::new(iv.label())
                    .size(12.0)
                    .color(theme::cur().text_dim)
            };
            // min_size 直接加高命中区：add_sized 的 justify 布局会破坏
            // horizontal_wrapped 的换行判定（按钮会挤在首行直到被裁剪）
            if ui.add(egui::Button::selectable(active, txt).min_size(egui::vec2(0.0, 24.0))).clicked() {
                app.switch_interval(iv);
            }
        }
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
                        .size(12.0)
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
                        .size(12.0)
                        .strong()
                        .color(color),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
        }
    });
    ui.separator();
}
