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

/// 导出 CSV 到数据目录 exports/（带 UTF-8 BOM，Excel 直开中文不乱码），
/// 文件名 {前缀}_{时间戳}.csv；成功返回完整路径供 toast 提示
fn write_csv(
    file_stem: &str,
    header: &[&str],
    rows: &[Vec<String>],
) -> Result<std::path::PathBuf, String> {
    let dir = crate::config::data_dir().join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = dir.join(format!("{file_stem}_{ts}.csv"));
    let mut s = String::from("\u{feff}");
    s.push_str(&header.join(","));
    s.push('\n');
    for r in rows {
        let cells: Vec<String> = r
            .iter()
            .map(|c| {
                // CSV 转义：含逗号/引号/换行的字段包引号，内部引号翻倍
                if c.contains(',') || c.contains('"') || c.contains('\n') {
                    format!("\"{}\"", c.replace('"', "\"\""))
                } else {
                    c.clone()
                }
            })
            .collect();
        s.push_str(&cells.join(","));
        s.push('\n');
    }
    std::fs::write(&path, s).map_err(|e| e.to_string())?;
    Ok(path)
}

/// 导出完成 toast：成功报路径，失败报原因
fn toast_export(state: &Arc<crate::engine::state::SharedState>, r: Result<std::path::PathBuf, String>) {
    match r {
        Ok(p) => state.push_toast(
            crate::engine::state::ToastKind::Ok,
            format!("{} {}", crate::i18n::tr("已导出 "), p.display()),
        ),
        Err(e) => state.push_toast(
            crate::engine::state::ToastKind::Err,
            format!("{}{e}", crate::i18n::tr("导出失败: ")),
        ),
    }
}

/// 底部订单区表格统一横向兜底：面板窄于表格最小宽（列 at_least 合计 +
/// 末列下限）时出横向滚动条，避免末尾几列被静默裁剪。
/// egui_extras 0.36 的 TableBuilder 无内建横向滚动（内部 ScrollArea 写死
/// [false, vscroll]）；这里用 set_min_width 把表格最小宽撑给外层横向
/// 滚动区——视口更宽时内容不溢出、布局与不包裹时完全一致
fn hscroll_table(ui: &mut egui::Ui, min_w: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(min_w);
            add(ui);
        });
}

