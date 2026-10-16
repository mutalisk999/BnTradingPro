//! Portfolio 页：现货资产与合约资产分区总览（双卡片布局）
//!
//! 页头（标题 + 全局刷新）之下，现货与合约各一张卡片彻底分开：
//! 卡头 = 市场名 + 各自的操作/徽章槽，卡体 = 明细表；合约卡另有
//! USDT 计价的迷你统计行（可跨资产加总，现货不做跨币种假合计）。
//! 本页只关注资产本身——持仓/平仓在期货交易页的持仓页签。

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

/// 卡头：市场名（正文字号加粗，层级压过页面标题）+ 右侧槽位
fn card_header(ui: &mut egui::Ui, title: &str, add_right: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong().size(theme::FONT_BODY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_right);
    });
    ui.add_space(2.0);
    ui.separator();
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

    // ---------- 页头：标题 + 全局刷新 ----------
    ui.add_space(2.0);
    theme::form_row(ui, |ui| {
        ui.label(
            egui::RichText::new(crate::i18n::tr("资产总览"))
                .strong()
                .size(theme::FONT_LG),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::btn_default(ui, crate::i18n::tr("刷新账户"), true).clicked() {
                state.send_cmd(Command::RefreshAccount);
            }
        });
    });
    ui.add_space(6.0);

    // ---------- 现货资产卡片 ----------
    spot_section(&state, ui);
    ui.add_space(10.0);

    // ---------- 合约资产卡片 ----------
    fut_section(&state, ui);
}

fn spot_section(state: &Arc<crate::engine::state::SharedState>, ui: &mut egui::Ui) {
    // 短锁拷贝后绘制：acct 是引擎写热点（WS 推送/REST 刷新），
    // 不能在整个表格构建期间持有锁（与 chart.rs 的 clone-and-release 纪律一致）
    let (nonzero, can_trade, err) = {
        let acct = state.acct.lock();
        match acct.spot.as_ref() {
            None => (Vec::new(), false, acct.spot_err.clone()),
            Some(a) => (
                a.balances
                    .iter()
                    .filter(|b| b.free != Decimal::ZERO || b.locked != Decimal::ZERO)
                    .cloned()
                    .collect(),
                a.can_trade,
                None,
            ),
        }
    };
    theme::card(ui, |ui| {
        card_header(ui, crate::i18n::tr("现货资产"), |ui| {
            if !can_trade {
                theme::badge(ui, crate::i18n::tr("该 Key 无交易权限"), theme::cur().warn);
            }
            if !nonzero.is_empty() {
                ui.label(theme::card_label(&format!(
                    "{} {}",
                    nonzero.len(),
                    crate::i18n::tr("个非零资产")
                )));
            }
        });
        if let Some(e) = err {
            load_failed_row(state, ui, Some(e));
            return;
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
        // 冻结列仅在有冻结资产时显示：绝大多数账户该值常年为 0，
        // 一整列孤零零的暗色 0 只有噪音没有信息
        let has_locked = nonzero.iter().any(|b| b.locked != Decimal::ZERO);
        TableBuilder::new(ui)
            // TableBuilder 默认 id_salt 固定 "__table_state"：同一 ui 下两张表
            // 各需唯一 salt，冲突时 egui 会画出 "First use of Table ID …" 调试错误
            .id_salt("spot_assets")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().clip(true).at_least(90.0)) // 资产
            .column(Column::auto().clip(true).at_least(130.0)) // 钱包余额（可用+冻结）
            .column(Column::auto().clip(true).at_least(130.0)) // 可用
            // 数值列统一 auto+at_least：末列不用 remainder，整卡宽度下
            // 会把末列顶到最右、中间大片留白
            .column(Column::auto().clip(true).at_least(130.0)) // 冻结（条件显示）
            .header(22.0, |mut h| {
                for t in if has_locked {
                    ["资产", "钱包余额", "可用", "冻结"].as_slice()
                } else {
                    ["资产", "钱包余额", "可用"].as_slice()
                } {
                    h.col(|ui| {
                        if *t == "资产" {
                            ui.label(theme::card_label(crate::i18n::tr(t)));
                        } else {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(theme::card_label(crate::i18n::tr(t)));
                                },
                            );
                        }
                    });
                }
            })
            .body(|mut body| {
                for b in &nonzero {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            ui.label(theme::code(&b.asset).size(theme::FONT_MD));
                        });
                        row.col(|ui| {
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_grouped(&(b.free + b.locked)),
                                None,
                            );
                        });
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_dec_grouped(&b.free), None);
                        });
                        if has_locked {
                            row.col(|ui| {
                                widgets::num_cell(
                                    ui,
                                    widgets::fmt_dec_grouped(&b.locked),
                                    Some(theme::cur().text_dim),
                                );
                            });
                        }
                    });
                }
            });
    });
}

