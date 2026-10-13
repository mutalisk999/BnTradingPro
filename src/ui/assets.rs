//! Portfolio 页：账户总览卡片 + 现货余额 + 合约资产与持仓

use crate::app::BnApp;
use crate::engine::Command;
use crate::ui::{theme, widgets};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use rust_decimal::Decimal;
use std::sync::Arc;

/// 账户列加载失败的显式错误行 + 重试入口（避免无限"加载中…"）
fn load_failed_row(
    state: &Arc<crate::engine::state::SharedState>,
    ui: &mut egui::Ui,
    err: Option<String>,
) {
    match err {
        Some(e) => {
            ui.label(
                egui::RichText::new(format!("{}{e}", crate::i18n::tr("加载失败: ")))
                    .size(11.5)
                    .color(theme::cur().err),
            );
            if ui.small_button(crate::i18n::tr("刷新账户")).clicked() {
                state.send_cmd(Command::RefreshAccount);
            }
        }
        None => {
            ui.label(egui::RichText::new(crate::i18n::tr("加载中…")).color(theme::cur().text_dim));
        }
    }
}

pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        widgets::placeholder(
            ui,
            ui.max_rect(),
            crate::i18n::tr("未配置 API 密钥，无法查看资产（可在设置页配置）"),
        );
        return;
    }

    // ---------- 合约账户概览卡片 ----------
    let fut = state.acct.lock().fut.clone();
    if let Some(a) = &fut {
        let f = |d: &Decimal| widgets::f64_decimal(d);
        // 卡片宽度一次性算好：循环内取 available_width 会随前一张卡片收窄，
        // 四张卡片逐张变矮变窄（原实现的实际 bug）
        let gap = 8.0;
        let cards: [(&str, f64, bool); 4] = [
            ("合约钱包余额", f(&a.total_wallet), false),
            ("未实现盈亏", f(&a.total_unrealized), true),
            ("保证金余额", f(&a.total_margin), false),
            ("可用余额", f(&a.available), false),
        ];
        let stat_card =
            |ui: &mut egui::Ui, card_w: f32, name: &'static str, v: f64, colored: bool| {
                theme::card(ui, |ui| {
                    // 24 = card 内边距（12×2）：外框总宽恰好 card_w
                    ui.set_min_width((card_w - 24.0).max(0.0));
                    ui.vertical(|ui| {
                        ui.label(theme::card_label(crate::i18n::tr(name)));
                        let c = if colored {
                            theme::signed_color(v)
                        } else {
                            theme::cur().text
                        };
                        ui.label(
                            egui::RichText::new(widgets::fmt_thousands(v, 4))
                                .monospace()
                                .size(16.0)
                                .strong()
                                .color(c),
                        );
                    });
                });
            };
        let avail_w = ui.available_width();
        if avail_w < 720.0 {
            // 窄窗：四张卡片改 2×2 两行，避免单行挤压溢出
            let card_w = ((avail_w - gap) / 2.0).max(80.0);
            for row in cards.chunks(2) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for &(name, v, colored) in row {
                        stat_card(ui, card_w, name, v, colored);
                    }
                });
            }
        } else {
            let card_w = ((avail_w - 3.0 * gap) / 4.0).max(80.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (name, v, colored) in cards {
                    stat_card(ui, card_w, name, v, colored);
                }
            });
        }
        ui.horizontal(|ui| {
            let dual = state.acct.lock().fut_dual_side;
            ui.label(theme::card_label(if dual {
                crate::i18n::tr("持仓模式：双向")
            } else {
                crate::i18n::tr("持仓模式：单向")
            }));
            if ui.small_button(crate::i18n::tr("切换")).clicked() {
                state.send_cmd(Command::SetDualSide { dual: !dual });
            }
            if ui.small_button(crate::i18n::tr("刷新账户")).clicked() {
                state.send_cmd(Command::RefreshAccount);
            }
        });
    } else {
        // 加载失败（如代理/密钥问题）时不能死等：给出显式重试入口
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(crate::i18n::tr("合约账户加载中…"))
                    .color(theme::cur().text_dim),
            );
            if ui.small_button(crate::i18n::tr("刷新账户")).clicked() {
                state.send_cmd(Command::RefreshAccount);
            }
        });
    }

    ui.add_space(4.0);

    // ---------- 现货资产 / 合约资产 双栏 ----------
    ui.columns(2, |cols| {
        cols[0].vertical(|ui| {
            theme::section(ui, crate::i18n::tr("现货资产"));
            show_spot(&state, ui);
        });
        cols[1].vertical(|ui| {
            theme::section(ui, crate::i18n::tr("合约资产"));
            show_fut_assets(&state, ui);
        });
    });

    ui.add_space(4.0);
    theme::section(ui, crate::i18n::tr("合约持仓"));
    show_positions_table(app, ui);
}

