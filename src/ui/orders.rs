//! 订单与持仓：当前挂单（撤销）、合约持仓（平仓/杠杆）、订单历史、成交历史、资金流水
//!
//! 里程碑 1：状态映射、CSV 导出、仓位 TP/SL 推导等纯函数原样迁移。
//! 里程碑 8：五个子页表格（GPUI）+ 持仓行内 TP/SL 弹窗 + 两步撤销。
//! 表格 = 定宽列 flex 行（横溢出滚动兜底 + 表体纵向滚动），撤销按钮
//! 两步确认（第一次武装、第二次发送）与 egui 版同一纪律。

use crate::app::{BnApp, SubTab};
use crate::binance::model::{OrderInfo, PositionRisk};
use crate::binance::{FutPosSide, Market, Side};
use crate::engine::Command;
use crate::ui::theme::{self, cur, faded, font_md, font_sm, FONT_MONO};
use crate::ui::{textfield::TextField, widgets};
use gpui::{
    div, prelude::FluentBuilder, px, Context, Div, ElementId, Entity, FontWeight,
    IntoElement, InteractiveElement, MouseButton, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled,
};
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
    pub fn label(&self) -> &'static str {
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

pub fn type_cn(s: &str) -> &'static str {
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

/// 订单状态展示色（up=已成交 / dim=已终结 / accent=活跃），渲染层引用
pub fn status_color(s: &str) -> gpui::Rgba {
    let t = cur();
    match s {
        "FILLED" => t.up,
        "CANCELED" | "CANCELLED" | "EXPIRED" | "REJECTED" => t.text_dim,
        _ => t.accent,
    }
}

