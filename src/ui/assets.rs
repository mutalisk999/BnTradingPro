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
                    .size(theme::FONT_SM)
                    .color(theme::cur().err),
            );
            if theme::btn_default(ui, crate::i18n::tr("刷新账户"), true).clicked() {
                state.send_cmd(Command::RefreshAccount);
            }
        }
        None => {
            widgets::loading_state(ui, crate::i18n::tr("加载中…"));
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
                                .size(theme::FONT_LG)
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
        // form_row：固定行高 + 垂直居中（horizontal 里标签先放置会贴顶错位）
        theme::form_row(ui, |ui| {
            let dual = state.acct.lock().fut_dual_side;
            ui.label(theme::card_label(if dual {
                crate::i18n::tr("持仓模式：双向")
            } else {
                crate::i18n::tr("持仓模式：单向")
            }));
            if theme::btn_default(ui, crate::i18n::tr("切换"), true).clicked() {
                state.send_cmd(Command::SetDualSide { dual: !dual });
            }
            if theme::btn_default(ui, crate::i18n::tr("刷新账户"), true).clicked() {
                state.send_cmd(Command::RefreshAccount);
            }
        });
    } else {
        // 加载失败（如代理/密钥问题）时不能死等：给出显式重试入口
        ui.horizontal(|ui| {
            widgets::loading_state(ui, crate::i18n::tr("合约账户加载中…"));
            if theme::btn_default(ui, crate::i18n::tr("刷新账户"), true).clicked() {
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
                .size(theme::FONT_SM)
                .color(theme::cur().warn),
        );
    }
    if nonzero.is_empty() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("暂无非零资产"),
            crate::i18n::tr("余额到账后自动显示"),
        );
        return;
    }
    TableBuilder::new(ui)
        // TableBuilder 默认 id_salt 固定 "__table_state"：同一 ui 下两张表会撞 id
        // （现货/合约两栏、下方持仓表都各需唯一 salt），冲突时 egui 会画出
        // "First use of Table ID …" 调试错误
        .id_salt("spot_assets")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(70.0))
        .column(Column::auto().at_least(120.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for t in ["资产", "可用", "冻结"] {
                h.col(|ui| {
                    if t == "资产" {
                        ui.label(theme::card_label(crate::i18n::tr(t)));
                    } else {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(theme::card_label(crate::i18n::tr(t)));
                        });
                    }
                });
            }
        })
        .body(|mut body| {
            for b in &nonzero {
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&b.asset).strong().size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(widgets::fmt_dec_grouped(&b.free))
                                    .monospace()
                                    .size(theme::FONT_MD),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(widgets::fmt_dec_grouped(&b.locked))
                                    .monospace()
                                    .size(theme::FONT_MD)
                                    .color(theme::cur().text_dim),
                            );
                        });
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
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("暂无非零资产"),
            crate::i18n::tr("余额到账后自动显示"),
        );
        return;
    }
    let f = |d: &Decimal| widgets::f64_decimal(d);
    TableBuilder::new(ui)
        // 同上：与现货表/持仓表区分 id
        .id_salt("fut_assets")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(60.0))
        .column(Column::auto().at_least(105.0))
        .column(Column::auto().at_least(105.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for t in ["资产", "钱包余额", "可用", "未实现盈亏"] {
                h.col(|ui| {
                    if t == "资产" {
                        ui.label(theme::card_label(crate::i18n::tr(t)));
                    } else {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(theme::card_label(crate::i18n::tr(t)));
                        });
                    }
                });
            }
        })
        .body(|mut body| {
            for b in &assets {
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&b.asset).strong().size(theme::FONT_MD));
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(widgets::fmt_dec_grouped(&b.wallet))
                                    .monospace()
                                    .size(theme::FONT_MD),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(widgets::fmt_dec_grouped(&b.available))
                                    .monospace()
                                    .size(theme::FONT_MD),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let s = widgets::fmt_dec_grouped(&b.unrealized);
                            let sign = if b.unrealized >= Decimal::ZERO {
                                "+"
                            } else {
                                ""
                            };
                            ui.label(
                                egui::RichText::new(format!("{sign}{s}"))
                                    .monospace()
                                    .size(theme::FONT_MD)
                                    .color(theme::signed_color(f(&b.unrealized))),
                            );
                        });
                    });
                });
            }
        });
}