fn show_spot(state: &Arc<crate::engine::state::SharedState>, ui: &mut egui::Ui) {
    // 短锁拷贝后绘制：acct 是引擎写热点（WS 推送/REST 刷新），
    // 不能在整个表格构建期间持有锁（与 chart.rs 的 clone-and-release 纪律一致）
    let (nonzero, can_trade) = {
        let acct = state.acct.lock();
        let Some(a) = acct.spot.as_ref() else {
            let err = acct.spot_err.clone();
            drop(acct);
            load_failed_row(state, ui, err);
            return;
        };
        let rows: Vec<_> = a
            .balances
            .iter()
            .filter(|b| b.free != Decimal::ZERO || b.locked != Decimal::ZERO)
            .cloned()
            .collect();
        (rows, a.can_trade)
    };
    if !can_trade {
        ui.label(
            egui::RichText::new(crate::i18n::tr("（该 Key 无交易权限）"))
                .size(11.5)
                .color(theme::cur().warn),
        );
    }
    if nonzero.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tr("暂无非零资产")).color(theme::cur().text_dim));
        return;
    }
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(70.0))
        .column(Column::auto().at_least(120.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for t in ["资产", "可用", "冻结"] {
                h.col(|ui| {
                    ui.label(theme::card_label(crate::i18n::tr(t)));
                });
            }
        })
        .body(|mut body| {
            for b in &nonzero {
                body.row(22.0, |mut row| {
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
                                .color(theme::cur().text_dim),
                        );
                    });
                });
            }
        });
}

fn show_fut_assets(state: &Arc<crate::engine::state::SharedState>, ui: &mut egui::Ui) {
    // 短锁拷贝后绘制（同 show_spot）
    let assets: Vec<crate::binance::model::FutBalance> = {
        let acct = state.acct.lock();
        let Some(a) = acct.fut.as_ref() else {
            let err = acct.fut_err.clone();
            drop(acct);
            load_failed_row(state, ui, err);
            return;
        };
        a.assets
            .iter()
            .filter(|b| b.wallet != Decimal::ZERO)
            .cloned()
            .collect()
    };
    if assets.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tr("暂无非零资产")).color(theme::cur().text_dim));
        return;
    }
    let f = |d: &Decimal| widgets::f64_decimal(d);
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(60.0))
        .column(Column::auto().at_least(105.0))
        .column(Column::auto().at_least(105.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for t in ["资产", "钱包余额", "可用", "未实现盈亏"] {
                h.col(|ui| {
                    ui.label(theme::card_label(crate::i18n::tr(t)));
                });
            }
        })
        .body(|mut body| {
            for b in &assets {
                body.row(22.0, |mut row| {
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
        ui.label(egui::RichText::new(crate::i18n::tr("暂无持仓")).color(theme::cur().text_dim));
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
        .header(22.0, |mut h| {
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
                    ui.label(theme::card_label(crate::i18n::tr(t)));
                });
            }
        })
        .body(|mut body| {
            for p in &positions {
                body.row(22.0, |mut row| {
                    let long = p.amt > Decimal::ZERO;
                    // 价格/盈亏用 normalize 全精度展示：固定 {:.4} 会把
                    // 低价币（<0.0001）的开仓价、盈亏、强平价全部显示成 0
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&p.symbol).strong().size(12.0));
                    });
                    row.col(|ui| {
                        let (txt, c) = if long {
                            (crate::i18n::tr("多"), theme::cur().up)
                        } else {
                            (crate::i18n::tr("空"), theme::cur().down)
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
                            egui::RichText::new(p.entry_price.normalize().to_string())
                                .monospace()
                                .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(p.mark_price.normalize().to_string())
                                .monospace()
                                .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{:+}", p.unrealized.normalize()))
                                .monospace()
                                .size(12.0)
                                .color(theme::signed_color(widgets::f64_decimal(
                                    &p.unrealized,
                                ))),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(if p.liquidation_price.is_zero() {
                                "—".into()
                            } else {
                                p.liquidation_price.normalize().to_string()
                            })
                            .monospace()
                            .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}x",
                                widgets::f64_decimal(&p.leverage) as i64
                            ))
                            .size(12.0),
                        );
                    });
                    row.col(|ui| {
                        if ui.small_button(crate::i18n::tr("平仓")).clicked() {
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
        crate::ui::trade::confirm_market_close(app, symbol, side, qty, pos_side);
    }
}
