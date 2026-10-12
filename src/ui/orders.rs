//! 订单与持仓：当前挂单（撤销）、合约持仓（平仓/杠杆）、订单历史、成交历史

use crate::app::BnApp;
use crate::binance::client::{OrderKind, OrderRequest};
use crate::binance::model::{OrderInfo, PositionRisk};
use crate::binance::{FutPosSide, Market, Side};
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

/// 当前挂单
pub fn show_open(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let market = app.market;
    if !state.has_creds() {
        ui.label(
            egui::RichText::new(crate::i18n::tr("未配置 API 密钥")).color(theme::cur().text_dim),
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
            .size(12.5),
        );
        if ui.small_button(crate::i18n::tr("刷新")).clicked() {
            state.send_cmd(Command::RefreshOpenOrders { market });
        }
        if !list.is_empty() {
            let sym = app.symbol.clone();
            let sym_open = list.iter().filter(|o| o.symbol == sym).count();
            if ui
                .small_button(format!(
                    "{} {}({sym_open})",
                    crate::i18n::tr("撤销全部"),
                    sym
                ))
                .clicked()
                && sym_open > 0
            {
                state.send_cmd(Command::CancelAll {
                    market,
                    symbol: sym,
                });
            }
        }
    });
    ui.separator();

    if list.is_empty() {
        widgets::placeholder(
            ui,
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 40.0)),
            "暂无挂单",
        );
        return;
    }
    let dp_of = |sym: &str| {
        state
            .rule_of(market, sym)
            .map(|r| r.price_dp() as usize)
            .unwrap_or(2)
    };
    let qdp_of = |sym: &str| {
        state
            .rule_of(market, sym)
            .map(|r| r.qty_dp() as usize)
            .unwrap_or(4)
    };

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
            for t in [
                crate::i18n::tr("时间"),
                crate::i18n::tr("交易对"),
                crate::i18n::tr("方向"),
                crate::i18n::tr("类型"),
                crate::i18n::tr("价格"),
                crate::i18n::tr("数量"),
                crate::i18n::tr("已成交"),
                crate::i18n::tr("状态"),
                crate::i18n::tr("操作"),
            ] {
                h.col(|ui| {
                    ui.label(
                        egui::RichText::new(t)
                            .size(11.5)
                            .color(theme::cur().text_dim),
                    );
                });
            }
        })
        .body(|mut body| {
            for o in &list {
                body.row(22.0, |mut row| {
                    let dp = dp_of(&o.symbol);
                    let qdp = qdp_of(&o.symbol);
                    row.col(|ui| {
                        ui.label(egui::RichText::new(widgets::fmt_time(o.time)).size(11.5));
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&o.symbol).strong().size(11.5));
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
                        ui.label(egui::RichText::new(txt).size(11.5).color(c).strong());
                    });
                    row.col(|ui| {
                        let extra = if o.reduce_only { " 只减" } else { "" };
                        ui.label(
                            egui::RichText::new(format!("{}{extra}", type_cn(&o.order_type)))
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        // 止损/止盈市价单 price 为 0，真实价格在 stopPrice
                        let p = match o.price.filter(|p| !p.is_zero()) {
                            Some(p) => format!("{:.*}", dp, crate::ui::widgets::f64_decimal(&p)),
                            None => match o.stop_price.filter(|s| !s.is_zero()) {
                                Some(s) => format!(
                                    "{} {:.*}",
                                    crate::i18n::tr("触发"),
                                    dp,
                                    crate::ui::widgets::f64_decimal(&s)
                                ),
                                None => crate::i18n::tr("市价").into(),
                            },
                        };
                        ui.label(egui::RichText::new(p).monospace().size(11.5));
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{:.*}",
                                qdp,
                                crate::ui::widgets::f64_decimal(&o.orig_qty)
                            ))
                            .monospace()
                            .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{:.*}",
                                qdp,
                                crate::ui::widgets::f64_decimal(&o.executed_qty)
                            ))
                            .monospace()
                            .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(status_cn(&o.status))
                                .size(11.5)
                                .color(theme::cur().accent),
                        );
                    });
                    row.col(|ui| {
                        if ui.small_button(crate::i18n::tr("撤销")).clicked() {
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
        ui.label(
            egui::RichText::new(crate::i18n::tr("未配置 API 密钥")).color(theme::cur().text_dim),
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
            .size(12.5),
        );
        if ui.small_button(crate::i18n::tr("刷新")).clicked() {
            state.send_cmd(Command::RefreshAccount);
        }
        if ui
            .small_button(if dual {
                crate::i18n::tr("切换单向持仓")
            } else {
                crate::i18n::tr("切换双向持仓")
            })
            .clicked()
        {
            state.send_cmd(Command::SetDualSide { dual: !dual });
        }
    });
    ui.separator();
    if positions.is_empty() {
        widgets::placeholder(
            ui,
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 40.0)),
            "暂无持仓",
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
            for t in [
                crate::i18n::tr("交易对"),
                crate::i18n::tr("方向"),
                crate::i18n::tr("数量"),
                crate::i18n::tr("开仓价"),
                crate::i18n::tr("标记价"),
                crate::i18n::tr("未实现盈亏"),
                crate::i18n::tr("强平价"),
                crate::i18n::tr("杠杆"),
                crate::i18n::tr("操作"),
            ] {
                h.col(|ui| {
                    ui.label(
                        egui::RichText::new(t)
                            .size(11.5)
                            .color(theme::cur().text_dim),
                    );
                });
            }
        })
        .body(|mut body| {
            for p in &positions {
                body.row(22.0, |mut row| {
                    let long = p.amt > Decimal::ZERO;
                    let f = |d: &Decimal| crate::ui::widgets::f64_decimal(d);
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&p.symbol).strong().size(11.5));
                    });
                    row.col(|ui| {
                        let (txt, c) = if long {
                            ("多", theme::cur().up)
                        } else {
                            ("空", theme::cur().down)
                        };
                        ui.label(egui::RichText::new(txt).color(c).strong().size(11.5));
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{}", p.amt.abs().normalize()))
                                .monospace()
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.4}", f(&p.entry_price)))
                                .monospace()
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.4}", f(&p.mark_price)))
                                .monospace()
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        let up = f(&p.unrealized);
                        let c = if up >= 0.0 {
                            theme::cur().up
                        } else {
                            theme::cur().down
                        };
                        ui.label(
                            egui::RichText::new(format!("{up:+.4}"))
                                .monospace()
                                .color(c)
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        let liq = f(&p.liquidation_price);
                        ui.label(
                            egui::RichText::new(if liq > 0.0 {
                                format!("{liq:.2}")
                            } else {
                                "—".into()
                            })
                            .monospace()
                            .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{}x", f(&p.leverage) as i64)).size(11.5),
                        );
                    });
                    row.col(|ui| {
                        if ui.small_button(crate::i18n::tr("市价平仓")).clicked() {
                            let side = if long { Side::Sell } else { Side::Buy };
                            let pos_side = FutPosSide::parse(&p.side);
                            close_req = Some((p.symbol.clone(), side, p.amt.abs(), pos_side));
                        }
                    });
                });
            }
        });

    if let Some((symbol, side, qty, pos_side)) = close_req {
        if !state.has_creds() {
            return;
        }
        let req = OrderRequest {
            symbol,
            side,
            kind: OrderKind::Market { qty },
            position_side: pos_side,
            reduce_only: !dual,
            client_id: None,
        };
        // 走确认弹窗，避免误触直接发真实市价单
        let rows = crate::ui::trade::order_rows(&req);
        app.confirm = Some(crate::app::ConfirmOrder {
            market: Market::UmFutures,
            side: req.side,
            req,
            rows,
        });
    }
}

