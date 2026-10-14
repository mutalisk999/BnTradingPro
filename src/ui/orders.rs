//! 订单与持仓：当前挂单（撤销）、合约持仓（平仓/杠杆）、订单历史、成交历史

use crate::app::BnApp;
use crate::binance::model::PositionRisk;
use crate::binance::{FutPosSide, Side};
use crate::engine::Command;
use crate::ui::{theme, widgets};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use rust_decimal::Decimal;
use std::sync::Arc;

pub fn status_cn(s: &str) -> &'static str {
    let zh = match s {
        "NEW" => "待成交",
        "PARTIALLY_FILLED" => "部分成交",
        "FILLED" => "已成交",
        "CANCELED" => "已撤销",
        "PENDING_CANCEL" => "撤销中",
        "REJECTED" => "已拒绝",
        "EXPIRED" => "已过期",
        "EXPIRED_IN_MATCH" => "匹配过期",
        "NEW_ADL" => "ADL",
        _ => "其他",
    };
    crate::i18n::tr(zh)
}

fn type_cn(s: &str) -> &'static str {
    let zh = match s {
        "LIMIT" => "限价",
        "MARKET" => "市价",
        "STOP_LOSS" => "止损市价",
        "STOP_LOSS_LIMIT" => "止损限价",
        "TAKE_PROFIT" => "止盈市价",
        "TAKE_PROFIT_LIMIT" => "止盈限价",
        "STOP_MARKET" => "止损市价",
        "TAKE_PROFIT_MARKET" => "止盈市价",
        "TRAILING_STOP_MARKET" => "跟踪委托",
        "LIMIT_MAKER" => "只做 maker",
        _ => "其他",
    };
    crate::i18n::tr(zh)
}

/// 表头单元格：数值列（right=true）右对齐，与数据列对齐方式保持一致
fn head_cell(ui: &mut egui::Ui, t: &str, right: bool) {
    if right {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::card_label(t));
        });
    } else {
        ui.label(theme::card_label(t));
    }
}