pub fn write_csv(
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
pub fn toast_export(
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

/// 由挂单 + 条件单推导仓位当前生效的止盈/止损触发价
/// （openOrders 兜底、algo 体系覆盖），渲染层"仓位止盈止损"列引用
pub fn pos_tpsl(
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

// ============================================================
// 里程碑 8：表格 UI（GPUI）
// ============================================================

/// 列定义：w > 0 定宽（与 egui 版 at_least 对应），w = 0 弹性占满余量
/// （remainder 列）；right = 数值列右对齐
#[derive(Clone, Copy)]
struct Col {
    w: f32,
    right: bool,
}

fn cw(w: f32, right: bool) -> Col {
    Col { w, right }
}

/// 数据单元格：定宽/弹性 + 数值列右对齐，溢出按边界裁剪（遮挡优于顶歪）
fn td(c: &Col, child: impl IntoElement) -> Div {
    let mut d = div()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .px(px(theme::SPACE_MD));
    if c.w > 0.0 {
        d = d.w(px(c.w));
    } else {
        d = d.flex_1().min_w(px(48.0));
    }
    if c.right {
        d = d.justify_end();
    }
    d.child(child)
}

/// 普通文本单元格内容
fn txt(txt: impl Into<SharedString>) -> Div {
    div()
        .text_size(font_md())
        .whitespace_nowrap()
        .overflow_hidden()
        .child(txt.into())
}

/// 交易对/资产列：等宽字体
fn mono(txt: impl Into<SharedString>) -> Div {
    div()
        .font_family(FONT_MONO)
        .text_size(font_md())
        .whitespace_nowrap()
        .overflow_hidden()
        .child(txt.into())
}

/// 表头单元格：数值列右对齐与数据列一致；tip 非空时挂悬停说明
/// （「仓位止盈止损」等语义特殊的列头）
fn head_cell(
    id: impl Into<ElementId>,
    txt: &'static str,
    c: &Col,
    tip: Option<&'static str>,
) -> Stateful<Div> {
    let tr = crate::i18n::tr;
    let mut d = div()
        .id(id)
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .px(px(theme::SPACE_MD));
    if c.w > 0.0 {
        d = d.w(px(c.w));
    } else {
        d = d.flex_1().min_w(px(48.0));
    }
    if c.right {
        d = d.justify_end();
    }
    let d = d.child(widgets::card_label(tr(txt)));
    match tip {
        Some(s) => {
            let s = tr(s).to_string();
            d.tooltip(move |_, cx| widgets::tooltip_view(s.clone(), cx))
        }
        None => d,
    }
}

/// 表头行：h22 + 底部分隔线；tips 与 cols 等长，空串 = 无悬停说明
fn header_row(cols: &[Col], labels: &[&'static str], tips: &[&'static str]) -> Div {
    let t = cur();
    let mut h = div()
        .h(px(theme::TH_H))
        .flex()
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(faded(t.border, 0.6));
    for (i, (c, l)) in cols.iter().zip(labels).enumerate() {
        h = h.child(head_cell(("head", i), l, c, Some(tips[i]).filter(|s| !s.is_empty())));
    }
    h
}

/// 表体行容器：斑马纹（奇数行微亮底），行高由调用方给定
fn tr_row(idx: usize, h: f32, cells: Vec<Div>) -> Div {
    let t = cur();
    div()
        .h(px(h))
        .flex()
        .flex_row()
        .items_center()
        .when(idx % 2 == 1, |d| d.bg(faded(t.text, 0.035)))
        // 指针所在行整行点亮：扫读宽表时的定位反馈（斑马纹静态、悬停动态）
        .hover(|s| s.bg(t.row_hover))
        .children(cells)
}

/// 表格框架：横向滚动兜底（面板窄于最小列宽合计时可拖出看全列），
/// 表头固定、表体纵向滚动。min_w 与 egui 版 hscroll_table 一致
fn table(id: &'static str, min_w: f32, header: Div, rows: Vec<Div>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.0))
        .overflow_x_scroll()
        .child(
            div()
                .min_w(px(min_w))
                .h_full()
                .flex()
                .flex_col()
                .child(header)
                .child(
                    div()
                        .id("tbl_body")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(rows),
                ),
        )
}

/// 子页内容根：工具条 + 分隔线 + 表格/空态
fn page_root(bar: Div, body: impl IntoElement) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .child(bar)
        .child(widgets::separator())
        .child(body)
}

/// 居中空态（empty_state 自带纵向留白，外层包一层居中容器）
fn centered_empty(icon: &str, title: &str, subtitle: &str) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .items_center()
        .justify_center()
        .child(widgets::empty_state(icon, title, subtitle))
}

/// 子页分发：按当前子页签渲染对应表格
pub fn show(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    match app.page.sub_tab {
        SubTab::Open => show_open(app, cx),
        SubTab::Positions => show_positions(app, cx),
        SubTab::History => show_history(app, cx),
        SubTab::Trades => show_trades_history(app, cx),
        SubTab::Flow => show_flow(app, cx),
    }
}

/// 当前挂单
fn show_open(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let tr = crate::i18n::tr;
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
        return centered_empty("◎", tr("未配置 API 密钥"), tr("在设置页配置密钥后查看"));
    }
    let list = state.orders.lock().open(market).clone();
    // 条件单（Algo 体系）只属于合约：现货页不合并，避免串市场显示
    let algo = if market == Market::UmFutures {
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
        if !list.iter().any(|o| o.order_id == id) && !algo.iter().any(|a| a.algo_id == id) {
            app.row_cancel_armed = None;
        }
    }

    // ---- 工具条 ----
    let mut bar = widgets::form_row().gap(px(theme::SPACE_LG));
    bar = bar.child(
        div()
            .text_size(font_md())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(format!(
                "{} · {}{}",
                market.label(),
                list.len() + algo.len(),
                tr("笔挂单")
            )),
    );
    bar = bar.child(
        widgets::btn_default("oo_refresh", tr("刷新"), true).on_click(cx.listener(
            move |app: &mut BnApp, _, _, cx| {
                app.state
                    .send_cmd(Command::RefreshOpenOrders { market: app.market });
                cx.notify();
            },
        )),
    );
    if !list.is_empty() || !algo.is_empty() {
        let sym = app.page.symbol.clone();
        let sym_open = list.iter().filter(|o| o.symbol == sym.as_str()).count()
            + algo.iter().filter(|o| o.symbol == sym.as_str()).count();
        // 当前交易对没有挂单时禁用，避免"点了没反应"的静默空操作。
        // 两步确认（与本市场全撤同一纪律）：第一击进入待确认态，
        // 第二击才真正发命令；撤销的是真实挂单，误点代价不可逆
        let is_armed = app.sym_cancel_armed.as_deref() == Some(sym.as_str());
        let label = if is_armed {
            format!("{} {}({sym_open})", tr("确认撤销"), sym)
        } else {
            format!("{} {}({sym_open})", tr("撤销全部"), sym)
        };
        let mut b = widgets::btn_danger("sym_cancel_all", label, sym_open > 0);
        if sym_open == 0 {
            b = b.tooltip(move |_, cx| widgets::tooltip_view(tr("该交易对无挂单").to_string(), cx));
        }
        bar = bar.child(b.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app: &mut BnApp, _, _, cx| {
                let sym = app.page.symbol.clone();
                let n = {
                    let o = app.state.orders.lock();
                    o.open(app.market).iter().filter(|x| x.symbol == sym).count()
                        + o.algo.iter().filter(|x| x.symbol == sym).count()
                };
                if n == 0 {
                    return;
                }
                if app.sym_cancel_armed.as_deref() == Some(sym.as_str()) {
                    app.sym_cancel_armed = None;
                    app.state.send_cmd(Command::CancelAll {
                        market: app.market,
                        symbol: Some(sym),
                    });
                } else {
                    app.sym_cancel_armed = Some(sym);
                    // 与行级/市场级武装互斥：同一时刻只有一个待确认态
                    app.row_cancel_armed = None;
                    app.market_cancel_armed = false;
                }
                cx.notify();
                cx.stop_propagation();
            }),
        ));
        // 撤销本市场全部挂单（跨交易对；两次点击确认，防误扫全局撤销）
        let mkt_all_n = list.len() + algo.len();
        let armed = app.market_cancel_armed;
        let label = if armed {
            format!("{}({mkt_all_n})", tr("确认撤销本市场全部"))
        } else {
            format!("{}({mkt_all_n})", tr("撤销本市场全部"))
        };
        bar = bar.child(
            widgets::btn_danger("mkt_cancel_all", label, true)
                .tooltip(move |_, cx| {
                    widgets::tooltip_view(tr("撤销该市场所有交易对的全部挂单").to_string(), cx)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|app: &mut BnApp, _, _, cx| {
                        if app.market_cancel_armed {
                            app.market_cancel_armed = false;
                            app.state.send_cmd(Command::CancelAll {
                                market: app.market,
                                symbol: None,
                            });
                        } else {
                            app.market_cancel_armed = true;
                            app.sym_cancel_armed = None;
                            app.row_cancel_armed = None;
                        }
                        cx.notify();
                        cx.stop_propagation();
                    }),
                ),
        );
    }

    // ---- 空态 / 表格 ----
    if list.is_empty() && algo.is_empty() {
        return page_root(
            bar,
            centered_empty("◎", tr("暂无挂单"), tr("下单后将显示在这里")),
        );
    }
    let cols = [
        cw(68.0, false), // 时间
        cw(72.0, true),  // 交易对（右对齐）
        cw(36.0, false), // 方向
        cw(50.0, false), // 类型
        cw(70.0, true),  // 价格
        cw(70.0, true),  // 数量
        cw(70.0, true),  // 已成交
        cw(44.0, false), // 状态
        cw(0.0, true),   // 操作（弹性，按钮贴行尾）
    ];
    let labels = ["时间", "交易对", "方向", "类型", "价格", "数量", "已成交", "状态", "操作"];
    let header = header_row(&cols, &labels, &[""; 9]);
    let t = cur();
    let mut rows = Vec::new();
    for (i, o) in list.iter().enumerate() {
        let (dir_txt, dir_c) = if o.side == "BUY" {
            (tr("买入"), t.up)
        } else {
            (tr("卖出"), t.down)
        };
        let extra = if o.reduce_only {
            format!(" {}", tr("只减"))
        } else {
            String::new()
        };
        // 止损/止盈市价单 price 为 0，真实价格在 stopPrice
        let p = match o.price.filter(|p| !p.is_zero()) {
            Some(p) => widgets::fmt_dec_fixed(&p, pxd(&o.symbol)),
            None => match o.stop_price.filter(|s| !s.is_zero()) {
                Some(s) => format!(
                    "{} {}",
                    tr("触发"),
                    widgets::fmt_dec_fixed(&s, pxd(&o.symbol))
                ),
                None => tr("市价").to_string(),
            },
        };
        let row_armed = app.row_cancel_armed == Some(o.order_id);
        let cancel_txt = if row_armed {
            tr("确认撤销")
        } else {
            tr("撤销")
        };
        let oid = o.order_id;
        let sym_c = o.symbol.clone();
        rows.push(tr_row(
            i,
            theme::TABLE_ROW_H,
            vec![
                td(&cols[0], widgets::num_cell(widgets::fmt_time(o.time), None)),
                td(&cols[1], widgets::num_cell(o.symbol.clone(), None)),
                td(
                    &cols[2],
                    div()
                        .text_size(font_md())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(dir_c)
                        .whitespace_nowrap()
                        .child(dir_txt),
                ),
                td(&cols[3], txt(format!("{}{extra}", type_cn(&o.order_type)))),
                td(&cols[4], widgets::num_cell(p, None)),
                td(
                    &cols[5],
                    widgets::num_cell(widgets::fmt_dec_fixed(&o.orig_qty, qtd(&o.symbol)), None),
                ),
                td(
                    &cols[6],
                    widgets::num_cell(
                        widgets::fmt_dec_fixed(&o.executed_qty, qtd(&o.symbol)),
                        None,
                    ),
                ),
                td(
                    &cols[7],
                    div()
                        .text_size(font_md())
                        .text_color(t.accent)
                        .whitespace_nowrap()
                        .child(status_cn(&o.status)),
                ),
                td(
                    &cols[8],
                    // 两步确认：第一击武装（标签变为"确认撤销"），
                    // 第二击才发撤单命令；armed 键 = 订单号
                    widgets::btn_danger(("oo_cancel", i), cancel_txt, true)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |app: &mut BnApp, _, _, cx| {
                                if app.row_cancel_armed == Some(oid) {
                                    app.row_cancel_armed = None;
                                    app.state.send_cmd(Command::CancelOrder {
                                        market: app.market,
                                        symbol: sym_c.clone(),
                                        order_id: oid,
                                    });
                                } else {
                                    app.row_cancel_armed = Some(oid);
                                    app.sym_cancel_armed = None;
                                    app.market_cancel_armed = false;
                                }
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        ),
                ),
            ],
        ));
    }
    // 条件单（Algo 体系：仓位止盈止损/追踪委托等）：与普通挂单
    // 同构展示，触发价即生效价；closePosition 整仓单 quantity
    // 恒 0，以「整仓」标注
    for (i, a) in algo.iter().enumerate() {
        let (dir_txt, dir_c) = if a.side == "BUY" {
            (tr("买入"), t.up)
        } else {
            (tr("卖出"), t.down)
        };
        let extra = if a.reduce_only {
            format!(" {}", tr("只减"))
        } else {
            String::new()
        };
        // 条件单以触发价为生效价（整仓/市价条件单无委托价）
        let p = match a.trigger_price.filter(|p| !p.is_zero()) {
            Some(s) => format!(
                "{} {}",
                tr("触发"),
                widgets::fmt_dec_fixed(&s, pxd(&a.symbol))
            ),
            None => tr("市价").to_string(),
        };
        let qty_cell = if a.close_position {
            txt(tr("整仓"))
        } else {
            widgets::num_cell(widgets::fmt_dec_fixed(&a.quantity, qtd(&a.symbol)), None)
        };
        let row_armed = app.row_cancel_armed == Some(a.algo_id);
        let cancel_txt = if row_armed {
            tr("确认撤销")
        } else {
            tr("撤销")
        };
        let algo_id = a.algo_id;
        rows.push(tr_row(
            list.len() + i,
            theme::TABLE_ROW_H,
            vec![
                td(
                    &cols[0],
                    widgets::num_cell(widgets::fmt_time(a.create_time), None),
                ),
                td(&cols[1], widgets::num_cell(a.symbol.clone(), None)),
                td(
                    &cols[2],
                    div()
                        .text_size(font_md())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(dir_c)
                        .whitespace_nowrap()
                        .child(dir_txt),
                ),
                td(&cols[3], txt(format!("{}{extra}", type_cn(&a.order_type)))),
                td(&cols[4], widgets::num_cell(p, None)),
                td(&cols[5], qty_cell),
                // 触发前已成交恒 0：触发后整单转出为实际订单
                td(
                    &cols[6],
                    widgets::num_cell(
                        widgets::fmt_dec_fixed(&a.actual_qty, qtd(&a.symbol)),
                        None,
                    ),
                ),
                td(
                    &cols[7],
                    div()
                        .text_size(font_md())
                        .text_color(t.accent)
                        .whitespace_nowrap()
                        .child(status_cn(&a.algo_status)),
                ),
                td(
                    &cols[8],
                    // 两步确认，armed 键 = algoId（与普通单 orderId 同一
                    // 武装槽位：id 空间相距极远不会撞）
                    widgets::btn_danger(("algo_cancel", i), cancel_txt, true)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |app: &mut BnApp, _, _, cx| {
                                if app.row_cancel_armed == Some(algo_id) {
                                    app.row_cancel_armed = None;
                                    app.state
                                        .send_cmd(Command::CancelAlgoOrder { algo_id });
                                } else {
                                    app.row_cancel_armed = Some(algo_id);
                                    app.sym_cancel_armed = None;
                                    app.market_cancel_armed = false;
                                }
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        ),
                ),
            ],
        ));
    }
    page_root(bar, table("tbl_open", 544.0, header, rows))
}

