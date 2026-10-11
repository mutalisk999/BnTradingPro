//! Portfolio 页：账户总览卡片 + 现货余额 + 合约资产与持仓

use crate::app::BnApp;
use crate::engine::Command;
use crate::ui::{theme, widgets};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use rust_decimal::Decimal;
use std::sync::Arc;

pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        widgets::placeholder(
            ui,
            ui.max_rect(),
            "未配置 API 密钥，无法查看资产（可在设置页配置）",
        );
        return;
    }

    // ---------- 合约账户概览卡片 ----------
    let fut = state.acct.lock().fut.clone();
    if let Some(a) = &fut {
        let f = |d: &Decimal| d.to_string().parse::<f64>().unwrap_or(0.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let cards: [(&str, f64, bool); 4] = [
                ("合约钱包余额", f(&a.total_wallet), false),
                ("未实现盈亏", f(&a.total_unrealized), true),
                ("保证金余额", f(&a.total_margin), false),
                ("可用余额", f(&a.available), false),
            ];
            for (name, v, colored) in cards {
                theme::card(ui, |ui| {
                    ui.set_min_width(ui.available_width() / 4.2);
                    ui.vertical(|ui| {
                        ui.label(theme::card_label(name));
                        let c = if colored {
                            theme::signed_color(v)
                        } else {
                            theme::TEXT
                        };
                        ui.label(
                            egui::RichText::new(format!("{v:.4}"))
                                .monospace()
                                .size(16.0)
                                .strong()
                                .color(c),
                        );
                    });
                });
            }
        });
        ui.horizontal(|ui| {
            let dual = state.acct.lock().fut_dual_side;
            ui.label(theme::card_label(if dual {
                "持仓模式：双向"
            } else {
                "持仓模式：单向"
            }));
            if ui.small_button("切换").clicked() {
                state.send_cmd(Command::SetDualSide { dual: !dual });
            }
            if ui.small_button("刷新账户").clicked() {
                state.send_cmd(Command::RefreshAccount);
            }
        });
    } else {
        ui.label(egui::RichText::new("合约账户加载中…").color(theme::TEXT_DIM));
    }

    ui.add_space(4.0);

    // ---------- 现货资产 / 合约资产 双栏 ----------
    ui.columns(2, |cols| {
        cols[0].vertical(|ui| {
            theme::section(ui, "现货资产");
            show_spot(&state, ui);
        });
        cols[1].vertical(|ui| {
            theme::section(ui, "合约资产");
            show_fut_assets(&state, ui);
        });
    });

    ui.add_space(4.0);
    theme::section(ui, "合约持仓");
    show_positions_table(app, ui);
}

fn show_spot(state: &Arc<crate::engine::state::SharedState>, ui: &mut egui::Ui) {
    let acct = state.acct.lock();
    let Some(a) = acct.spot.as_ref() else {
        ui.label(egui::RichText::new("加载中…").color(theme::TEXT_DIM));
        return;
    };
    let nonzero: Vec<_> = a
        .balances
        .iter()
        .filter(|b| b.free != Decimal::ZERO || b.locked != Decimal::ZERO)
        .collect();
    if !a.can_trade {
        ui.label(
            egui::RichText::new("（该 Key 无交易权限）")
                .size(11.5)
                .color(theme::WARN),
        );
    }
    if nonzero.is_empty() {
        ui.label(egui::RichText::new("暂无非零资产").color(theme::TEXT_DIM));
        return;
    }
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(70.0))
        .column(Column::auto().at_least(120.0))
        .column(Column::remainder())
        .header(20.0, |mut h| {
            for t in ["资产", "可用", "冻结"] {
                h.col(|ui| {
                    ui.label(theme::card_label(t));
                });
            }
        })
        .body(|mut body| {
            for b in &nonzero {
                body.row(20.0, |mut row| {
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&b.asset).strong().size(12.0));
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(b.free.normalize().to_string())
                                .monospace()
                                .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(b.locked.normalize().to_string())
                                .monospace()
                                .size(12.0)
                                .color(theme::TEXT_DIM),
                        );
                    });
                });
            }
        });
}

