//! 订单与持仓：当前挂单（撤销）、合约持仓（平仓/杠杆）、订单历史、成交历史
//!
//! 里程碑 1：状态映射、CSV 导出、仓位 TP/SL 推导等纯函数原样迁移；
//! 五个子页表格在里程碑 8 移植（uniform_list + 固定列宽 flex 行）。

use crate::binance::model::PositionRisk;
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
    let t = crate::ui::theme::cur();
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