/// 当前挂单
pub fn show_open(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let market = app.market;
    if !state.has_creds() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("未配置 API 密钥"),
            crate::i18n::tr("在设置页配置密钥后查看"),
        );
        return;
    }
    let list = state.orders.lock().open(market).clone();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} · {}{}",
                market.label(),
                list.len(),
                crate::i18n::tr("笔挂单")
            ))
            .size(theme::FONT_MD),
        );
        if theme::btn_default(ui, crate::i18n::tr("刷新"), true).clicked() {
            state.send_cmd(Command::RefreshOpenOrders { market });
        }
        if !list.is_empty() {
            let sym = app.page.symbol.clone();
            let sym_open = list.iter().filter(|o| o.symbol == sym).count();
            // 当前交易对没有挂单时禁用，避免"点了没反应"的静默空操作
            let cancel_all = theme::btn_danger(
                ui,
                &format!("{} {}({sym_open})", crate::i18n::tr("撤销全部"), sym),
                sym_open > 0,
            )
            .on_disabled_hover_text(crate::i18n::tr("该交易对无挂单"));
            if cancel_all.clicked() {
                state.send_cmd(Command::CancelAll {
                    market,
                    symbol: sym,
                });
            }
        }
    });
    ui.separator();

    if list.is_empty() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("暂无挂单"),
            crate::i18n::tr("下单后将显示在这里"),
        );
        return;
    }

    let mut cancel_id: Option<i64> = None;
    let mut cancel_sym = String::new();
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(105.0)) // 时间
        .column(Column::auto().at_least(85.0)) // 交易对
        .column(Column::auto().at_least(55.0)) // 方向
        .column(Column::auto().at_least(75.0)) // 类型
        .column(Column::auto().at_least(95.0)) // 价格
        .column(Column::auto().at_least(95.0)) // 数量
        .column(Column::auto().at_least(95.0)) // 已成交
        .column(Column::auto().at_least(70.0)) // 状态
        .column(Column::remainder()) // 操作
        .header(22.0, |mut h| {
            for (i, t) in [
                "时间",
                "交易对",
                "方向",
                "类型",
                "价格",
                "数量",
                "已成交",
                "状态",
                "操作",
            ]
            .into_iter()
            .enumerate()
            {
                h.col(|ui| head_cell(ui, crate::i18n::tr(t), (4..=6).contains(&i)));
            }
        })
        .body(|mut body| {
            for o in &list {
                // 行高 28：容纳 btn_danger（BTN_H）按钮
                body.row(28.0, |mut row| {
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(widgets::fmt_time(o.time)).size(theme::FONT_MD),
                        );
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&o.symbol).strong().size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        let c = if o.side == "BUY" {
                            theme::cur().up
                        } else {
                            theme::cur().down
                        };
                        let txt = if o.side == "BUY" {
                            crate::i18n::tr("买入")
                        } else {
                            crate::i18n::tr("卖出")
                        };
                        ui.label(
                            egui::RichText::new(txt)
                                .size(theme::FONT_MD)
                                .color(c)
                                .strong(),
                        );
                    });
                    row.col(|ui| {
                        let extra = if o.reduce_only {
                            format!(" {}", crate::i18n::tr("只减"))
                        } else {
                            String::new()
                        };
                        ui.label(
                            egui::RichText::new(format!("{}{extra}", type_cn(&o.order_type)))
                                .size(theme::FONT_MD),
                        );
                    });
                    row.col(|ui| {
                        // 止损/止盈市价单 price 为 0，真实价格在 stopPrice
                        let p = match o.price.filter(|p| !p.is_zero()) {
                            Some(p) => widgets::fmt_dec_grouped(&p),
                            None => match o.stop_price.filter(|s| !s.is_zero()) {
                                Some(s) => format!(
                                    "{} {}",
                                    crate::i18n::tr("触发"),
                                    widgets::fmt_dec_grouped(&s)
                                ),
                                None => crate::i18n::tr("市价").into(),
                            },
                        };
                        widgets::num_cell(ui, p, None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&o.orig_qty), None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&o.executed_qty), None);
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(status_cn(&o.status))
                                .size(theme::FONT_MD)
                                .color(theme::cur().accent),
                        );
                    });
                    row.col(|ui| {
                        if theme::btn_danger(ui, crate::i18n::tr("撤销"), true).clicked() {
                            cancel_id = Some(o.order_id);
                            cancel_sym = o.symbol.clone();
                        }
                    });
                });
            }
        });

    if let Some(id) = cancel_id {
        app.state.send_cmd(Command::CancelOrder {
            market,
            symbol: cancel_sym,
            order_id: id,
        });
    }
}