fn show_positions_table(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    // 持仓均为 U 本位合约；价格小数位 = quote 资产侧精度（tick 小数位），
    // 数量 = base 资产侧精度（step 小数位），与订单页持仓表一致
    let market = crate::binance::Market::UmFutures;
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.price_dp() as usize).unwrap_or(2))
    };
    let qtd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4))
    };
    let positions: Vec<crate::binance::model::PositionRisk> = state
        .acct
        .lock()
        .fut_positions
        .iter()
        .filter(|p| p.amt != Decimal::ZERO)
        .cloned()
        .collect();
    if positions.is_empty() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("暂无持仓"),
            crate::i18n::tr("下单后将显示在这里"),
        );
        return;
    }
    let mut close_req: Option<(
        String,
        crate::binance::Side,
        Decimal,
        crate::binance::FutPosSide,
    )> = None;
    TableBuilder::new(ui)
        // 同上：与现货/合约资产表区分 id
        .id_salt("fut_positions")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::auto().at_least(85.0))
        .column(Column::auto().at_least(50.0))
        .column(Column::auto().at_least(85.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(105.0))
        .column(Column::auto().at_least(95.0))
        .column(Column::auto().at_least(50.0))
        .column(Column::remainder())
        .header(22.0, |mut h| {
            for (i, t) in [
                "交易对",
                "方向",
                "数量",
                "开仓价",
                "损益两平价",
                "标记价",
                "未实现盈亏",
                "强平价",
                "杠杆",
                "操作",
            ]
            .into_iter()
            .enumerate()
            {
                // 数值列（数量/开仓价/损益两平价/标记价/盈亏/强平价/杠杆）表头同步右对齐
                h.col(|ui| {
                    if (2..=8).contains(&i) {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(theme::card_label(crate::i18n::tr(t)));
                        });
                    } else {
                        ui.label(theme::card_label(crate::i18n::tr(t)));
                    }
                });
            }
        })
        .body(|mut body| {
            for p in &positions {
                // 行高 28：容纳 btn_danger（BTN_H）按钮（与订单页持仓表一致）
                body.row(28.0, |mut row| {
                    let long = p.amt > Decimal::ZERO;
                    // 价格/盈亏按交易对规则精度（tick 小数位 = quote 侧）定长显示：
                    // 低价币规则精度高，不会出现固定小数位把价格/盈亏显示成 0 的问题
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
                        widgets::num_cell(
                            ui,
                            widgets::fmt_dec_fixed(&p.amt.abs(), qtd(&p.symbol)),
                            None,
                        );
                    });
                    row.col(|ui| {
                        widgets::num_cell(
                            ui,
                            widgets::fmt_dec_fixed(&p.entry_price, pxd(&p.symbol)),
                            None,
                        );
                    });
                    row.col(|ui| {
                        // 损益两平价（含手续费/资金费；接口无值时为 0 → 显示 —）
                        let bep = if p.break_even_price.is_zero() {
                            "—".to_string()
                        } else {
                            widgets::fmt_dec_fixed(&p.break_even_price, pxd(&p.symbol))
                        };
                        widgets::num_cell(ui, bep, None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(
                            ui,
                            widgets::fmt_dec_fixed(&p.mark_price, pxd(&p.symbol)),
                            None,
                        );
                    });
                    row.col(|ui| {
                        let s = widgets::fmt_dec_fixed(&p.unrealized, pxd(&p.symbol));
                        let sign = if p.unrealized >= Decimal::ZERO {
                            "+"
                        } else {
                            ""
                        };
                        widgets::num_cell(
                            ui,
                            format!("{sign}{s}"),
                            Some(theme::signed_color(widgets::f64_decimal(&p.unrealized))),
                        );
                    });
                    row.col(|ui| {
                        let liq = if p.liquidation_price.is_zero() {
                            "—".to_string()
                        } else {
                            widgets::fmt_dec_fixed(&p.liquidation_price, pxd(&p.symbol))
                        };
                        widgets::num_cell(ui, liq, None);
                    });
                    row.col(|ui| {
                        widgets::num_cell(
                            ui,
                            format!("{}x", widgets::f64_decimal(&p.leverage) as i64),
                            None,
                        );
                    });
                    row.col(|ui| {
                        // 破坏性操作统一危险按钮样式（与订单页市价平仓一致）
                        if theme::btn_danger(ui, crate::i18n::tr("平仓"), true).clicked() {
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
