//! Portfolio 页：现货资产与合约资产分区总览（双卡片布局）
//!
//! 页头（标题 + 全局刷新）之下，现货与合约各一张卡片彻底分开：
//! 卡头 = 市场名 + 各自的操作/徽章槽，卡体 = 明细表；合约卡另有
//! USDT 计价的迷你统计行（可跨资产加总，现货不做跨币种假合计）。
//! 本页只关注资产本身——持仓/平仓在期货交易页的持仓页签。
//!
//! 里程碑 9：GPUI 移植。账户快照短锁拷贝后绘制（acct 是引擎写热点，
//! 不能在整个表格构建期间持有锁，与 chart.rs 的 clone-and-release 纪律一致）。

use crate::app::BnApp;
use crate::engine::state::SharedState;
use crate::engine::Command;
use crate::i18n::tr;
use crate::ui::theme::{self, cur, faded, font_body, font_lg, font_md};
use crate::ui::widgets;
use gpui::{
    div, prelude::FluentBuilder, px, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled,
};
use rust_decimal::Decimal;
use std::sync::Arc;

// ---------- 本页私有表格助手（列少且定宽，无需 orders.rs 的横滚框架） ----------

#[derive(Clone, Copy)]
struct Col {
    w: f32,
    right: bool,
}

const fn cw(w: f32, right: bool) -> Col {
    Col { w, right }
}

/// 数据单元格：定宽 + 数值列右对齐，溢出按边界裁剪（遮挡优于顶歪）
fn td(c: &Col, child: impl IntoElement) -> Div {
    div()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .px(px(theme::SPACE_MD))
        .w(px(c.w))
        .when(c.right, |d| d.justify_end())
        .child(child)
}

fn head_cell(txt: &'static str, c: &Col) -> Div {
    div()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .px(px(theme::SPACE_MD))
        .w(px(c.w))
        .when(c.right, |d| d.justify_end())
        .child(widgets::card_label(tr(txt)))
}

fn header_row(cols: &[Col], labels: &[&'static str]) -> Div {
    let t = cur();
    let mut h = div()
        .h(px(theme::TH_H))
        .flex()
        .flex_row()
        .items_center()
        .border_b_1()
        .border_color(faded(t.border, 0.6));
    for (i, c) in cols.iter().enumerate() {
        h = h.child(head_cell(labels[i], c));
    }
    h
}

fn tr_row(idx: usize, cells: Vec<Div>) -> Div {
    let t = cur();
    div()
        .h(px(theme::TABLE_ROW_H))
        .flex()
        .flex_row()
        .items_center()
        .when(idx % 2 == 1, |d| d.bg(faded(t.text, 0.035)))
        // 与 orders.rs 表格同语义：指针所在行整行点亮
        .hover(|s| s.bg(t.row_hover))
        .children(cells)
}

/// 资产代码：等宽字体
fn asset_code(code: &str) -> Div {
    div()
        .font_family(crate::ui::theme::FONT_MONO)
        .text_size(font_md())
        .text_color(cur().text)
        .child(code.to_string())
}

/// 账户列加载失败的显式错误行 + 重试入口（避免无限"加载中…"）
fn load_failed_row(_state: &Arc<SharedState>, err: Option<String>, cx: &mut Context<BnApp>) -> Div {
    match err {
        Some(e) => div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::SPACE_LG))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(font_body())
                    .text_color(cur().err)
                    .child(format!("{}{e}", tr("加载失败: "))),
            )
            .child(
                widgets::btn_default("assets_retry", tr("刷新账户"), true).on_click(cx.listener(
                    |app: &mut BnApp, _, _, cx| {
                        app.state.send_cmd(Command::RefreshAccount);
                        cx.notify();
                    },
                )),
            ),
        None => div().child(widgets::loading_state(tr("加载中…"))),
    }
}

const EMPTY_ASSETS_ICON: &str = "◎";

pub fn show(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let state = Arc::clone(&app.state);
    if !state.has_creds() {
        return div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .child(widgets::empty_state(
                EMPTY_ASSETS_ICON,
                tr("未配置 API 密钥，无法查看资产（可在设置页配置）"),
                "",
            ));
    }

    // ---------- 页头：标题 + 全局刷新 ----------
    let refresh_btn = widgets::btn_default("assets_refresh", tr("刷新账户"), true).on_click(
        cx.listener(|app: &mut BnApp, _, _, cx| {
            app.state.send_cmd(Command::RefreshAccount);
            cx.notify();
        }),
    );

    let mut root = div()
        .id("assets_page")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .child(
            div()
                .px(px(theme::SPACE_XL))
                .py(px(theme::SPACE_LG))
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_LG))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE_LG))
                        .child(
                            div()
                                .text_size(font_lg())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cur().text)
                                .child(tr("资产总览")),
                        )
                        .child(div().flex_1())
                        .child(refresh_btn),
                ),
        );

    // ---------- 现货资产卡片 ----------
    root = root.child(spot_section(&state, cx));

    // ---------- 合约资产卡片 ----------
    root = root.child(fut_section(&state, cx));

    // 外层再包一层弹性容器（root 带 id/滚动为 Stateful<Div>，页面统一返回 Div）
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .child(root)
}