fn fut_section(state: &Arc<crate::engine::state::SharedState>, ui: &mut egui::Ui) {
    let (fut, dual, fut_err) = {
        let acct = state.acct.lock();
        (acct.fut.clone(), acct.fut_dual_side, acct.fut_err.clone())
    };
    theme::card(ui, |ui| {
        card_header(ui, crate::i18n::tr("合约资产 · U本位"), |ui| {
            if theme::btn_default(ui, crate::i18n::tr("切换"), true)
                .on_hover_text(crate::i18n::tr("切换单向/双向持仓模式"))
                .clicked()
            {
                state.send_cmd(Command::SetDualSide { dual: !dual });
            }
            ui.label(theme::card_label(if dual {
                crate::i18n::tr("持仓模式：双向")
            } else {
                crate::i18n::tr("持仓模式：单向")
            }));
        });
        let Some(a) = fut else {
            load_failed_row(state, ui, fut_err);
            return;
        };
        // 迷你统计行：USDT 计价可跨资产加总；horizontal_wrapped 窄窗折行不裁切
        ui.horizontal_wrapped(|ui| {
            let ext =
                |rt: egui::RichText| egui::Label::new(rt).wrap_mode(egui::TextWrapMode::Extend);
            let stat = |ui: &mut egui::Ui, label: &str, v: String, color: egui::Color32| {
                ui.add(ext(theme::card_label(label)));
                ui.add(ext(egui::RichText::new(v)
                    .monospace()
                    .size(theme::FONT_MD)
                    .color(color)));
                ui.add_space(10.0);
            };
            let f = |d: &Decimal| widgets::f64_decimal(d);
            stat(
                ui,
                crate::i18n::tr("钱包余额"),
                widgets::fmt_thousands(f(&a.total_wallet), 4),
                theme::cur().text,
            );
            stat(
                ui,
                crate::i18n::tr("可用余额"),
                widgets::fmt_thousands(f(&a.available), 4),
                theme::cur().text,
            );
            stat(
                ui,
                crate::i18n::tr("保证金余额"),
                widgets::fmt_thousands(f(&a.total_margin), 4),
                theme::cur().text,
            );
            stat(
                ui,
                crate::i18n::tr("未实现盈亏"),
                widgets::fmt_thousands(f(&a.total_unrealized), 4),
                theme::signed_color(f(&a.total_unrealized)),
            );
        });
        ui.add_space(4.0);
        // 短锁拷贝资产明细
        let assets: Vec<crate::binance::model::FutBalance> = {
            let acct = state.acct.lock();
            match acct.fut.as_ref() {
                Some(a) => a
                    .assets
                    .iter()
                    .filter(|b| b.wallet != Decimal::ZERO)
                    .cloned()
                    .collect(),
                None => Vec::new(),
            }
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
        TableBuilder::new(ui)
            // 同上：与现货表区分 id
            .id_salt("fut_assets")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().clip(true).at_least(90.0)) // 资产
            .column(Column::auto().clip(true).at_least(130.0)) // 钱包余额
            .column(Column::auto().clip(true).at_least(130.0)) // 可用
            // 末列不用 remainder：整卡宽度下会把末列顶到最右、中间大片留白
            .column(Column::auto().clip(true).at_least(130.0)) // 未实现盈亏
            .header(22.0, |mut h| {
                for t in ["资产", "钱包余额", "可用", "未实现盈亏"] {
                    h.col(|ui| {
                        if t == "资产" {
                            ui.label(theme::card_label(crate::i18n::tr(t)));
                        } else {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(theme::card_label(crate::i18n::tr(t)));
                                },
                            );
                        }
                    });
                }
            })
            .body(|mut body| {
                for b in &assets {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            ui.label(theme::code(&b.asset).size(theme::FONT_MD));
                        });
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_dec_grouped(&b.wallet), None);
                        });
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_dec_grouped(&b.available), None);
                        });
                        row.col(|ui| {
                            let s = widgets::fmt_dec_grouped(&b.unrealized);
                            let sign = if b.unrealized >= Decimal::ZERO {
                                "+"
                            } else {
                                ""
                            };
                            widgets::num_cell(
                                ui,
                                format!("{sign}{s}"),
                                Some(theme::signed_color(widgets::f64_decimal(&b.unrealized))),
                            );
                        });
                    });
                }
            });
    });
}