/// 当前挂单
pub fn show_open(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let market = app.market;
    // 价格小数位 = quote 资产侧精度（tick 小数位）；数量 = base 资产侧精度（step 小数位）
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.price_dp() as usize).unwrap_or(2))
    };
    let qtd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4))
    };
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
    theme::form_row(ui, |ui| {
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
                    symbol: Some(sym),
                });
            }
            // 撤销本市场全部挂单（跨交易对；两次点击确认，防误扫全局撤销）
            let mkt_all_n = list.len();
            let armed = app.market_cancel_armed;
            let cancel_mkt = theme::btn_ghost(
                ui,
                &if armed {
                    format!("{}({mkt_all_n})", crate::i18n::tr("确认撤销本市场全部"))
                } else {
                    format!("{}({mkt_all_n})", crate::i18n::tr("撤销本市场全部"))
                },
                true,
            )
            .on_hover_text(crate::i18n::tr("撤销该市场所有交易对的全部挂单"));
            if cancel_mkt.clicked() {
                if !armed {
                    app.market_cancel_armed = true;
                } else {
                    app.market_cancel_armed = false;
                    state.send_cmd(Command::CancelAll { market, symbol: None });
                }
            } else if armed && ui.input(|i| i.pointer.any_click()) {
                // 点其它位置退出待确认态
                app.market_cancel_armed = false;
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
    hscroll_table(ui, 544.0, |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            // 列宽合计须容于底部订单区默认宽（~680pt）：auto 列会随内容自动加宽，
            // at_least 只兜底表头/短值，宽裕度留给价格/数量的千分位展开
            .column(Column::auto().clip(true).at_least(68.0)) // 时间
            .column(Column::auto().clip(true).at_least(72.0)) // 交易对
            .column(Column::auto().clip(true).at_least(36.0)) // 方向
            .column(Column::auto().clip(true).at_least(50.0)) // 类型
            .column(Column::auto().clip(true).at_least(70.0)) // 价格
            .column(Column::auto().clip(true).at_least(70.0)) // 数量
            .column(Column::auto().clip(true).at_least(70.0)) // 已成交
            .column(Column::auto().clip(true).at_least(44.0)) // 状态
            .column(Column::remainder().clip(true).at_least(64.0)) // 操作
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
                    h.col(|ui| head_cell(ui, crate::i18n::tr(t), i == 1 || i == 8 || (4..=6).contains(&i)));
                }
            })
            .body(|mut body| {
                for o in &list {
                    // 行高 28：容纳 btn_danger（BTN_H）按钮
                    body.row(28.0, |mut row| {
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_time(o.time), None);
                        });
                        row.col(|ui| {
                            // 交易对列右对齐（与表头一致）
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    egui::RichText::new(&o.symbol).strong().size(theme::FONT_MD),
                                );
                            });
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
                                Some(p) => widgets::fmt_dec_fixed(&p, pxd(&o.symbol)),
                                None => match o.stop_price.filter(|s| !s.is_zero()) {
                                    Some(s) => format!(
                                        "{} {}",
                                        crate::i18n::tr("触发"),
                                        widgets::fmt_dec_fixed(&s, pxd(&o.symbol))
                                    ),
                                    None => crate::i18n::tr("市价").into(),
                                },
                            };
                            widgets::num_cell(ui, p, None);
                        });
                        row.col(|ui| {
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_fixed(&o.orig_qty, qtd(&o.symbol)),
                                None,
                            );
                        });
                        row.col(|ui| {
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_fixed(&o.executed_qty, qtd(&o.symbol)),
                                None,
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(status_cn(&o.status))
                                    .size(theme::FONT_MD)
                                    .color(theme::cur().accent),
                            );
                        });
                        row.col(|ui| {
                            // 操作列右对齐：撤销按钮贴行尾
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if theme::btn_danger(ui, crate::i18n::tr("撤销"), true).clicked() {
                                    cancel_id = Some(o.order_id);
                                    cancel_sym = o.symbol.clone();
                                }
                            });
                        });
                    });
                }
            });
    });

    if let Some(id) = cancel_id {
        app.state.send_cmd(Command::CancelOrder {
            market,
            symbol: cancel_sym,
            order_id: id,
        });
    }
}

/// 仓位止盈/止损触发价：v3 positionRisk 实测不返回 takeProfit/stopLos，
/// 从挂单中的整仓条件单推导（TP/SL 按钮提交的就是这类单）——
/// TAKE_PROFIT_MARKET → 止盈、STOP_MARKET → 止损（触发价即 stopPrice，
/// 与持仓方向无关）。positionRisk 的同名字段（若未来恢复返回）作为兜底；
/// 对冲模式下按 positionSide 匹配，单向模式（BOTH）接受全部
fn pos_tpsl(p: &PositionRisk, open: &[crate::binance::model::OrderInfo]) -> (Decimal, Decimal) {
    let mut tp = p.take_profit;
    let mut sl = p.stop_loss;
    for o in open.iter().filter(|o| o.symbol == p.symbol && o.status == "NEW") {
        let side_ok =
            o.position_side == "BOTH" || p.side == "BOTH" || o.position_side == p.side;
        if !side_ok {
            continue;
        }
        let Some(sp) = o.stop_price else { continue };
        match o.order_type.as_str() {
            "TAKE_PROFIT_MARKET" => tp = sp,
            "STOP_MARKET" => sl = sp,
            _ => continue,
        }
    }
    (tp, sl)
}