/// 合约持仓
fn show_positions(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let tr = crate::i18n::tr;
    let state = Arc::clone(&app.state);
    // 持仓均为 U 本位合约；价格小数位 = quote 资产侧精度（tick 小数位），
    // 数量 = base 资产侧精度（step 小数位）
    let market = Market::UmFutures;
    let pxd = |sym: &str| {
        state.with_rule(market, sym, |r| {
            r.map(|x| x.price_dp() as usize).unwrap_or(2)
        })
    };
    let qtd =
        |sym: &str| state.with_rule(market, sym, |r| r.map(|x| x.qty_dp() as usize).unwrap_or(4));
    if !state.has_creds() {
        return centered_empty("◎", tr("未配置 API 密钥"), tr("在设置页配置密钥后查看"));
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

    // ---- 工具条 ----
    let mut bar = widgets::form_row().gap(px(theme::SPACE_LG));
    bar = bar.child(
        div()
            .text_size(font_md())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(format!(
                "{} · {}{}",
                tr("U本位合约"),
                positions.len(),
                tr("个持仓")
            )),
    );
    bar = bar.child(
        widgets::btn_default("pos_refresh", tr("刷新"), true).on_click(cx.listener(
            |app: &mut BnApp, _, _, cx| {
                app.state.send_cmd(Command::RefreshAccount);
                cx.notify();
            },
        )),
    );
    bar = bar.child(
        widgets::btn_default(
            "pos_dual",
            if dual {
                tr("切换单向持仓")
            } else {
                tr("切换双向持仓")
            },
            true,
        )
        .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
            app.state.send_cmd(Command::SetDualSide { dual: !dual });
            cx.notify();
        })),
    );
    // 一键平仓：全部非零持仓聚合为一个确认弹窗（真实资金纪律不破例）
    if !positions.is_empty() {
        bar = bar.child(
            widgets::btn_danger("pos_close_all", tr("全部平仓"), true)
                .tooltip(move |_, cx| {
                    widgets::tooltip_view(tr("市价平仓所有持仓（聚合确认）").to_string(), cx)
                })
                .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                    crate::ui::trade::confirm_close_all(app);
                    cx.notify();
                })),
        );
    }

    if positions.is_empty() {
        return page_root(
            bar,
            centered_empty("◎", tr("暂无持仓"), tr("下单后将显示在这里")),
        );
    }

    let cols = [
        cw(68.0, true),   // 交易对
        cw(32.0, false),  // 方向
        cw(56.0, true),   // 数量
        cw(68.0, true),   // 开仓价
        cw(80.0, true),   // 损益两平价
        cw(64.0, true),   // 标记价
        cw(68.0, true),   // 未实现盈亏
        cw(62.0, true),   // 强平价
        cw(28.0, true),   // 杠杆
        cw(104.0, true),  // 仓位止盈止损
        cw(0.0, false),   // 操作（弹性）
    ];
    let labels = [
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
    ];
    let mut tips = [""; 11];
    // 仓位止盈止损语义特殊（整仓触发、平仓即取消），表头挂完整说明
    tips[9] = "该止盈止损针对整个仓位（无论加仓或者减仓）。平仓后该止盈止损将被自动取消。当价格到达设定的触发价格时，将执行市价平仓委托。如果仓位数量超过市价委托单笔最大数量限制，委托将被拒绝。";
    let header = header_row(&cols, &labels, &tips);
    let t = cur();
    let mut rows = Vec::new();
    for (i, p) in positions.iter().enumerate() {
        let long = p.amt > Decimal::ZERO;
        let (dir_txt, dir_c) = if long {
            (tr("多"), t.up)
        } else {
            (tr("空"), t.down)
        };
        // 损益两平价（含手续费/资金费；接口无值时为 0 → 显示 —）
        let bep = if p.break_even_price.is_zero() {
            "—".to_string()
        } else {
            widgets::fmt_dec_fixed(&p.break_even_price, pxd(&p.symbol))
        };
        let uc = if p.unrealized >= Decimal::ZERO {
            t.up
        } else {
            t.down
        };
        let usign = if p.unrealized >= Decimal::ZERO {
            "+"
        } else {
            ""
        };
        let liq = if p.liquidation_price.is_zero() {
            "—".to_string()
        } else {
            widgets::fmt_dec_fixed(&p.liquidation_price, pxd(&p.symbol))
        };
        // symbolConfig 未返回（如拉取失败）时为 0 → 显示 —
        let lev = widgets::f64_decimal(&p.leverage) as i64;
        // 仓位止盈/止损触发价（0 = 未设置）：止盈用涨色、止损用跌色，
        // 未设置显示 —。v3 positionRisk 实测不返回该字段，从挂单中的
        // 整仓条件单推导（与 TP/SL 按钮提交的单子同源）
        let (tp_px, sl_px) = pos_tpsl(p, &fut_open, &fut_algo);
        let tpsl_cell = if tp_px.is_zero() && sl_px.is_zero() {
            widgets::num_cell("—", None)
        } else {
            let dp = pxd(&p.symbol);
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
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_SM))
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(font_sm())
                        .text_color(t.down)
                        .whitespace_nowrap()
                        .child(sl),
                )
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(font_sm())
                        .text_color(t.text_dim)
                        .child("/"),
                )
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(font_sm())
                        .text_color(t.up)
                        .whitespace_nowrap()
                        .child(tp),
                )
        };
        let sym_tpsl = p.symbol.clone();
        let sym_close = p.symbol.clone();
        let sym_rev = p.symbol.clone();
        let qty = p.amt.abs();
        let pos_side = FutPosSide::parse(&p.side);
        let pos_side_close = pos_side;
        let pos_side_rev = pos_side;
        rows.push(tr_row(
            i,
            theme::TABLE_ROW_H,
            vec![
                td(&cols[0], widgets::num_cell(p.symbol.clone(), None)),
                td(
                    &cols[1],
                    div()
                        .text_size(font_md())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(dir_c)
                        .child(dir_txt),
                ),
                td(
                    &cols[2],
                    widgets::num_cell(widgets::fmt_dec_fixed(&p.amt.abs(), qtd(&p.symbol)), None),
                ),
                td(
                    &cols[3],
                    widgets::num_cell(widgets::fmt_dec_fixed(&p.entry_price, pxd(&p.symbol)), None),
                ),
                td(&cols[4], widgets::num_cell(bep, None)),
                td(
                    &cols[5],
                    widgets::num_cell(widgets::fmt_dec_fixed(&p.mark_price, pxd(&p.symbol)), None),
                ),
                td(
                    &cols[6],
                    widgets::num_cell(
                        format!("{usign}{}", widgets::fmt_dec_fixed(&p.unrealized, pxd(&p.symbol))),
                        Some(uc),
                    ),
                ),
                td(&cols[7], widgets::num_cell(liq, None)),
                td(
                    &cols[8],
                    widgets::num_cell(if lev > 0 { format!("{lev}x") } else { "—".into() }, None),
                ),
                td(&cols[9], tpsl_cell),
                // 操作列：视觉顺序（自左向右）止盈止损 | 平仓 | 反手交易——
                // 平仓紧邻反手交易（两者都是「了结当前持仓」的高频操作），
                // TP/SL 作为条件单配置单独靠左
                td(
                    &cols[10],
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::SPACE_SM))
                        // 定宽按钮：中文四字与英文 TP/SL 长度不一，定宽保证
                        // 两种语言都不截断且点击区足够大
                        .child(
                            widgets::btn_default_w(("pos_tpsl", i), tr("止盈止损"), 72.0, true)
                                .tooltip(move |_, cx| {
                                    widgets::tooltip_view(
                                        tr("附加止盈/止损（触发后市价全平）").to_string(),
                                        cx,
                                    )
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |app: &mut BnApp, _, _, cx| {
                                        app.tpsl_edit = Some((sym_tpsl.clone(), long, pos_side));
                                        app.tpsl_sl_field.update(cx, |f, _| f.clear());
                                        app.tpsl_tp_field.update(cx, |f, _| f.clear());
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            widgets::btn_danger(("pos_close", i), tr("平仓"), true)
                                .tooltip(move |_, cx| {
                                    widgets::tooltip_view(
                                        tr("市价平仓（有确认弹窗）").to_string(),
                                        cx,
                                    )
                                })
                                .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                                    let side = if long { Side::Sell } else { Side::Buy };
                                    crate::ui::trade::confirm_market_close(
                                        app,
                                        sym_close.clone(),
                                        side,
                                        qty,
                                        pos_side_close,
                                    );
                                    cx.notify();
                                })),
                        )
                        .child(
                            widgets::btn_default(("pos_reverse", i), tr("反手交易"), true)
                                .tooltip(move |_, cx| {
                                    widgets::tooltip_view(
                                        tr("市价平仓并反向开同等数量（有确认弹窗）").to_string(),
                                        cx,
                                    )
                                })
                                .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                                    crate::ui::trade::confirm_market_reverse(
                                        app,
                                        sym_rev.clone(),
                                        long,
                                        qty,
                                        pos_side_rev,
                                    );
                                    cx.notify();
                                })),
                        ),
                ),
            ],
        ));
    }
    page_root(bar, table("tbl_positions", 712.0, header, rows))
}