/// 订单历史
pub fn show_history(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        ui.label(
            egui::RichText::new(crate::i18n::tr("未配置 API 密钥")).color(theme::cur().text_dim),
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
                .size(12.5),
            ),
            None => ui.label(
                egui::RichText::new(crate::i18n::tr("切换交易对后自动加载"))
                    .size(12.5)
                    .color(theme::cur().text_dim),
            ),
        };
        if let Some((m, s)) = key.clone() {
            if ui.small_button(crate::i18n::tr("刷新")).clicked() {
                state.send_cmd(Command::RefreshHistory {
                    market: m,
                    symbol: s,
                });
            }
        }
    });
    ui.separator();
    if hist.is_empty() {
        widgets::placeholder(
            ui,
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 40.0)),
            "暂无订单历史",
        );
        return;
    }
    let market = key.map(|(m, _)| m).unwrap_or(app.market);
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
            for t in [
                crate::i18n::tr("时间"),
                crate::i18n::tr("交易对"),
                crate::i18n::tr("方向"),
                crate::i18n::tr("类型"),
                crate::i18n::tr("价格"),
                crate::i18n::tr("数量"),
                crate::i18n::tr("已成交"),
                crate::i18n::tr("状态"),
            ] {
                h.col(|ui| {
                    ui.label(
                        egui::RichText::new(t)
                            .size(11.5)
                            .color(theme::cur().text_dim),
                    );
                });
            }
        })
        .body(|mut body| {
            for o in &hist {
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        ui.label(egui::RichText::new(widgets::fmt_time(o.time)).size(11.5));
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&o.symbol).size(11.5));
                    });
                    row.col(|ui| {
                        let c = if o.side == "BUY" {
                            theme::cur().up
                        } else {
                            theme::cur().down
                        };
                        ui.label(egui::RichText::new(o.side_label()).color(c).size(11.5));
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(type_cn(&o.order_type)).size(11.5));
                    });
                    row.col(|ui| {
                        // 止损/止盈单 price 为 0 时展示触发价
                        let p = match o.price.filter(|p| !p.is_zero()) {
                            Some(p) => p.normalize().to_string(),
                            None => match o.stop_price.filter(|s| !s.is_zero()) {
                                Some(s) => {
                                    format!("{} {}", crate::i18n::tr("触发"), s.normalize())
                                }
                                None => crate::i18n::tr("市价").into(),
                            },
                        };
                        ui.label(egui::RichText::new(p).monospace().size(11.5));
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(o.orig_qty.normalize().to_string())
                                .monospace()
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(o.executed_qty.normalize().to_string())
                                .monospace()
                                .size(11.5),
                        );
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
                                .size(11.5),
                        );
                    });
                });
            }
        });
    let _ = market;
}

