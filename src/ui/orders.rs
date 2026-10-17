//! 订单与持仓：当前挂单（撤销）、合约持仓（平仓/杠杆）、订单历史、成交历史

use crate::app::BnApp;
use crate::binance::model::{OrderInfo, PositionRisk};
use crate::binance::{FutPosSide, Market, Side};
use crate::engine::Command;
use crate::ui::{theme, widgets};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use rust_decimal::Decimal;
use std::sync::Arc;

/// 订单历史筛选：基础单来自 allOrders，条件单来自 allAlgoOrders
/// （两类合并同表展示，默认全选）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistKind {
    All,
    Basic,
    Conditional,
}

impl HistKind {
    fn label(&self) -> &'static str {
        match self {
            HistKind::All => crate::i18n::tr("全部"),
            HistKind::Basic => crate::i18n::tr("基础单"),
            HistKind::Conditional => crate::i18n::tr("条件单"),
        }
    }
}

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
        // 条件单（Algo）状态：触发后转出的普通单有自己的状态流，
        // 这里记录条件单自身的生命周期
        "TRIGGERED" => "已触发",
        "CANCELLED" => "已撤销",
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
fn toast_export(
    state: &Arc<crate::engine::state::SharedState>,
    r: Result<std::path::PathBuf, String>,
) {
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
/// 滚动区——视口更宽时内容不溢出、布局与不包裹时完全一致。
/// salt 同时区隔横向 ScrollArea 与 TableBuilder 的持久化 Id：四张子表
/// 互斥渲染于同一父 ui，默认 salt 会让横向滚动偏移与列宽跨表泄漏
/// （订单历史与成交历史同为 8 列，会互相继承并棘轮式撑宽列宽）
fn hscroll_table(ui: &mut egui::Ui, salt: &str, min_w: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.push_id(salt, |ui| {
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(min_w);
                add(ui);
            });
    });
}