/// 合约持仓
pub fn show_positions(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("未配置 API 密钥"),
            crate::i18n::tr("在设置页配置密钥后查看"),
        );
        return;
    }
    let (positions, dual) = {
        let a = state.acct.lock();
        (
            a.fut_positions
                .iter()
                .filter(|p| p.amt != Decimal::ZERO)
                .cloned()
                .collect::<Vec<PositionRisk>>(),
            a.fut_dual_side,
        )
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} · {}{}",
                crate::i18n::tr("U本位合约"),
                positions.len(),
                crate::i18n::tr("个持仓")
            ))
            .size(theme::FONT_MD),
        );
        if theme::btn_default(ui, crate::i18n::tr("刷新"), true).clicked() {
            state.send_cmd(Command::RefreshAccount);
        }
        if theme::btn_default(
            ui,
            if dual {
                crate::i18n::tr("切换单向持仓")
            } else {
                crate::i18n::tr("切换双向持仓")
            },
            true,
        )
        .clicked()
        {
            state.send_cmd(Command::SetDualSide { dual: !dual });
        }
    });
    ui.separator();
    if positions.is_empty() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("暂无持仓"),
            crate::i18n::tr("下单后将显示在这里"),
        );
        return;
    }

    let mut close_req: Option<(String, Side, Decimal, FutPosSide)> = None;

    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(85.0))
        .column(Column::auto().at_least(55.0))
        .column(Column::auto().at_least(90.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(100.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(55.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for (i, t) in [
                "交易对",
                "方向",
                "数量",
                "开仓价",
                "标记价",
                "未实现盈亏",
                "强平价",
                "杠杆",
                "操作",
            ]
            .into_iter()
            .enumerate()
            {
                h.col(|ui| head_cell(ui, crate::i18n::tr(t), (2..=7).contains(&i)));
            }
        })
        .body(|mut body| {
            for p in &positions {
                // 行高 28：容纳 btn_danger（BTN_H）按钮
                body.row(28.0, |mut row| {
                    let long = p.amt > Decimal::ZERO;
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&p.symbol).strong().size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        let (txt, c) = if long {
                            (crate::i18n::tr("多"), theme::cur().up)
                        } else {
                            (crate::i18n::tr("空"), theme::cur().down)
                        };
                        ui.label(
                            egui::RichText::new(txt)
                                .color(c)
                                .strong()
                                .size(theme::FONT_MD),
                        );
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&p.amt.abs()), None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&p.entry_price), None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&p.mark_price), None);
                    });
                    row.col(|ui| {
                        let c = if p.unrealized >= Decimal::ZERO {
                            theme::cur().up
                        } else {
                            theme::cur().down
                        };
                        let s = widgets::fmt_dec_grouped(&p.unrealized);
                        let sign = if p.unrealized >= Decimal::ZERO {
                            "+"
                        } else {
                            ""
                        };
                        widgets::num_cell(ui, format!("{sign}{s}"), Some(c));
                    });
                    row.col(|ui| {
                        let liq = if p.liquidation_price.is_zero() {
                            "—".to_string()
                        } else {
                            widgets::fmt_dec_grouped(&p.liquidation_price)
                        };
                        widgets::num_cell(ui, liq, None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(
                            ui,
                            format!("{}x", crate::ui::widgets::f64_decimal(&p.leverage) as i64),
                            None,
                        );
                    });
                    row.col(|ui| {
                        if theme::btn_danger(ui, crate::i18n::tr("市价平仓"), true).clicked() {
                            let side = if long { Side::Sell } else { Side::Buy };
                            let pos_side = FutPosSide::parse(&p.side);
                            close_req = Some((p.symbol.clone(), side, p.amt.abs(), pos_side));
                        }
                    });
                });
            }
        });

    if let Some((symbol, side, qty, pos_side)) = close_req {
        crate::ui::trade::confirm_market_close(app, symbol, side, qty, pos_side);
    }
}

/// 订单历史
pub fn show_history(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("未配置 API 密钥"),
            crate::i18n::tr("在设置页配置密钥后查看"),
        );
        return;
    }
    let (hist, key) = {
        let o = state.orders.lock();
        (o.history.clone(), o.history_symbol.clone())
    };
    ui.horizontal(|ui| {
        match &key {
            Some((m, s)) => ui.label(
                egui::RichText::new(format!(
                    "{} {} · {}",
                    m.label(),
                    s,
                    crate::i18n::tr("最近 100 笔")
                ))
                .size(theme::FONT_MD),
            ),
            None => ui.label(
                egui::RichText::new(crate::i18n::tr("切换交易对后自动加载"))
                    .size(theme::FONT_MD)
                    .color(theme::cur().text_dim),
            ),
        };
        let resp = theme::btn_default(ui, crate::i18n::tr("刷新"), key.is_some());
        let resp = if key.is_none() {
            resp.on_disabled_hover_text(crate::i18n::tr("请先在交易页选择交易对"))
        } else {
            resp
        };
        if resp.clicked() {
            if let Some((m, s)) = key.clone() {
                state.send_cmd(Command::RefreshHistory {
                    market: m,
                    symbol: s,
                });
            }
        }
    });
    ui.separator();
    if hist.is_empty() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("暂无订单历史"),
            crate::i18n::tr("下单后将显示在这里"),
        );
        return;
    }
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(105.0))
        .column(Column::auto().at_least(85.0))
        .column(Column::auto().at_least(50.0))
        .column(Column::auto().at_least(75.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for (i, t) in [
                "时间",
                "交易对",
                "方向",
                "类型",
                "价格",
                "数量",
                "已成交",
                "状态",
            ]
            .into_iter()
            .enumerate()
            {
                h.col(|ui| head_cell(ui, crate::i18n::tr(t), (4..=6).contains(&i)));
            }
        })
        .body(|mut body| {
            for o in &hist {
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(widgets::fmt_time(o.time)).size(theme::FONT_MD),
                        );
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&o.symbol).size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        let c = if o.side == "BUY" {
                            theme::cur().up
                        } else {
                            theme::cur().down
                        };
                        let word = if o.side == "BUY" {
                            crate::i18n::tr("买入")
                        } else {
                            crate::i18n::tr("卖出")
                        };
                        ui.label(egui::RichText::new(word).color(c).size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(type_cn(&o.order_type)).size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        // 止损/止盈单 price 为 0 时展示触发价
                        let p = match o.price.filter(|p| !p.is_zero()) {
                            Some(p) => widgets::fmt_dec_grouped(&p),
                            None => match o.stop_price.filter(|s| !s.is_zero()) {
                                Some(s) => format!(
                                    "{} {}",
                                    crate::i18n::tr("触发"),
                                    widgets::fmt_dec_grouped(&s)
                                ),
                                None => crate::i18n::tr("市价").into(),
                            },
                        };
                        widgets::num_cell(ui, p, None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&o.orig_qty), None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&o.executed_qty), None);
                    });
                    row.col(|ui| {
                        let c = match o.status.as_str() {
                            "FILLED" => theme::cur().up,
                            "CANCELED" | "EXPIRED" | "REJECTED" => theme::cur().text_dim,
                            _ => theme::cur().accent,
                        };
                        ui.label(
                            egui::RichText::new(status_cn(&o.status))
                                .color(c)
                                .size(theme::FONT_MD),
                        );
                    });
                });
            }
        });
}

