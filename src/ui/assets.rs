//! 资产页：现货余额 + 合约账户概况

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
        widgets::placeholder(ui, ui.max_rect(), "未配置 API 密钥，无法查看资产");
        return;
    }
    ui.columns(2, |cols| {
        cols[0].heading(egui::RichText::new("现货账户").size(14.0));
        cols[1].heading(egui::RichText::new("U本位合约账户").size(14.0));
    });
    ui.separator();
    ui.columns(2, |cols| {
        show_spot(&state, &mut cols[0]);
        show_fut(&state, &mut cols[1]);
    });
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
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} 个非零资产", nonzero.len()))
                .size(12.0)
                .color(theme::TEXT_DIM),
        );
        if !a.can_trade {
            ui.label(
                egui::RichText::new("（该 Key 无交易权限）")
                    .size(11.5)
                    .color(theme::WARN),
            );
        }
    });
    ui.add_space(2.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, 20.0, nonzero.len().max(1), |ui, range| {
            egui::Grid::new("spot_bal_grid")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("资产")
                            .size(11.5)
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(
                        egui::RichText::new("可用")
                            .size(11.5)
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(
                        egui::RichText::new("冻结")
                            .size(11.5)
                            .color(theme::TEXT_DIM),
                    );
                    ui.end_row();
                    for b in nonzero
                        .iter()
                        .cloned()
                        .skip(range.start)
                        .take(range.end - range.start)
                    {
                        ui.label(egui::RichText::new(&b.asset).strong().size(12.0));
                        ui.label(
                            egui::RichText::new(b.free.normalize().to_string())
                                .monospace()
                                .size(12.0),
                        );
                        ui.label(
                            egui::RichText::new(b.locked.normalize().to_string())
                                .monospace()
                                .size(12.0)
                                .color(theme::TEXT_DIM),
                        );
                        ui.end_row();
                    }
                });
        });
}

fn show_fut(state: &Arc<crate::engine::state::SharedState>, ui: &mut egui::Ui) {
    let acct = state.acct.lock();
    let Some(a) = acct.fut.as_ref() else {
        ui.label(egui::RichText::new("加载中…").color(theme::TEXT_DIM));
        return;
    };
    let f = |d: &Decimal| d.to_string().parse::<f64>().unwrap_or(0.0);
    // 概览卡片
    let cards: [(&str, f64, bool); 4] = [
        ("钱包余额", f(&a.total_wallet), false),
        ("未实现盈亏", f(&a.total_unrealized), true),
        ("保证金余额", f(&a.total_margin), false),
        ("可用余额", f(&a.available), false),
    ];
    ui.columns(4, |cols| {
        for (i, (name, v, colored)) in cards.iter().enumerate() {
            cols[i].vertical(|ui| {
                ui.label(egui::RichText::new(*name).size(10.5).color(theme::TEXT_DIM));
                let c = if *colored {
                    theme::signed_color(*v)
                } else {
                    theme::TEXT
                };
                ui.label(
                    egui::RichText::new(format!("{v:.4}"))
                        .monospace()
                        .size(13.5)
                        .strong()
                        .color(c),
                );
            });
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let dual = acct.fut_dual_side;
        ui.label(
            egui::RichText::new(if dual {
                "持仓模式：双向"
            } else {
                "持仓模式：单向"
            })
            .size(11.5)
            .color(theme::TEXT_DIM),
        );
        if ui.small_button("切换").clicked() {
            state.send_cmd(Command::SetDualSide { dual: !dual });
        }
    });
    ui.separator();

    // 合约资产
    let assets: Vec<_> = a
        .assets
        .iter()
        .filter(|b| b.wallet != Decimal::ZERO)
        .collect();
    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(60.0))
        .column(Column::auto().at_least(110.0))
        .column(Column::auto().at_least(110.0))
        .column(Column::remainder())
        .header(20.0, |mut h| {
            for t in ["资产", "钱包余额", "可用", "未实现盈亏"] {
                h.col(|ui| {
                    ui.label(egui::RichText::new(t).size(11.5).color(theme::TEXT_DIM));
                });
            }
        })
        .body(|mut body| {
            for b in &assets {
                body.row(19.0, |mut row| {
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