/// 当前挂单
pub fn show_open(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let market = app.market;
    // 价格小数位 = quote 资产侧精度（tick 小数位）；数量 = base 资产侧精度（step 小数位）
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| {
            r.map(|x| x.price_dp() as usize).unwrap_or(2)
        })
    };
    let qtd =
        |sym: &str| state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4));
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
    // 条件单（Algo 体系）只属于合约：现货页不合并，避免串市场显示
    let algo = if market == crate::binance::Market::UmFutures {
        state.orders.lock().algo.clone()
    } else {
        Vec::new()
    };
    // ---- 武装态时效性 ----
    // 武装的交易对已无挂单（成交/撤销经用户数据流实时生效）时必须解除：
    // 否则下次新挂单到来时按钮直接停在"确认撤销"标签上，一点即发，
    // 两步确认被静默旁路。同理，行级武装对应的订单已不存在时解除
    let s = app.page.symbol.clone();
    let sym_open = list.iter().filter(|o| o.symbol == s).count()
        + algo.iter().filter(|o| o.symbol == s).count();
    if app.sym_cancel_armed.as_deref() == Some(s.as_str()) && sym_open == 0 {
        app.sym_cancel_armed = None;
    }
    if let Some(id) = app.row_cancel_armed {
        if !list.iter().any(|o| o.order_id == id) {
            app.row_cancel_armed = None;
        }
    }
    // Esc 退出所有撤销待确认态：键盘用户无需点其它位置解除
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.sym_cancel_armed = None;
        app.row_cancel_armed = None;
        app.market_cancel_armed = false;
    }
    theme::form_row(ui, |ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} · {}{}",
                market.label(),
                list.len() + algo.len(),
                crate::i18n::tr("笔挂单")
            ))
            .size(theme::FONT_MD),
        );
        if theme::btn_default(ui, crate::i18n::tr("刷新"), true).clicked() {
            state.send_cmd(Command::RefreshOpenOrders { market });
        }
        if !list.is_empty() || !algo.is_empty() {
            let sym = app.page.symbol.clone();
            let sym_open = list.iter().filter(|o| o.symbol == sym).count()
                + algo.iter().filter(|o| o.symbol == sym).count();
            // 当前交易对没有挂单时禁用，避免"点了没反应"的静默空操作
            // 两步确认（与本市场全撤同一纪律）：第一击进入待确认态，
            // 第二击才真正发命令；撤销的是真实挂单，误点代价不可逆
            let is_armed = app.sym_cancel_armed.as_deref() == Some(sym.as_str());
            let cancel_all = theme::btn_danger(
                ui,
                &if is_armed {
                    format!("{} {}({sym_open})", crate::i18n::tr("确认撤销"), sym)
                } else {
                    format!("{} {}({sym_open})", crate::i18n::tr("撤销全部"), sym)
                },
                sym_open > 0,
            )
            .on_disabled_hover_text(crate::i18n::tr("该交易对无挂单"));
            if cancel_all.clicked() {
                if is_armed {
                    app.sym_cancel_armed = None;
                    state.send_cmd(Command::CancelAll {
                        market,
                        symbol: Some(sym),
                    });
                } else {
                    app.sym_cancel_armed = Some(sym);
                    // 与行级武装互斥：同一时刻只有一个待确认态
                    app.row_cancel_armed = None;
                }
            } else if app.sym_cancel_armed.is_some() && ui.input(|i| i.pointer.any_click()) {
                // 点其它位置退出待确认态
                app.sym_cancel_armed = None;
            }
            // 撤销本市场全部挂单（跨交易对；两次点击确认，防误扫全局撤销）。
            // 样式与左侧「撤销全部」同源（btn_danger）：同为破坏性操作，
            // 字号（FONT_BODY）/高度（BTN_H）/红调完全一致，不再用小号 ghost
            let mkt_all_n = list.len() + algo.len();
            let armed = app.market_cancel_armed;
            let cancel_mkt = theme::btn_danger(
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
                    state.send_cmd(Command::CancelAll {
                        market,
                        symbol: None,
                    });
                }
            } else if armed && ui.input(|i| i.pointer.any_click()) {
                // 点其它位置退出待确认态
                app.market_cancel_armed = false;
            }
        }
    });
    ui.separator();

    if list.is_empty() && algo.is_empty() {
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
    // 条件单行（Algo 体系）的撤销：主键是 algoId，与普通单 orderId 走
    // 不同的撤单命令
    let mut cancel_algo_id: Option<i64> = None;
    // 本帧是否点中了某行的撤销按钮：行级武装态的解除以它为准
    // （any_click 是全局输入，按钮自身的点击必须在解除逻辑之前排除）
    let mut row_cancel_clicked = false;
    hscroll_table(ui, "orders_open", 544.0, |ui| {
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
                    h.col(|ui| {
                        head_cell(
                            ui,
                            crate::i18n::tr(t),
                            i == 1 || i == 8 || (4..=6).contains(&i),
                        )
                    });
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
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(theme::code(&o.symbol).size(theme::FONT_MD));
                                },
                            );
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
                            // 两步确认：第一击武装（标签变为"确认撤销"），
                            // 第二击才发撤单命令；armed 键 = 订单号
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let row_armed = app.row_cancel_armed == Some(o.order_id);
                                    if theme::btn_danger(
                                        ui,
                                        if row_armed {
                                            crate::i18n::tr("确认撤销")
                                        } else {
                                            crate::i18n::tr("撤销")
                                        },
                                        true,
                                    )
                                    .clicked()
                                    {
                                        row_cancel_clicked = true;
                                        if row_armed {
                                            app.row_cancel_armed = None;
                                            cancel_id = Some(o.order_id);
                                            cancel_sym = o.symbol.clone();
                                        } else {
                                            app.row_cancel_armed = Some(o.order_id);
                                        }
                                    }
                                },
                            );
                        });
                    });
                }
                // 条件单（Algo 体系：仓位止盈止损/追踪委托等）：与普通挂单
                // 同构展示，触发价即生效价；closePosition 整仓单 quantity
                // 恒 0，以「整仓」标注
                for a in &algo {
                    body.row(28.0, |mut row| {
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_time(a.create_time), None);
                        });
                        row.col(|ui| {
                            // 交易对列右对齐（与表头一致）
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(theme::code(&a.symbol).size(theme::FONT_MD));
                                },
                            );
                        });
                        row.col(|ui| {
                            let c = if a.side == "BUY" {
                                theme::cur().up
                            } else {
                                theme::cur().down
                            };
                            let txt = if a.side == "BUY" {
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
                            let extra = if a.reduce_only {
                                format!(" {}", crate::i18n::tr("只减"))
                            } else {
                                String::new()
                            };
                            ui.label(
                                egui::RichText::new(format!("{}{extra}", type_cn(&a.order_type)))
                                    .size(theme::FONT_MD),
                            );
                        });
                        row.col(|ui| {
                            // 条件单以触发价为生效价（整仓/市价条件单无委托价）
                            let p = match a.trigger_price.filter(|p| !p.is_zero()) {
                                Some(s) => format!(
                                    "{} {}",
                                    crate::i18n::tr("触发"),
                                    widgets::fmt_dec_fixed(&s, pxd(&a.symbol))
                                ),
                                None => crate::i18n::tr("市价").into(),
                            };
                            widgets::num_cell(ui, p, None);
                        });
                        row.col(|ui| {
                            if a.close_position {
                                ui.label(
                                    egui::RichText::new(crate::i18n::tr("整仓"))
                                        .size(theme::FONT_MD),
                                );
                            } else {
                                widgets::num_cell(
                                    ui,
                                    widgets::fmt_dec_fixed(&a.quantity, qtd(&a.symbol)),
                                    None,
                                );
                            }
                        });
                        row.col(|ui| {
                            // 触发前已成交恒 0：触发后整单转出为实际订单
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_fixed(&a.actual_qty, qtd(&a.symbol)),
                                None,
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(status_cn(&a.algo_status))
                                    .size(theme::FONT_MD)
                                    .color(theme::cur().accent),
                            );
                        });
                        row.col(|ui| {
                            // 操作列右对齐：两步确认，armed 键 = algoId
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let row_armed = app.row_cancel_armed == Some(a.algo_id);
                                    if theme::btn_danger(
                                        ui,
                                        if row_armed {
                                            crate::i18n::tr("确认撤销")
                                        } else {
                                            crate::i18n::tr("撤销")
                                        },
                                        true,
                                    )
                                    .clicked()
                                    {
                                        row_cancel_clicked = true;
                                        if row_armed {
                                            app.row_cancel_armed = None;
                                            cancel_algo_id = Some(a.algo_id);
                                        } else {
                                            app.row_cancel_armed = Some(a.algo_id);
                                        }
                                    }
                                },
                            );
                        });
                    });
                }
            });
    });

    // 行级撤销武装解除：点击其它任意位置（未命中任意一行的撤销按钮）即退出
    if !row_cancel_clicked && app.row_cancel_armed.is_some() && ui.input(|i| i.pointer.any_click())
    {
        app.row_cancel_armed = None;
    }
    if let Some(id) = cancel_id {
        app.state.send_cmd(Command::CancelOrder {
            market,
            symbol: cancel_sym,
            order_id: id,
        });
    }
    if let Some(algo_id) = cancel_algo_id {
        app.state.send_cmd(Command::CancelAlgoOrder { algo_id });
    }
}