/// 合约持仓
pub fn show_positions(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    // 持仓均为 U 本位合约；价格小数位 = quote 资产侧精度（tick 小数位），
    // 数量 = base 资产侧精度（step 小数位）
    let market = crate::binance::Market::UmFutures;
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.price_dp() as usize).unwrap_or(2))
    };
    let qtd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4))
    };
    if !state.has_creds() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("未配置 API 密钥"),
            crate::i18n::tr("在设置页配置密钥后查看"),
        );
        return;
    }
    let (positions, dual, fut_open) = {
        let a = state.acct.lock();
        let fut_open = state.orders.lock().open(market).clone();
        (
            a.fut_positions
                .iter()
                .filter(|p| p.amt != Decimal::ZERO)
                .cloned()
                .collect::<Vec<PositionRisk>>(),
            a.fut_dual_side,
            fut_open,
        )
    };
    theme::form_row(ui, |ui| {
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
        // 一键平仓：全部非零持仓聚合为一个确认弹窗（真实资金纪律不破例）
        if !positions.is_empty()
            && theme::btn_danger(ui, crate::i18n::tr("全部平仓"), true)
                .on_hover_text(crate::i18n::tr("市价平仓所有持仓（聚合确认）"))
                .clicked()
        {
            crate::ui::trade::confirm_close_all(app);
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
    let mut tpsl_open: Option<(String, bool, FutPosSide)> = None;
    let mut reverse_req: Option<(String, bool, Decimal, FutPosSide)> = None;

    hscroll_table(ui, 712.0, |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            // 列宽合计 ≈ 830pt：底部订单区默认宽（~680pt）放不下，但
            // hscroll_table 已包横向滚动，窄面板下可拖出来看全列
            .column(Column::auto().clip(true).at_least(68.0)) // 交易对
            .column(Column::auto().clip(true).at_least(32.0)) // 方向
            .column(Column::auto().clip(true).at_least(56.0)) // 数量
            .column(Column::auto().clip(true).at_least(68.0)) // 开仓价
            .column(Column::auto().clip(true).at_least(80.0)) // 损益两平价
            .column(Column::auto().clip(true).at_least(64.0)) // 标记价
            .column(Column::auto().clip(true).at_least(68.0)) // 未实现盈亏
            .column(Column::auto().clip(true).at_least(62.0)) // 强平价
            .column(Column::auto().clip(true).at_least(28.0)) // 杠杆
            .column(Column::auto().clip(true).at_least(104.0)) // 仓位止盈止损
            .column(Column::remainder().clip(true).at_least(190.0)) // 操作（TP/SL + 平仓 + 反手交易）
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
                    "仓位止盈止损",
                    "操作",
                ]
                .into_iter()
                .enumerate()
                {
                    h.col(|ui| {
                        head_cell(
                            ui,
                            crate::i18n::tr(t),
                            i == 0 || i == 10 || (2..=9).contains(&i),
                        );
                        if i == 9 {
                            // 仓位止盈止损语义特殊（整仓触发、平仓即取消），
                            // 表头挂完整说明，悬停可查
                            ui.interact(
                                ui.max_rect(),
                                egui::Id::new("pos_tpsl_head"),
                                egui::Sense::hover(),
                            )
                            .on_hover_text(crate::i18n::tr(
                                "该止盈止损针对整个仓位（无论加仓或者减仓）。平仓后该止盈止损将被自动取消。当价格到达设定的触发价格时，将执行市价平仓委托。如果仓位数量超过市价委托单笔最大数量限制，委托将被拒绝。",
                            ));
                        }
                    });
                }
            })
            .body(|mut body| {
                for p in &positions {
                    // 行高 28：容纳 btn_danger（BTN_H）按钮
                    body.row(28.0, |mut row| {
                        let long = p.amt > Decimal::ZERO;
                        row.col(|ui| {
                            // 交易对列右对齐（与表头一致）
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    egui::RichText::new(&p.symbol).strong().size(theme::FONT_MD),
                                );
                            });
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
                            let c = if p.unrealized >= Decimal::ZERO {
                                theme::cur().up
                            } else {
                                theme::cur().down
                            };
                            let s = widgets::fmt_dec_fixed(&p.unrealized, pxd(&p.symbol));
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
                                widgets::fmt_dec_fixed(&p.liquidation_price, pxd(&p.symbol))
                            };
                            widgets::num_cell(ui, liq, None);
                        });
                        row.col(|ui| {
                            // symbolConfig 未返回（如拉取失败）时为 0 → 显示 —
                            let lev = crate::ui::widgets::f64_decimal(&p.leverage) as i64;
                            widgets::num_cell(
                                ui,
                                if lev > 0 { format!("{lev}x") } else { "—".into() },
                                None,
                            );
                        });
                        row.col(|ui| {
                            // 仓位止盈/止损触发价（0 = 未设置）：止盈用涨色、
                            // 止损用跌色，未设置显示 —。v3 positionRisk 实测
                            // 不返回该字段，从挂单中的整仓条件单推导（与
                            // TP/SL 按钮提交的单子同源）
                            let (tp_px, sl_px) = pos_tpsl(p, &fut_open);
                            if tp_px.is_zero() && sl_px.is_zero() {
                                widgets::num_cell(ui, "—".into(), None);
                            } else {
                                let dp = pxd(&p.symbol);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        let tp = if tp_px.is_zero() {
                                            "—".to_string()
                                        } else {
                                            widgets::fmt_dec_fixed(&tp_px, dp)
                                        };
                                        let sl = if sl_px.is_zero() {
                                            "—".to_string()
                                        } else {
                                            widgets::fmt_dec_fixed(&sl_px, dp)
                                        };
                                        // RTL 先加的贴最右：自左向右呈 止盈 / 止损，
                                        // 与表头「仓位止盈止损」语序一致
                                        ui.label(
                                            egui::RichText::new(sl)
                                                .monospace()
                                                .size(theme::FONT_SM)
                                                .color(theme::cur().down),
                                        );
                                        ui.label(
                                            egui::RichText::new("/")
                                                .monospace()
                                                .size(theme::FONT_SM)
                                                .color(theme::cur().text_dim),
                                        );
                                        ui.label(
                                            egui::RichText::new(tp)
                                                .monospace()
                                                .size(theme::FONT_SM)
                                                .color(theme::cur().up),
                                        );
                                    },
                                );
                            }
                        });
                        row.col(|ui| {
                            // 操作列右对齐：按钮贴行尾（RTL 先加的贴最右）。
                            // 视觉顺序（自左向右）TP/SL | 平仓 | 反手交易：
                            // 平仓紧邻反手交易——两者都是「了结当前持仓」的
                            // 高频操作，TP/SL 作为条件单配置单独靠左
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // 反手交易（最右，最先添加）
                                if theme::btn_default(ui, crate::i18n::tr("反手交易"), true)
                                    .on_hover_text(crate::i18n::tr(
                                        "市价平仓并反向开同等数量（有确认弹窗）",
                                    ))
                                    .clicked()
                                {
                                    reverse_req = Some((p.symbol.clone(), long, p.amt.abs(), FutPosSide::parse(&p.side)));
                                }
                                // 平仓（居中）
                                // 与资产页持仓表一致的短文案（市价语义放悬停提示，省列宽）
                                if theme::btn_danger(ui, crate::i18n::tr("平仓"), true)
                                    .on_hover_text(crate::i18n::tr("市价平仓（有确认弹窗）"))
                                    .clicked()
                                {
                                    let side = if long { Side::Sell } else { Side::Buy };
                                    let pos_side = FutPosSide::parse(&p.side);
                                    close_req =
                                        Some((p.symbol.clone(), side, p.amt.abs(), pos_side));
                                }
                                // TP/SL（最左）
                                if theme::btn_default(ui, crate::i18n::tr("TP/SL"), true)
                                    .on_hover_text(crate::i18n::tr("附加止盈/止损（触发后市价全平）"))
                                    .clicked()
                                {
                                    tpsl_open = Some((
                                        p.symbol.clone(),
                                        long,
                                        FutPosSide::parse(&p.side),
                                    ));
                                }
                            });
                        });
                    });
                }
            });
    });

    if let Some((symbol, side, qty, pos_side)) = close_req {
        crate::ui::trade::confirm_market_close(app, symbol, side, qty, pos_side);
    }
    if let Some((sym, long, pos_side)) = tpsl_open {
        app.tpsl_edit = Some((sym, long, pos_side, String::new(), String::new()));
    }
    if let Some((symbol, long, qty, pos_side)) = reverse_req {
        crate::ui::trade::confirm_market_reverse(app, symbol, long, qty, pos_side);
    }
    show_tpsl_window(app, ui);
}