/// 成交历史
pub fn show_trades_history(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        ui.label(
            egui::RichText::new(crate::i18n::tr("未配置 API 密钥")).color(theme::cur().text_dim),
        );
        return;
    }
    let trades = state.orders.lock().trades.clone();
    let hist_key = state.orders.lock().history_symbol.clone();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} {}{}",
                crate::i18n::tr("最近"),
                trades.len(),
                crate::i18n::tr("笔成交")
            ))
            .size(12.5),
        );
        if ui.small_button(crate::i18n::tr("刷新")).clicked() {
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
        widgets::placeholder(
            ui,
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 40.0)),
            "暂无成交",
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
            for t in [
                crate::i18n::tr("时间"),
                crate::i18n::tr("交易对"),
                crate::i18n::tr("方向"),
                crate::i18n::tr("价格"),
                crate::i18n::tr("数量"),
                crate::i18n::tr("金额"),
                crate::i18n::tr("手续费"),
                crate::i18n::tr("角色"),
            ] {
                h.col(|ui| {
                    ui.label(
                        egui::RichText::new(t)
                            .size(11.5)
                            .color(theme::cur().text_dim),
                    );
                });
            }
        })
        .body(|mut body| {
            for t in &trades {
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        ui.label(egui::RichText::new(widgets::fmt_time(t.time)).size(11.5));
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&t.symbol).size(11.5));
                    });
                    row.col(|ui| {
                        let (txt, c) = if t.is_buyer {
                            ("买入", theme::cur().up)
                        } else {
                            ("卖出", theme::cur().down)
                        };
                        ui.label(egui::RichText::new(txt).color(c).size(11.5));
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(t.price.normalize().to_string())
                                .monospace()
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(t.qty.normalize().to_string())
                                .monospace()
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        let q = if t.quote_qty.is_zero() {
                            t.price * t.qty
                        } else {
                            t.quote_qty
                        };
                        ui.label(
                            egui::RichText::new(q.normalize().round_dp(8).to_string())
                                .monospace()
                                .size(11.5),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {}",
                                t.commission.normalize(),
                                t.commission_asset
                            ))
                            .monospace()
                            .size(11.5)
                            .color(theme::cur().text_dim),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(if t.is_maker { "maker" } else { "taker" })
                                .size(11.5)
                                .color(theme::cur().text_dim),
                        );
                    });
                });
            }
        });
}

// OrderInfo 辅助
trait OrderInfoExt {
    fn side_label(&self) -> &'static str;
}

impl OrderInfoExt for OrderInfo {
    fn side_label(&self) -> &'static str {
        if self.side == "BUY" {
            "买入"
        } else {
            "卖出"
        }
    }
}