/// 仓位止盈/止损触发价：生产 v3 positionRisk 不返回 takeProfit/stopLos，
/// 从活动条件单推导——条件单在 Algo 体系（algo 列表，主流来源），旧端点
/// 时代的 openOrders 整仓条件单兜底；TAKE_PROFIT(_MARKET) → 止盈、
/// STOP(_MARKET) → 止损（触发价即成交触发价，与持仓方向无关）。
/// positionRisk 的同名字段（若未来恢复返回）作为兜底；对冲模式下按
/// positionSide 匹配，单向模式（BOTH）接受全部
fn pos_tpsl(
    p: &PositionRisk,
    open: &[crate::binance::model::OrderInfo],
    algo: &[crate::binance::model::AlgoOrderInfo],
) -> (Decimal, Decimal) {
    let mut tp = p.take_profit;
    let mut sl = p.stop_loss;
    for o in open
        .iter()
        .filter(|o| o.symbol == p.symbol && o.status == "NEW")
    {
        let side_ok = o.position_side == "BOTH" || p.side == "BOTH" || o.position_side == p.side;
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
    // algo 体系为准：覆盖 openOrders 兜底值（两来源不会同时含同一单）
    for o in algo.iter().filter(|o| o.symbol == p.symbol) {
        let side_ok = o.position_side == "BOTH" || p.side == "BOTH" || o.position_side == p.side;
        if !side_ok {
            continue;
        }
        let Some(sp) = o.trigger_price else { continue };
        match o.order_type.as_str() {
            "TAKE_PROFIT_MARKET" | "TAKE_PROFIT" => tp = sp,
            "STOP_MARKET" | "STOP" => sl = sp,
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
        state.with_rule(market, sym, |r| {
            r.map(|x| x.price_dp() as usize).unwrap_or(2)
        })
    };
    let qtd =
        |sym: &str| state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4));
    if !state.has_creds() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("未配置 API 密钥"),
            crate::i18n::tr("在设置页配置密钥后查看"),
        );
        return;
    }
    let (positions, dual, fut_open, fut_algo) = {
        let a = state.acct.lock();
        let ord = state.orders.lock();
        let fut_open = ord.open(market).clone();
        let fut_algo = ord.algo.clone();
        (
            a.fut_positions
                .iter()
                .filter(|p| p.amt != Decimal::ZERO)
                .cloned()
                .collect::<Vec<PositionRisk>>(),
            a.fut_dual_side,
            fut_open,
            fut_algo,
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

    hscroll_table(ui, "orders_positions", 712.0, |ui| {
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
            .column(Column::remainder().clip(true).at_least(240.0)) // 操作（止盈止损 + 平仓 + 反手交易）
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
                                    theme::code(&p.symbol).size(theme::FONT_MD),
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
                            let (tp_px, sl_px) = pos_tpsl(p, &fut_open, &fut_algo);
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
                                // 止盈止损（最左）：定宽按钮——中文四字与英文
                                // TP/SL 长度不一，定宽保证两种语言都不截断且
                                // 点击区足够大
                                if theme::btn_default_w(
                                    ui,
                                    crate::i18n::tr("止盈止损"),
                                    72.0,
                                    true,
                                )
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
    // 标题由 i18n 文案 + 交易对 + 方向拼成，Window 的 Id 默认派生自标题：
    // 换仓位开窗或中/EN 切换都会改标题，窗口位置/焦点状态随之丢失。
    // 固定 Id 后标题可任意变化（与预警窗 alerts_win 同一范式）
    .id(egui::Id::new("tpsl_win"))
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
            if theme::btn_solid(ui, crate::i18n::tr("提交"), theme::cur().accent, true).clicked()
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
                app.state
                    .push_toast(crate::engine::state::ToastKind::Warn, e);
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
    // 单锁取数：history 是 Arc，clone 是原子引用计数而非深拷贝；
    // 引擎整体替换 Arc，读方持有的旧帧不受写方影响
    let (hist, key) = {
        let o = state.orders.lock();
        (o.history.clone(), o.history_symbol.clone())
    };
    // 价格小数位 = quote 资产侧精度（tick 小数位）；数量 = base 资产侧精度（step 小数位）。
    // 历史按 (市场, 交易对) 查询，行内交易对即查询键，市场取查询键的市场
    let market = key.as_ref().map(|(m, _)| *m).unwrap_or(app.market);
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| {
            r.map(|x| x.price_dp() as usize).unwrap_or(2)
        })
    };
    let qtd =
        |sym: &str| state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4));
    // 筛选视图：is_conditional 由引擎在合并 allAlgoOrders 条件单历史时
    // 置位（基础单来源 allOrders 恒为 false）
    let view: Vec<&OrderInfo> = match app.hist_kind {
        HistKind::All => hist.iter().collect(),
        HistKind::Basic => hist.iter().filter(|o| !o.is_conditional).collect(),
        HistKind::Conditional => hist.iter().filter(|o| o.is_conditional).collect(),
    };
    theme::form_row(ui, |ui| {
        match &key {
            Some((m, s)) => ui.label(
                egui::RichText::new(format!(
                    "{} {} · {}",
                    m.label(),
                    s,
                    // 合约历史按 30 天窗口分段拉取（默认窗口太窄），
                    // 现货仍是单次 limit=100
                    if *m == Market::UmFutures {
                        crate::i18n::tr("最近 30 天")
                    } else {
                        crate::i18n::tr("最近 100 笔")
                    }
                ))
                .size(theme::FONT_MD),
            ),
            None => ui.label(
                egui::RichText::new(crate::i18n::tr("切换交易对后自动加载"))
                    .size(theme::FONT_MD)
                    .color(theme::cur().text_dim),
            ),
        };
        // 基础单/条件单分段筛选：pill_tab 与两侧按钮同为 BTN_H，
        // form_row 垂直居中放置，整行同高对齐
        for k in [HistKind::All, HistKind::Basic, HistKind::Conditional] {
            if theme::pill_tab(ui, k.label(), app.hist_kind == k).clicked() {
                app.hist_kind = k;
            }
        }
        // 不再要求 key 已存在：key 为空（首次进入/上次拉取失败）时用
        // 当前页市场+交易对兜底，避免"按钮灰死"导致永远无法手动重试
        if theme::btn_default(ui, crate::i18n::tr("刷新"), true).clicked() {
            let (m, s) = key.clone().unwrap_or((app.market, app.page.symbol.clone()));
            state.send_cmd(Command::RefreshHistory {
                market: m,
                symbol: s,
            });
        }
        // 导出当前筛选下展示的订单历史
        if theme::btn_default(ui, crate::i18n::tr("导出 CSV"), !view.is_empty())
            .on_disabled_hover_text(if hist.is_empty() {
                crate::i18n::tr("暂无订单历史")
            } else {
                crate::i18n::tr("当前筛选无订单")
            })
            .clicked()
        {
            let rows = view
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
                    &[
                        "time", "symbol", "type", "side", "price", "qty", "executed", "status",
                    ],
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
    if view.is_empty() {
        // 有历史但被筛选全部过滤（如只看基础单而该交易对只有条件单）
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("当前筛选无订单"),
            crate::i18n::tr("切换筛选查看其它订单"),
        );
        return;
    }
    hscroll_table(ui, "orders_history", 494.0, |ui| {
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
                for o in view.iter() {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_time(o.time), None);
                        });
                        row.col(|ui| {
                            ui.label(theme::code(&o.symbol).size(theme::FONT_MD));
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
                            ui.label(
                                egui::RichText::new(type_cn(&o.order_type)).size(theme::FONT_MD),
                            );
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
    // 单锁取数（与 show_history 同构）：trades 为 Arc，clone 不深拷贝
    let (trades, hist_key) = {
        let o = state.orders.lock();
        (o.trades.clone(), o.history_symbol.clone())
    };
    // 价格小数位 = quote 资产侧精度（tick 小数位）；数量 = base 资产侧精度（step 小数位）。
    // 成交按 (市场, 交易对) 查询，行内交易对即查询键，市场取查询键的市场
    let market = hist_key.as_ref().map(|(m, _)| *m).unwrap_or(app.market);
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| {
            r.map(|x| x.price_dp() as usize).unwrap_or(2)
        })
    };
    let qtd =
        |sym: &str| state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4));
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
        // 同 show_history：key 为空时用当前页市场+交易对兜底，保证可手动重试
        if theme::btn_default(ui, crate::i18n::tr("刷新"), true).clicked() {
            let (m, s) = hist_key
                .clone()
                .unwrap_or((app.market, app.page.symbol.clone()));
            state.send_cmd(Command::RefreshHistory {
                market: m,
                symbol: s,
            });
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
                hist_key.as_ref().map(|(_, s)| s.as_str()).unwrap_or("all")
            );
            toast_export(
                &state,
                write_csv(
                    &stem,
                    &[
                        "time",
                        "symbol",
                        "side",
                        "price",
                        "qty",
                        "quote_qty",
                        "fee",
                        "fee_asset",
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
    hscroll_table(ui, "orders_trades", 514.0, |ui| {
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
                for t in trades.iter() {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_time(t.time), None);
                        });
                        row.col(|ui| {
                            ui.label(theme::code(&t.symbol).size(theme::FONT_MD));
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
                            widgets::num_cell(
                                ui,
                                widgets::fmt_dec_fixed(&t.qty, qtd(&t.symbol)),
                                None,
                            );
                        });
                        row.col(|ui| {
                            let q = if t.quote_qty.is_zero() {
                                t.price * t.qty
                            } else {
                                t.quote_qty
                            };
                            widgets::num_cell(ui, widgets::fmt_dec_fixed(&q, pxd(&t.symbol)), None);
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

/// 资金流水：合约 income（划转/资金费/已实现盈亏/手续费），
/// 现货充值+提现合并；按市场查询，数据归属见 flow_market
pub fn show_flow(app: &mut BnApp, ui: &mut egui::Ui) {
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
    // 单锁取数（与 show_history 同构）：flow 为 Arc，clone 不深拷贝
    let (flows, flow_market) = {
        let o = state.orders.lock();
        (o.flow.clone(), o.flow_market)
    };
    let market = app.market;
    theme::form_row(ui, |ui| {
        match flow_market {
            Some(m) => ui.label(
                egui::RichText::new(format!(
                    "{} · {}",
                    m.label(),
                    crate::i18n::tr("最近 100 条")
                ))
                .size(theme::FONT_MD),
            ),
            None => ui.label(
                egui::RichText::new(crate::i18n::tr("进入页签后自动加载"))
                    .size(theme::FONT_MD)
                    .color(theme::cur().text_dim),
            ),
        };
        if theme::btn_default(ui, crate::i18n::tr("刷新"), true).clicked() {
            state.send_cmd(Command::RefreshFlow { market });
        }
        // 导出当前展示的流水
        if theme::btn_default(ui, crate::i18n::tr("导出 CSV"), !flows.is_empty())
            .on_disabled_hover_text(crate::i18n::tr("暂无资金流水"))
            .clicked()
        {
            let rows = flows
                .iter()
                .map(|f| {
                    vec![
                        widgets::fmt_time(f.time),
                        crate::i18n::tr(f.kind).to_string(),
                        f.symbol.clone(),
                        f.asset.clone(),
                        f.amount.to_string(),
                        f.note.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            let stem = format!("flows_{}", market.label());
            toast_export(
                &state,
                write_csv(
                    &stem,
                    &["time", "type", "symbol", "asset", "amount", "note"],
                    &rows,
                ),
            );
        }
    });
    ui.separator();
    if flows.is_empty() {
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("暂无资金流水"),
            crate::i18n::tr("划转、资金费、充值与提现将显示在这里"),
        );
        return;
    }
    hscroll_table(ui, "orders_flow", 434.0, |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().clip(true).at_least(78.0))
            .column(Column::auto().clip(true).at_least(64.0))
            .column(Column::auto().clip(true).at_least(72.0))
            .column(Column::auto().clip(true).at_least(52.0))
            .column(Column::auto().clip(true).at_least(96.0))
            .column(Column::remainder().clip(true).at_least(72.0))
            .header(22.0, |mut h| {
                for (i, t) in ["时间", "类型", "交易对", "资产", "金额", "备注"]
                    .into_iter()
                    .enumerate()
                {
                    h.col(|ui| head_cell(ui, crate::i18n::tr(t), i == 4));
                }
            })
            .body(|mut body| {
                for f in flows.iter() {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            widgets::num_cell(ui, widgets::fmt_time(f.time), None);
                        });
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(f.kind)).size(theme::FONT_MD),
                            );
                        });
                        row.col(|ui| {
                            // 划转/充值/提现等全局条目 symbol 为空
                            if f.symbol.is_empty() {
                                ui.label(
                                    egui::RichText::new("—")
                                        .size(theme::FONT_MD)
                                        .color(theme::cur().text_dim),
                                );
                            } else {
                                ui.label(theme::code(&f.symbol).size(theme::FONT_MD));
                            }
                        });
                        row.col(|ui| {
                            ui.label(theme::code(&f.asset).size(theme::FONT_MD));
                        });
                        row.col(|ui| {
                            // 流入正色/流出负色，与买卖方向配色一致
                            let c = if f.amount.is_zero() {
                                theme::cur().text
                            } else if f.amount > rust_decimal::Decimal::ZERO {
                                theme::cur().up
                            } else {
                                theme::cur().down
                            };
                            let sign = if f.amount > rust_decimal::Decimal::ZERO {
                                "+"
                            } else {
                                ""
                            };
                            widgets::num_cell(
                                ui,
                                format!(
                                    "{sign}{} {}",
                                    widgets::fmt_dec_grouped(&f.amount),
                                    f.asset
                                ),
                                Some(c),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(&f.note)
                                    .size(theme::FONT_MD)
                                    .color(theme::cur().text_dim),
                            );
                        });
                    });
                }
            });
    });
}