fn show_fut_assets(state: &Arc<crate::engine::state::SharedState>, ui: &mut egui::Ui) {
    let acct = state.acct.lock();
    let Some(a) = acct.fut.as_ref() else {
        ui.label(egui::RichText::new("加载中…").color(theme::TEXT_DIM));
        return;
    };
    let assets: Vec<_> = a
        .assets
        .iter()
        .filter(|b| b.wallet != Decimal::ZERO)
        .collect();
    if assets.is_empty() {
        ui.label(egui::RichText::new("暂无非零资产").color(theme::TEXT_DIM));
        return;
    }
    let f = |d: &Decimal| d.to_string().parse::<f64>().unwrap_or(0.0);
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(60.0))
        .column(Column::auto().at_least(105.0))
        .column(Column::auto().at_least(105.0))
        .column(Column::remainder())
        .header(20.0, |mut h| {
            for t in ["资产", "钱包余额", "可用", "未实现盈亏"] {
                h.col(|ui| {
                    ui.label(theme::card_label(t));
                });
            }
        })
        .body(|mut body| {
            for b in &assets {
                body.row(20.0, |mut row| {
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&b.asset).strong().size(12.0));
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.6}", f(&b.wallet)))
                                .monospace()
                                .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.6}", f(&b.available)))
                                .monospace()
                                .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        let up = f(&b.unrealized);
                        ui.label(
                            egui::RichText::new(format!("{up:+.6}"))
                                .monospace()
                                .size(12.0)
                                .color(theme::signed_color(up)),
                        );
                    });
                });
            }
        });
}

fn show_positions_table(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let positions: Vec<crate::binance::model::PositionRisk> = state
        .acct
        .lock()
        .fut_positions
        .iter()
        .filter(|p| p.amt != Decimal::ZERO)
        .cloned()
        .collect();
    if positions.is_empty() {
        ui.label(egui::RichText::new("暂无持仓").color(theme::TEXT_DIM));
        return;
    }
    let mut close_req: Option<(
        String,
        crate::binance::Side,
        Decimal,
        crate::binance::FutPosSide,
    )> = None;
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(85.0))
        .column(Column::auto().at_least(50.0))
        .column(Column::auto().at_least(85.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(105.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(50.0))
        .column(Column::remainder())
        .header(20.0, |mut h| {
            for t in [
                "交易对",
                "方向",
                "数量",
                "开仓价",
                "标记价",
                "未实现盈亏",
                "强平价",
                "杠杆",
                "操作",
            ] {
                h.col(|ui| {
                    ui.label(theme::card_label(t));
                });
            }
        })
        .body(|mut body| {
            for p in &positions {
                body.row(21.0, |mut row| {
                    let long = p.amt > Decimal::ZERO;
                    let f = |d: &Decimal| d.to_string().parse::<f64>().unwrap_or(0.0);
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&p.symbol).strong().size(12.0));
                    });
                    row.col(|ui| {
                        let (txt, c) = if long {
                            ("多", theme::UP)
                        } else {
                            ("空", theme::DOWN)
                        };
                        ui.label(egui::RichText::new(txt).color(c).strong().size(12.0));
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{}", p.amt.abs().normalize()))
                                .monospace()
                                .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.4}", f(&p.entry_price)))
                                .monospace()
                                .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.4}", f(&p.mark_price)))
                                .monospace()
                                .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        let up = f(&p.unrealized);
                        ui.label(
                            egui::RichText::new(format!("{up:+.4}"))
                                .monospace()
                                .size(12.0)
                                .color(theme::signed_color(up)),
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
                            .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{}x", f(&p.leverage) as i64)).size(12.0),
                        );
                    });
                    row.col(|ui| {
                        if ui.small_button("平仓").clicked() {
                            let side = if long {
                                crate::binance::Side::Sell
                            } else {
                                crate::binance::Side::Buy
                            };
                            close_req = Some((
                                p.symbol.clone(),
                                side,
                                p.amt.abs(),
                                crate::binance::FutPosSide::parse(&p.side),
                            ));
                        }
                    });
                });
            }
        });

    if let Some((symbol, side, qty, pos_side)) = close_req {
        let req = crate::binance::client::OrderRequest {
            symbol,
            side,
            kind: crate::binance::client::OrderKind::Market { qty },
            position_side: pos_side,
            reduce_only: !state.acct.lock().fut_dual_side,
            client_id: None,
        };
        let rows = crate::ui::trade::order_rows(&req);
        app.confirm = Some(crate::app::ConfirmOrder {
            market: crate::binance::Market::UmFutures,
            side: req.side,
            req,
            rows,
        });
    }
}