/// 订单历史
fn show_history(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let tr = crate::i18n::tr;
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        return centered_empty("◎", tr("未配置 API 密钥"), tr("在设置页配置密钥后查看"));
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

    // ---- 工具条 ----
    let mut bar = widgets::form_row().gap(px(theme::SPACE_LG));
    bar = bar.child(match &key {
        Some((m, s)) => div()
            .text_size(font_md())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(format!(
                "{} {} · {}",
                m.label(),
                s,
                // 合约历史按 30 天窗口分段拉取（默认窗口太窄），
                // 现货仍是单次 limit=100
                if *m == Market::UmFutures {
                    tr("最近 30 天")
                } else {
                    tr("最近 100 笔")
                }
            )),
        None => div()
            .text_size(font_md())
            .text_color(t_text_dim())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(tr("切换交易对后自动加载")),
    });
    // 基础单/条件单分段筛选：pill_tab 与两侧按钮同为 BTN_H，整行同高对齐
    for (i, k) in [HistKind::All, HistKind::Basic, HistKind::Conditional]
        .into_iter()
        .enumerate()
    {
        bar = bar.child(
            widgets::pill_tab(("hist_kind", i), k.label(), app.hist_kind == k).on_click(
                cx.listener(move |app: &mut BnApp, _, _, cx| {
                    app.hist_kind = k;
                    cx.notify();
                }),
            ),
        );
    }
    // 不再要求 key 已存在：key 为空（首次进入/上次拉取失败）时用
    // 当前页市场+交易对兜底，避免"按钮灰死"导致永远无法手动重试
    bar = bar.child(
        widgets::btn_default("hist_refresh", tr("刷新"), true).on_click(cx.listener(
            move |app: &mut BnApp, _, _, cx| {
                let (m, s) = app
                    .state
                    .orders
                    .lock()
                    .history_symbol
                    .clone()
                    .unwrap_or((app.market, app.page.symbol.clone()));
                app.state.send_cmd(Command::RefreshHistory { market: m, symbol: s });
                cx.notify();
            },
        )),
    );
    // 导出当前筛选下展示的订单历史
    let mut b = widgets::btn_default("hist_export", tr("导出 CSV"), !view.is_empty());
    if view.is_empty() {
        let tip = if hist.is_empty() {
            tr("暂无订单历史")
        } else {
            tr("当前筛选无订单")
        };
        b = b.tooltip(move |_, cx| widgets::tooltip_view(tip.to_string(), cx));
    }
    bar = bar.child(b.on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
        // 监听器闭包不能借用渲染期的 view：从 Arc 快照重筛（clone 只复制引用计数）
        let hist = app.state.orders.lock().history.clone();
        let kind = app.hist_kind;
        let key = app.state.orders.lock().history_symbol.clone();
        let view: Vec<OrderInfo> = hist
            .iter()
            .filter(|o| match kind {
                HistKind::All => true,
                HistKind::Basic => !o.is_conditional,
                HistKind::Conditional => o.is_conditional,
            })
            .cloned()
            .collect();
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
            &app.state,
            write_csv(
                &stem,
                &[
                    "time", "symbol", "type", "side", "price", "qty", "executed", "status",
                ],
                &rows,
            ),
        );
        cx.notify();
    })));

    if hist.is_empty() {
        return page_root(
            bar,
            centered_empty("◎", tr("暂无订单历史"), tr("下单后将显示在这里")),
        );
    }
    if view.is_empty() {
        // 有历史但被筛选全部过滤（如只看基础单而该交易对只有条件单）
        return page_root(
            bar,
            centered_empty("◎", tr("当前筛选无订单"), tr("切换筛选查看其它订单")),
        );
    }

    let cols = [
        cw(78.0, false), // 时间
        cw(72.0, false), // 交易对
        cw(36.0, false), // 方向
        cw(50.0, false), // 类型
        cw(70.0, true),  // 价格
        cw(70.0, true),  // 数量
        cw(70.0, true),  // 已成交
        cw(0.0, false),  // 状态（弹性）
    ];
    let labels = ["时间", "交易对", "方向", "类型", "价格", "数量", "已成交", "状态"];
    let header = header_row(&cols, &labels, &[""; 8]);
    let t = cur();
    let rows = view
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let dir_c = if o.side == "BUY" { t.up } else { t.down };
            let word = if o.side == "BUY" {
                tr("买入")
            } else {
                tr("卖出")
            };
            // 止损/止盈单 price 为 0 时展示触发价
            let p = match o.price.filter(|p| !p.is_zero()) {
                Some(p) => widgets::fmt_dec_fixed(&p, pxd(&o.symbol)),
                None => match o.stop_price.filter(|s| !s.is_zero()) {
                    Some(s) => format!(
                        "{} {}",
                        tr("触发"),
                        widgets::fmt_dec_fixed(&s, pxd(&o.symbol))
                    ),
                    None => tr("市价").to_string(),
                },
            };
            tr_row(
                i,
                theme::TABLE_ROW_H,
                vec![
                    td(&cols[0], widgets::num_cell(widgets::fmt_time(o.time), None)),
                    td(&cols[1], mono(o.symbol.clone())),
                    td(
                        &cols[2],
                        div()
                            .text_size(font_md())
                            .text_color(dir_c)
                            .whitespace_nowrap()
                            .child(word),
                    ),
                    td(&cols[3], txt(type_cn(&o.order_type))),
                    td(&cols[4], widgets::num_cell(p, None)),
                    td(
                        &cols[5],
                        widgets::num_cell(
                            widgets::fmt_dec_fixed(&o.orig_qty, qtd(&o.symbol)),
                            None,
                        ),
                    ),
                    td(
                        &cols[6],
                        widgets::num_cell(
                            widgets::fmt_dec_fixed(&o.executed_qty, qtd(&o.symbol)),
                            None,
                        ),
                    ),
                    td(
                        &cols[7],
                        div()
                            .text_size(font_md())
                            .text_color(status_color(&o.status))
                            .whitespace_nowrap()
                            .child(status_cn(&o.status)),
                    ),
                ],
            )
        })
        .collect::<Vec<_>>();
    page_root(bar, table("tbl_history", 494.0, header, rows))
}