/// 持仓行内 TP/SL 弹窗：两个触发价输入，提交进入确认弹窗（真实资金纪律不破例）
fn show_tpsl_window(app: &mut BnApp, ui: &mut egui::Ui) {
    let Some(edit) = app.tpsl_edit.as_mut() else {
        return;
    };
    let (symbol, long, pos_side) = (edit.0.clone(), edit.1, edit.2);
    let mut close = false;
    let mut submit: Option<(String, String)> = None;
    egui::Window::new(
        egui::RichText::new(format!(
            "{} · {} {}",
            crate::i18n::tr("止盈 / 止损"),
            symbol,
            if long {
                crate::i18n::tr("多")
            } else {
                crate::i18n::tr("空")
            }
        ))
        .strong()
        .size(theme::FONT_LG),
    )
    .anchor(egui::Align2::CENTER_TOP, [0.0, 96.0])
    .collapsible(false)
    .resizable(false)
    .min_width(268.0)
    .default_height(0.0)
    .show(ui.ctx(), |ui| {
        ui.label(
            egui::RichText::new(crate::i18n::tr("触发后市价全平该持仓，提交前需确认"))
                .size(theme::FONT_SM)
                .color(theme::cur().text_dim),
        );
        ui.add_space(4.0);
        egui::Grid::new("tpsl_grid")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                for (label, buf) in [
                    (crate::i18n::tr("止损触发"), &mut edit.3),
                    (crate::i18n::tr("止盈触发"), &mut edit.4),
                ] {
                    ui.label(egui::RichText::new(label).size(theme::FONT_MD));
                    ui.add(
                        egui::TextEdit::singleline(buf)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(116.0),
                    );
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::btn_solid(
                ui,
                crate::i18n::tr("提交"),
                theme::cur().accent,
                true,
            )
            .clicked()
            {
                submit = Some((edit.3.clone(), edit.4.clone()));
            }
            if theme::btn_default(ui, crate::i18n::tr("关闭"), true).clicked() {
                close = true;
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
    });
    if close {
        app.tpsl_edit = None;
    }
    if let Some((sl, tp)) = submit {
        match crate::ui::trade::confirm_position_tpsl(app, symbol, long, pos_side, sl, tp) {
            Ok(()) => app.tpsl_edit = None,
            Err(e) => {
                app.state.push_toast(crate::engine::state::ToastKind::Warn, e);
            }
        }
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
    // 价格小数位 = quote 资产侧精度（tick 小数位）；数量 = base 资产侧精度（step 小数位）。
    // 历史按 (市场, 交易对) 查询，行内交易对即查询键，市场取查询键的市场
    let market = key.as_ref().map(|(m, _)| *m).unwrap_or(app.market);
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.price_dp() as usize).unwrap_or(2))
    };
    let qtd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4))
    };
    theme::form_row(ui, |ui| {
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
        // 导出当前展示的订单历史（最多 100 笔）
        if theme::btn_default(ui, crate::i18n::tr("导出 CSV"), !hist.is_empty())
            .on_disabled_hover_text(crate::i18n::tr("暂无订单历史"))
            .clicked()
        {
            let rows = hist
                .iter()
                .map(|o| {
                    vec![
                        widgets::fmt_time(o.time),
                        o.symbol.clone(),
                        type_cn(&o.order_type).to_string(),
                        if o.side == "BUY" {
                            crate::i18n::tr("买入").to_string()
                        } else {
                            crate::i18n::tr("卖出").to_string()
                        },
                        o.price.unwrap_or_default().normalize().to_string(),
                        o.orig_qty.normalize().to_string(),
                        o.executed_qty.normalize().to_string(),
                        status_cn(&o.status).to_string(),
                    ]
                })
                .collect::<Vec<_>>();
            let stem = format!(
                "orders_{}",
                key.as_ref().map(|(_, s)| s.as_str()).unwrap_or("all")
            );
            toast_export(
                &state,
                write_csv(
                    &stem,
                    &["time", "symbol", "type", "side", "price", "qty", "executed", "status"],
                    &rows,
                ),
            );
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
    hscroll_table(ui, 494.0, |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().clip(true).at_least(78.0))
            .column(Column::auto().clip(true).at_least(72.0))
            .column(Column::auto().clip(true).at_least(36.0))
            .column(Column::auto().clip(true).at_least(50.0))
            .column(Column::auto().clip(true).at_least(70.0))
            .column(Column::auto().clip(true).at_least(70.0))
            .column(Column::auto().clip(true).at_least(70.0))
            .column(Column::remainder().clip(true).at_least(48.0))
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
                            widgets::num_cell(ui, widgets::fmt_time(o.time), None);
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
                                Some(p) => widgets::fmt_dec_fixed(&p, pxd(&o.symbol)),
                                None => match o.stop_price.filter(|s| !s.is_zero()) {
                                    Some(s) => format!(
                                        "{} {}",
                                        crate::i18n::tr("触发"),
                                        widgets::fmt_dec_fixed(&s, pxd(&o.symbol))
                                    ),
                                    None => crate::i18n::tr("市价").into(),
                                },
                            };
                            widgets::num_cell(ui, p, None);
                        });
                        row.col(|ui| {
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_fixed(&o.orig_qty, qtd(&o.symbol)),
                                None,
                            );
                        });
                        row.col(|ui| {
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_fixed(&o.executed_qty, qtd(&o.symbol)),
                                None,
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
                                    .size(theme::FONT_MD),
                            );
                        });
                    });
                }
            });
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
    // 价格小数位 = quote 资产侧精度（tick 小数位）；数量 = base 资产侧精度（step 小数位）。
    // 成交按 (市场, 交易对) 查询，行内交易对即查询键，市场取查询键的市场
    let market = hist_key.as_ref().map(|(m, _)| *m).unwrap_or(app.market);
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.price_dp() as usize).unwrap_or(2))
    };
    let qtd = |sym: &str| {
        state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4))
    };
    theme::form_row(ui, |ui| {
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
        // 导出当前展示的成交明细
        if theme::btn_default(ui, crate::i18n::tr("导出 CSV"), !trades.is_empty())
            .on_disabled_hover_text(crate::i18n::tr("暂无成交"))
            .clicked()
        {
            let rows = trades
                .iter()
                .map(|t| {
                    vec![
                        widgets::fmt_time(t.time),
                        t.symbol.clone(),
                        if t.is_buyer {
                            crate::i18n::tr("买入").to_string()
                        } else {
                            crate::i18n::tr("卖出").to_string()
                        },
                        t.price.normalize().to_string(),
                        t.qty.normalize().to_string(),
                        t.quote_qty.normalize().to_string(),
                        t.commission.normalize().to_string(),
                        t.commission_asset.clone(),
                        if t.is_maker { "maker" } else { "taker" }.to_string(),
                    ]
                })
                .collect::<Vec<_>>();
            let stem = format!(
                "trades_{}",
                hist_key
                    .as_ref()
                    .map(|(_, s)| s.as_str())
                    .unwrap_or("all")
            );
            toast_export(
                &state,
                write_csv(
                    &stem,
                    &[
                        "time", "symbol", "side", "price", "qty", "quote_qty", "fee", "fee_asset",
                        "role",
                    ],
                    &rows,
                ),
            );
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
    hscroll_table(ui, 514.0, |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().clip(true).at_least(78.0))
            .column(Column::auto().clip(true).at_least(72.0))
            .column(Column::auto().clip(true).at_least(36.0))
            .column(Column::auto().clip(true).at_least(70.0))
            .column(Column::auto().clip(true).at_least(70.0))
            .column(Column::auto().clip(true).at_least(80.0))
            .column(Column::auto().clip(true).at_least(60.0))
            .column(Column::remainder().clip(true).at_least(48.0))
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
                            widgets::num_cell(ui, widgets::fmt_time(t.time), None);
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
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_fixed(&t.price, pxd(&t.symbol)),
                                None,
                            );
                        });
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_dec_fixed(&t.qty, qtd(&t.symbol)), None);
                        });
                        row.col(|ui| {
                            let q = if t.quote_qty.is_zero() {
                                t.price * t.qty
                            } else {
                                t.quote_qty
                            };
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_fixed(&q, pxd(&t.symbol)),
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
    });
}