fn spot_section(state: &Arc<SharedState>, cx: &mut Context<BnApp>) -> Div {
    // 短锁拷贝后绘制
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

    let card = widgets::card()
        .flex()
        .flex_col()
        .child(
            widgets::card_header(tr("现货资产"))
                .when(!can_trade, |h| {
                    h.child(widgets::badge(tr("该 Key 无交易权限"), cur().warn))
                })
                .when(!nonzero.is_empty(), |h| {
                    h.child(widgets::card_label(format!(
                        "{} {}",
                        nonzero.len(),
                        tr("个非零资产")
                    )))
                }),
        )
        .child(widgets::separator());

    if let Some(e) = err {
        return card.child(load_failed_row(state, Some(e), cx));
    }
    if nonzero.is_empty() {
        return card.child(widgets::empty_state(
            EMPTY_ASSETS_ICON,
            tr("暂无非零资产"),
            tr("余额到账后自动显示"),
        ));
    }

    // 冻结列仅在有冻结资产时显示：绝大多数账户该值常年为 0，
    // 一整列孤零零的暗色 0 只有噪音没有信息
    let has_locked = nonzero.iter().any(|b| b.locked != Decimal::ZERO);
    let cols: Vec<Col> = if has_locked {
        vec![cw(90.0, false), cw(130.0, true), cw(130.0, true), cw(130.0, true)]
    } else {
        vec![cw(90.0, false), cw(130.0, true), cw(130.0, true)]
    };
    let labels: Vec<&'static str> = if has_locked {
        vec!["资产", "钱包余额", "可用", "冻结"]
    } else {
        vec!["资产", "钱包余额", "可用"]
    };

    let rows: Vec<Div> = nonzero
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let mut cells = vec![
                td(&cols[0], asset_code(&b.asset)),
                td(
                    &cols[1],
                    widgets::num_cell(widgets::fmt_dec_grouped(&(b.free + b.locked)), None),
                ),
                td(
                    &cols[2],
                    widgets::num_cell(widgets::fmt_dec_grouped(&b.free), None),
                ),
            ];
            if has_locked {
                cells.push(td(
                    &cols[3],
                    widgets::num_cell(
                        widgets::fmt_dec_grouped(&b.locked),
                        Some(cur().text_dim),
                    ),
                ));
            }
            tr_row(i, cells)
        })
        .collect();

    card
        .child(header_row(&cols, &labels))
        .children(rows)
}

fn fut_section(state: &Arc<SharedState>, cx: &mut Context<BnApp>) -> Div {
    let (fut, dual, fut_err) = {
        let acct = state.acct.lock();
        (acct.fut.clone(), acct.fut_dual_side, acct.fut_err.clone())
    };

    let toggle_btn = widgets::btn_default("fut_dual_toggle", tr("切换"), true)
        .tooltip(move |_, cx| widgets::tooltip_view(tr("切换单向/双向持仓模式").to_string(), cx))
        .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
            app.state.send_cmd(Command::SetDualSide { dual: !dual });
            cx.notify();
        }));

    let mut card = widgets::card()
        .flex()
        .flex_col()
        .child(
            widgets::card_header(tr("合约资产 · U本位"))
                .child(toggle_btn)
                .child(widgets::card_label(if dual {
                    tr("持仓模式：双向")
                } else {
                    tr("持仓模式：单向")
                })),
        )
        .child(widgets::separator());

    let Some(a) = fut else {
        return card.child(load_failed_row(state, fut_err, cx));
    };

    // 迷你统计行：USDT 计价可跨资产加总；flex_wrap 窄窗折行不裁切
    let f = |d: &Decimal| widgets::f64_decimal(d);
    let stat = |label: &'static str, v: String, color: gpui::Rgba| -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(theme::SPACE_MD))
            .child(widgets::card_label(tr(label)))
            .child(
                div()
                    .font_family(crate::ui::theme::FONT_MONO)
                    .text_size(font_md())
                    .text_color(color)
                    .child(v),
            )
    };
    let stats = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x(px(18.0))
        .gap_y(px(4.0))
        .child(stat("钱包余额", widgets::fmt_thousands(f(&a.total_wallet), 4), cur().text))
        .child(stat("可用余额", widgets::fmt_thousands(f(&a.available), 4), cur().text))
        .child(stat("保证金余额", widgets::fmt_thousands(f(&a.total_margin), 4), cur().text))
        .child(stat(
            "未实现盈亏",
            widgets::fmt_thousands(f(&a.total_unrealized), 4),
            theme_signed(f(&a.total_unrealized)),
        ));
    card = card.child(stats);

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
        return card.child(widgets::empty_state(
            EMPTY_ASSETS_ICON,
            tr("暂无非零资产"),
            tr("余额到账后自动显示"),
        ));
    }

    let cols = [cw(90.0, false), cw(130.0, true), cw(130.0, true), cw(130.0, true)];
    let labels = ["资产", "钱包余额", "可用", "未实现盈亏"];

    let rows: Vec<Div> = assets
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let s = widgets::fmt_dec_grouped(&b.unrealized);
            let sign = if b.unrealized >= Decimal::ZERO { "+" } else { "" };
            tr_row(
                i,
                vec![
                    td(&cols[0], asset_code(&b.asset)),
                    td(&cols[1], widgets::num_cell(widgets::fmt_dec_grouped(&b.wallet), None)),
                    td(&cols[2], widgets::num_cell(widgets::fmt_dec_grouped(&b.available), None)),
                    td(
                        &cols[3],
                        widgets::num_cell(
                            format!("{sign}{s}"),
                            Some(theme_signed(widgets::f64_decimal(&b.unrealized))),
                        ),
                    ),
                ],
            )
        })
        .collect();

    card.child(header_row(&cols, &labels)).children(rows)
}

/// signed_color 的薄封装（本文件多处调用，缩短行宽）
fn theme_signed(d: f64) -> gpui::Rgba {
    crate::ui::theme::signed_color(d)
}