/// 成交历史
fn show_trades_history(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let tr = crate::i18n::tr;
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        return centered_empty("◎", tr("未配置 API 密钥"), tr("在设置页配置密钥后查看"));
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

    // ---- 工具条 ----
    let mut bar = widgets::form_row().gap(px(theme::SPACE_LG));
    bar = bar.child(
        div()
            .text_size(font_md())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(format!("{}{}{}", tr("最近"), trades.len(), tr("笔成交"))),
    );
    // 同 show_history：key 为空时用当前页市场+交易对兜底，保证可手动重试
    bar = bar.child(
        widgets::btn_default("th_refresh", tr("刷新"), true).on_click(cx.listener(
            move |app: &mut BnApp, _, _, cx| {
                let (m, s) = app
                    .state
                    .orders
                    .lock()
                    .history_symbol
                    .clone()
                    .unwrap_or((app.market, app.page.symbol.clone()));
                app.state.send_cmd(Command::RefreshHistory { market: m, symbol: s });
                cx.notify();
            },
        )),
    );
    let mut b = widgets::btn_default("th_export", tr("导出 CSV"), !trades.is_empty());
    if trades.is_empty() {
        b = b.tooltip(move |_, cx| widgets::tooltip_view(tr("暂无成交").to_string(), cx));
    }
    bar = bar.child(b.on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
        let trades = app.state.orders.lock().trades.clone();
        let hist_key = app.state.orders.lock().history_symbol.clone();
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
            &app.state,
            write_csv(
                &stem,
                &[
                    "time", "symbol", "side", "price", "qty", "quote_qty", "fee", "fee_asset",
                    "role",
                ],
                &rows,
            ),
        );
        cx.notify();
    })));

    if trades.is_empty() {
        return page_root(
            bar,
            centered_empty("◎", tr("暂无成交"), tr("下单后将显示在这里")),
        );
    }

    let cols = [
        cw(78.0, false), // 时间
        cw(72.0, false), // 交易对
        cw(36.0, false), // 方向
        cw(70.0, true),  // 价格
        cw(70.0, true),  // 数量
        cw(80.0, true),  // 金额
        cw(60.0, true),  // 手续费
        cw(0.0, false),  // 角色（弹性）
    ];
    let labels = ["时间", "交易对", "方向", "价格", "数量", "金额", "手续费", "角色"];
    let header = header_row(&cols, &labels, &[""; 8]);
    let t = cur();
    let rows = trades
        .iter()
        .enumerate()
        .map(|(i, t2)| {
            let (dir_txt, dir_c) = if t2.is_buyer {
                (tr("买入"), t.up)
            } else {
                (tr("卖出"), t.down)
            };
            let q = if t2.quote_qty.is_zero() {
                t2.price * t2.qty
            } else {
                t2.quote_qty
            };
            tr_row(
                i,
                theme::TABLE_ROW_H,
                vec![
                    td(&cols[0], widgets::num_cell(widgets::fmt_time(t2.time), None)),
                    td(&cols[1], mono(t2.symbol.clone())),
                    td(
                        &cols[2],
                        div()
                            .text_size(font_md())
                            .text_color(dir_c)
                            .whitespace_nowrap()
                            .child(dir_txt),
                    ),
                    td(
                        &cols[3],
                        widgets::num_cell(widgets::fmt_dec_fixed(&t2.price, pxd(&t2.symbol)), None),
                    ),
                    td(
                        &cols[4],
                        widgets::num_cell(widgets::fmt_dec_fixed(&t2.qty, qtd(&t2.symbol)), None),
                    ),
                    td(
                        &cols[5],
                        widgets::num_cell(widgets::fmt_dec_fixed(&q, pxd(&t2.symbol)), None),
                    ),
                    td(
                        &cols[6],
                        widgets::num_cell(
                            format!(
                                "{} {}",
                                widgets::fmt_dec_grouped(&t2.commission),
                                t2.commission_asset
                            ),
                            Some(t.text_dim),
                        ),
                    ),
                    td(
                        &cols[7],
                        div()
                            .text_size(font_md())
                            .text_color(t.text_dim)
                            .whitespace_nowrap()
                            .child(if t2.is_maker { "maker" } else { "taker" }),
                    ),
                ],
            )
        })
        .collect::<Vec<_>>();
    page_root(bar, table("tbl_trades", 514.0, header, rows))
}