/// 成交历史
pub fn show_trades_history(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("未配置 API 密钥"),
            crate::i18n::tr("在设置页配置密钥后查看"),
        );
        return;
    }
    let (trades, hist_key) = {
        let o = state.orders.lock();
        (o.trades.clone(), o.history_symbol.clone())
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} {}{}",
                crate::i18n::tr("最近"),
                trades.len(),
                crate::i18n::tr("笔成交")
            ))
            .size(theme::FONT_MD),
        );
        // 尚未选定交易对时按钮置灰而非消失：工具条高度不跳变，禁用原因悬停可见
        let resp = theme::btn_default(ui, crate::i18n::tr("刷新"), hist_key.is_some());
        let resp = if hist_key.is_none() {
            resp.on_disabled_hover_text(crate::i18n::tr("请先在交易页选择交易对"))
        } else {
            resp
        };
        if resp.clicked() {
            if let Some((m, s)) = hist_key.clone() {
                state.send_cmd(Command::RefreshHistory {
                    market: m,
                    symbol: s,
                });
            }
        }
    });
    ui.separator();
    if trades.is_empty() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("暂无成交"),
            crate::i18n::tr("下单后将显示在这里"),
        );
        return;
    }
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(105.0))
        .column(Column::auto().at_least(85.0))
        .column(Column::auto().at_least(50.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(110.0))
        .column(Column::auto().at_least(90.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for (i, t) in [
                "时间",
                "交易对",
                "方向",
                "价格",
                "数量",
                "金额",
                "手续费",
                "角色",
            ]
            .into_iter()
            .enumerate()
            {
                h.col(|ui| head_cell(ui, crate::i18n::tr(t), (3..=6).contains(&i)));
            }
        })
        .body(|mut body| {
            for t in &trades {
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(widgets::fmt_time(t.time)).size(theme::FONT_MD),
                        );
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&t.symbol).size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        let (txt, c) = if t.is_buyer {
                            (crate::i18n::tr("买入"), theme::cur().up)
                        } else {
                            (crate::i18n::tr("卖出"), theme::cur().down)
                        };
                        ui.label(egui::RichText::new(txt).color(c).size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&t.price), None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(ui, widgets::fmt_dec_grouped(&t.qty), None);
                    });
                    row.col(|ui| {
                        let q = if t.quote_qty.is_zero() {
                            t.price * t.qty
                        } else {
                            t.quote_qty
                        };
                        widgets::num_cell(
                            ui,
                            widgets::fmt_dec_grouped(&q.normalize().round_dp(8)),
                            None,
                        );
                    });
                    row.col(|ui| {
                        widgets::num_cell(
                            ui,
                            format!(
                                "{} {}",
                                widgets::fmt_dec_grouped(&t.commission),
                                t.commission_asset
                            ),
                            Some(theme::cur().text_dim),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(if t.is_maker { "maker" } else { "taker" })
                                .size(theme::FONT_MD)
                                .color(theme::cur().text_dim),
                        );
                    });
                });
            }
        });
}