/// 资金流水：合约 income（划转/资金费/已实现盈亏/手续费），
/// 现货充值+提现合并；按市场查询，数据归属见 flow_market
fn show_flow(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let tr = crate::i18n::tr;
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        return centered_empty("◎", tr("未配置 API 密钥"), tr("在设置页配置密钥后查看"));
    }
    // 单锁取数（与 show_history 同构）：flow 为 Arc，clone 不深拷贝
    let (flows, flow_market) = {
        let o = state.orders.lock();
        (o.flow.clone(), o.flow_market)
    };

    // ---- 工具条 ----
    let mut bar = widgets::form_row().gap(px(theme::SPACE_LG));
    bar = bar.child(match flow_market {
        Some(m) => div()
            .text_size(font_md())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(format!("{} · {}", m.label(), tr("最近 100 条"))),
        None => div()
            .text_size(font_md())
            .text_color(t_text_dim())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(tr("进入页签后自动加载")),
    });
    bar = bar.child(
        widgets::btn_default("flow_refresh", tr("刷新"), true).on_click(cx.listener(
            move |app: &mut BnApp, _, _, cx| {
                app.state
                    .send_cmd(Command::RefreshFlow { market: app.market });
                cx.notify();
            },
        )),
    );
    let mut b = widgets::btn_default("flow_export", tr("导出 CSV"), !flows.is_empty());
    if flows.is_empty() {
        b = b.tooltip(move |_, cx| widgets::tooltip_view(tr("暂无资金流水").to_string(), cx));
    }
    bar = bar.child(b.on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
        let flows = app.state.orders.lock().flow.clone();
        let market = app.market;
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
            &app.state,
            write_csv(
                &stem,
                &["time", "type", "symbol", "asset", "amount", "note"],
                &rows,
            ),
        );
        cx.notify();
    })));

    if flows.is_empty() {
        return page_root(
            bar,
            centered_empty(
                "◎",
                tr("暂无资金流水"),
                tr("划转、资金费、充值与提现将显示在这里"),
            ),
        );
    }

    let cols = [
        cw(78.0, false), // 时间
        cw(64.0, false), // 类型
        cw(72.0, false), // 交易对
        cw(52.0, false), // 资产
        cw(96.0, true),  // 金额
        cw(0.0, false),  // 备注（弹性）
    ];
    let labels = ["时间", "类型", "交易对", "资产", "金额", "备注"];
    let header = header_row(&cols, &labels, &[""; 6]);
    let t = cur();
    let rows = flows
        .iter()
        .enumerate()
        .map(|(i, f)| {
            // 流入正色/流出负色，与买卖方向配色一致
            let c = if f.amount.is_zero() {
                t.text
            } else if f.amount > Decimal::ZERO {
                t.up
            } else {
                t.down
            };
            let sign = if f.amount > Decimal::ZERO { "+" } else { "" };
            let sym_cell = if f.symbol.is_empty() {
                // 划转/充值/提现等全局条目 symbol 为空
                div()
                    .text_size(font_md())
                    .text_color(t.text_dim)
                    .child("—")
            } else {
                mono(f.symbol.clone())
            };
            tr_row(
                i,
                theme::TABLE_ROW_H,
                vec![
                    td(&cols[0], widgets::num_cell(widgets::fmt_time(f.time), None)),
                    td(&cols[1], txt(crate::i18n::tr(f.kind))),
                    td(&cols[2], sym_cell),
                    td(&cols[3], mono(f.asset.clone())),
                    td(
                        &cols[4],
                        widgets::num_cell(
                            format!("{sign}{} {}", widgets::fmt_dec_grouped(&f.amount), f.asset),
                            Some(c),
                        ),
                    ),
                    td(
                        &cols[5],
                        div()
                            .text_size(font_md())
                            .text_color(t.text_dim)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(f.note.clone()),
                    ),
                ],
            )
        })
        .collect::<Vec<_>>();
    page_root(bar, table("tbl_flow", 434.0, header, rows))
}

/// 工具条暗字快捷取色（statusline 用处极多，独立小函数省一行）
fn t_text_dim() -> gpui::Rgba {
    cur().text_dim
}

/// 持仓行内 TP/SL 弹窗：两个触发价输入，提交进入确认弹窗（真实资金纪律不破例）。
/// 无遮罩不挡下方 UI（egui Window 同语义），关闭路径 = 关闭键 / Esc / 提交成功
pub fn tpsl_dialog(app: &mut BnApp, cx: &mut Context<BnApp>) -> Option<Div> {
    let (symbol, long, _pos_side) = app.tpsl_edit.as_ref()?;
    let t = cur();
    let tr = crate::i18n::tr;
    let dir_txt = if *long { tr("多") } else { tr("空") };
    let title = format!("{} · {} {}", tr("止盈 / 止损"), symbol, dir_txt);
    // 输入框：egui 版 Monospace 116pt；GPUI TextField 数字模式 + 定宽容器
    let field_row = |label: &'static str, f: &Entity<TextField>| -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::SPACE_LG))
            .child(div().text_size(font_md()).child(tr(label)))
            .child(div().w(px(116.0)).child(f.clone()))
    };
    let card = div()
        .min_w(px(268.0))
        .rounded(px(theme::R_LG))
        .bg(t.panel2)
        .border_1()
        .border_color(faded(t.border, 0.7))
        // 悬浮层 elevation：无遮罩悬挂卡片靠阴影与页面分层
        .shadow_lg()
        .p(px(theme::SPACE_XL))
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .child(
            div()
                .text_size(theme::font_lg())
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .child(title),
        )
        .child(
            div()
                .text_size(font_sm())
                .text_color(t.text_dim)
                .child(tr("触发后市价全平该持仓，提交前需确认")),
        )
        .child(field_row("止损触发", &app.tpsl_sl_field))
        .child(field_row("止盈触发", &app.tpsl_tp_field))
        .child(
            div()
                .pt(px(theme::SPACE_XS))
                .flex()
                .flex_row()
                .justify_end()
                .items_center()
                .gap(px(theme::SPACE_LG))
                .child(
                    widgets::btn_default("tpsl_close", tr("关闭"), true).on_click(cx.listener(
                        |app: &mut BnApp, _, _, cx| {
                            app.tpsl_edit = None;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    widgets::btn_solid("tpsl_submit", tr("提交"), t.accent, true).on_click(
                        cx.listener(|app: &mut BnApp, _, _, cx| {
                            let Some((symbol, long, pos_side)) = app.tpsl_edit.clone() else {
                                return;
                            };
                            let sl = app.tpsl_sl_field.read(cx).text().to_string();
                            let tp = app.tpsl_tp_field.read(cx).text().to_string();
                            match crate::ui::trade::confirm_position_tpsl(
                                app,
                                symbol,
                                long,
                                pos_side,
                                sl,
                                tp,
                            ) {
                                Ok(()) => app.tpsl_edit = None,
                                Err(e) => {
                                    app.state
                                        .push_toast(crate::engine::state::ToastKind::Warn, e);
                                }
                            }
                            cx.notify();
                        }),
                    ),
                ),
        );
    // 顶部居中悬挂（egui CENTER_TOP [0, 96] 同位）：容器无监听不挡下方点击。
    // 锚 = 顶栏 + 60pt 视觉下移，与页头下方留白对齐
    const ANCHOR_TOP: f32 = theme::TOPBAR_H + 60.0;
    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .child(div().mt(px(ANCHOR_TOP)).child(card)),
    )
}
