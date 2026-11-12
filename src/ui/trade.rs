//! 下单面板：买/卖、限价/市价/止损/止盈、数量百分比快捷填充、确认下单
//!
//! 里程碑 1：类型与全部纯逻辑（build_request 族/confirm_* 族/fill_by_pct/
//! order_rows）原样迁移。
//! 里程碑 7：表单与确认弹窗 UI（GPUI）。输入框是独立 TextField Entity
//! （文本状态在框内），OrderForm 的 String 字段仍是构建请求的单一数据源：
//! 框 on_change 写回字符串，渲染帧把外部变更（盘口点击/比例填充/切交易对）
//! 同步回框（文本不同才 set，不干扰输入中的光标）。

use crate::app::{BnApp, ConfirmOrder};
use crate::binance::client::{OrderKind, OrderRequest, Tif};
use crate::binance::{FutPosSide, Market, Side};
use crate::engine::Command;
use crate::ui::textfield::TextField;
use crate::ui::theme::{self, cur, faded, font_body, font_md, font_sm};
use crate::ui::widgets;
use gpui::{
    div, prelude::FluentBuilder, px, AppContext, Context, Div, ElementId, Entity, FontWeight,
    InteractiveElement, MouseButton, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, WeakEntity,
};
use rust_decimal::Decimal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiKind {
    Limit,
    Market,
    StopLossLimit,
    TakeProfitLimit,
    StopLossMarket,
    TakeProfitMarket,
    /// 合约跟踪委托（回调幅度触发市价全平）
    TrailingStop,
    /// 现货 OCO（限价腿 + 止损限价腿）
    Oco,
}

impl UiKind {
    pub fn label(&self) -> &'static str {
        match self {
            UiKind::Limit => crate::i18n::tr("限价"),
            UiKind::Market => crate::i18n::tr("市价"),
            UiKind::StopLossLimit => crate::i18n::tr("止损限价"),
            UiKind::TakeProfitLimit => crate::i18n::tr("止盈限价"),
            UiKind::StopLossMarket => crate::i18n::tr("止损市价"),
            UiKind::TakeProfitMarket => crate::i18n::tr("止盈市价"),
            UiKind::TrailingStop => crate::i18n::tr("跟踪委托"),
            UiKind::Oco => crate::i18n::tr("OCO"),
        }
    }

    pub fn available(market: Market) -> Vec<UiKind> {
        match market {
            Market::Spot => vec![
                UiKind::Limit,
                UiKind::Market,
                UiKind::StopLossLimit,
                UiKind::TakeProfitLimit,
                UiKind::Oco,
            ],
            Market::UmFutures => vec![
                UiKind::Limit,
                UiKind::Market,
                UiKind::StopLossMarket,
                UiKind::TakeProfitMarket,
                UiKind::TrailingStop,
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct OrderForm {
    pub side: Side,
    pub kind: UiKind,
    pub tif: Tif,
    pub price: String,
    pub qty: String,
    pub trigger: String,
    /// 跟踪委托回调幅度（百分数，0.1-10）
    pub callback_rate: String,
    /// 现货 OCO 止损腿限价
    pub stop_limit: String,
    pub reduce_only: bool,
    /// 合约附加止盈/止损（单向持仓模式；触发即全平）
    pub tpsl_on: bool,
    pub tp_price: String,
    pub sl_price: String,
    /// 校验错误文本：非空时在提交按钮上方显示红色提示条（替代仅 toast 的反馈）
    pub err: String,
}

impl Default for OrderForm {
    fn default() -> Self {
        Self {
            side: Side::Buy,
            kind: UiKind::Limit,
            tif: Tif::Gtc,
            price: String::new(),
            qty: String::new(),
            trigger: String::new(),
            callback_rate: "1".to_string(),
            stop_limit: String::new(),
            reduce_only: false,
            tpsl_on: false,
            tp_price: String::new(),
            sl_price: String::new(),
            err: String::new(),
        }
    }
}

impl OrderForm {
    /// 盘口点击价格
    pub fn set_price(&mut self, price: &str) {
        self.price = price.to_string();
    }
}

/// 从 UI 状态构建订单请求（量化后），供提交与确认弹窗使用
pub fn build_request(app: &BnApp) -> Result<OrderRequest, String> {
    let state = &app.state;
    let market = app.market;
    let symbol = app.page.symbol.clone();
    let Some(r) = state.rule_of(market, &symbol) else {
        return Err(crate::i18n::tr("交易规则尚未加载").to_string());
    };
    let qty = widgets::parse_dec(&app.page.form.qty).ok_or(crate::i18n::tr("请输入数量"))?;
    let price = widgets::parse_dec(&app.page.form.price);
    let trigger = widgets::parse_dec(&app.page.form.trigger);

    // 量化（Option 化防溢出）
    let q_price = |v: Decimal| {
        crate::binance::symbols::quantize_price(v, r.tick)
            .ok_or_else(|| crate::i18n::tr("价格超出可表示范围").to_string())
    };
    let q_qty = |v: Decimal| {
        crate::binance::symbols::quantize_qty(v, r.step)
            .ok_or_else(|| crate::i18n::tr("数量超出可表示范围").to_string())
    };
    let kind = match app.page.form.kind {
        UiKind::Limit => OrderKind::Limit {
            price: q_price(price.ok_or(crate::i18n::tr("请输入价格"))?)?,
            qty: q_qty(qty)?,
        },
        UiKind::Market => OrderKind::Market { qty: q_qty(qty)? },
        UiKind::StopLossLimit => OrderKind::StopLimit {
            trigger: q_price(trigger.ok_or(crate::i18n::tr("请输入触发价"))?)?,
            price: q_price(price.ok_or(crate::i18n::tr("请输入价格"))?)?,
            qty: q_qty(qty)?,
            take_profit: false,
        },
        UiKind::TakeProfitLimit => OrderKind::StopLimit {
            trigger: q_price(trigger.ok_or(crate::i18n::tr("请输入触发价"))?)?,
            price: q_price(price.ok_or(crate::i18n::tr("请输入价格"))?)?,
            qty: q_qty(qty)?,
            take_profit: true,
        },
        UiKind::StopLossMarket => OrderKind::StopMarket {
            trigger: q_price(trigger.ok_or(crate::i18n::tr("请输入触发价"))?)?,
            qty: q_qty(qty)?,
            take_profit: false,
        },
        UiKind::TakeProfitMarket => OrderKind::StopMarket {
            trigger: q_price(trigger.ok_or(crate::i18n::tr("请输入触发价"))?)?,
            qty: q_qty(qty)?,
            take_profit: true,
        },
        UiKind::TrailingStop => {
            let rate = widgets::parse_dec(&app.page.form.callback_rate)
                .ok_or(crate::i18n::tr("请输入回调幅度（0.1-10 的百分数）"))?;
            if !(Decimal::from_str_exact("0.1").unwrap_or_default()..=Decimal::from(10))
                .contains(&rate)
            {
                return Err(crate::i18n::tr("回调幅度需为 0.1-10 的百分数").to_string());
            }
            OrderKind::TrailingStop {
                callback_rate: rate.round_dp(1),
                qty: q_qty(qty)?,
            }
        }
        // OCO 走独立构建器（build_oco），此分支仅在类型枚举穷尽时兜底；
        // 启用判定与提交都走 check_oco，不会到达
        UiKind::Oco => OrderKind::Market {
            qty: q_qty(qty).unwrap_or_default(),
        },
    };
    let dual = state.acct.lock().fut_dual_side;
    // 止损/止盈为保护单（两个持仓模式同理）：已有持仓时它的语义是"保护"，
    // 而不是触发后开新仓
    let protective = matches!(
        app.page.form.kind,
        UiKind::StopLossLimit
            | UiKind::TakeProfitLimit
            | UiKind::StopLossMarket
            | UiKind::TakeProfitMarket
    );
    // 收集该交易对全部非零仓位：同 symbol 双向持仓时取 find 第一个
    // 可能平错边（如 SELL+LONG 平掉多头），两个方向都在仓时直接拒绝
    let existing: Option<bool> =
        if market == Market::UmFutures && protective {
            let longs: Vec<bool> = state
                .acct
                .lock()
                .fut_positions
                .iter()
                .filter(|p| p.symbol == symbol && p.amt != Decimal::ZERO)
                .map(|p| p.amt > Decimal::ZERO)
                .collect();
            match longs.len() {
                0 => None,
                1 => Some(longs[0]),
                _ => return Err(crate::i18n::tr(
                    "该交易对同时存在多空双向持仓，无法确定止盈止损的平仓方向，请先手动平掉一侧",
                )
                .to_string()),
            }
        } else {
            None
        };
    // 双向持仓：限价/市价跟随买卖方向开仓（买入=LONG，卖出=SHORT）。
    // 保护单在已有持仓时一律平掉现有持仓——持仓方向取持仓侧，同时订单方向
    // 必须取平仓方向（多平=SELL+LONG，空平=BUY+SHORT）；只改持仓方向不改
    // 订单方向会发出反向开仓或加仓单。无持仓时退回跟随方向按钮。
    // 确认弹窗展示实际方向与持仓方向。
    let mut side = app.page.form.side;
    let position_side = if market == Market::UmFutures && dual {
        match existing {
            Some(is_long) => {
                side = if is_long { Side::Sell } else { Side::Buy };
                if is_long {
                    FutPosSide::Long
                } else {
                    FutPosSide::Short
                }
            }
            None => {
                if app.page.form.side == Side::Buy {
                    FutPosSide::Long
                } else {
                    FutPosSide::Short
                }
            }
        }
    } else {
        FutPosSide::Both
    };
    // 单向模式：保护单在已有持仓时强制只减仓——不带 reduceOnly 的止损
    // 触发后会加仓/反向开仓，与"保护"语义相反（确认弹窗可见该行）
    let reduce_only = if dual {
        false
    } else {
        app.page.form.reduce_only
            || (market == Market::UmFutures && protective && existing.is_some())
    };
    Ok(OrderRequest {
        symbol,
        side,
        kind,
        tif: app.page.form.tif,
        position_side,
        reduce_only,
        close_position: false,
        // 幂等标识：同一确认弹窗重发相同 newClientOrderId，方便对账与排查重复委托
        client_id: Some(format!("btp-{}", chrono::Utc::now().timestamp_millis())),
    })
}

/// 主单 + 合约附加止盈/止损（仅单向持仓模式）：附加单为 STOP_MARKET
/// closePosition=true（触发即全平该方向持仓），方向与主单相反
pub fn build_request_bundle(app: &BnApp) -> Result<(OrderRequest, Vec<OrderRequest>), String> {
    let main = build_request(app)?;
    let state = &app.state;
    let market = app.market;
    if market != Market::UmFutures
        || !app.page.form.tpsl_on
        || state.acct.lock().fut_dual_side
        || !matches!(app.page.form.kind, UiKind::Limit | UiKind::Market)
    {
        return Ok((main, Vec::new()));
    }
    // 参考价：优先表单价格，其次最新价（触发价方向校验的基准）
    let ref_price = widgets::parse_dec(&app.page.form.price).or_else(|| {
        state
            .md
            .lock()
            .get(market, &main.symbol)
            .and_then(|md| md.ticker.as_ref().map(|t| t.last_price))
    });
    let Some(ref_price) = ref_price else {
        return Err(crate::i18n::tr("已启用止盈/止损：先填写委托价或等待行情").to_string());
    };
    let Some(r) = state.rule_of(market, &main.symbol) else {
        return Err(crate::i18n::tr("交易规则尚未加载").to_string());
    };
    let tp = widgets::parse_dec(&app.page.form.tp_price);
    let sl = widgets::parse_dec(&app.page.form.sl_price);
    if tp.is_none() && sl.is_none() {
        return Err(crate::i18n::tr("已启用止盈/止损：至少填写一个触发价").to_string());
    }
    let q = |v: Decimal| {
        crate::binance::symbols::quantize_price(v, r.tick)
            .ok_or_else(|| crate::i18n::tr("触发价超出可表示范围").to_string())
    };
    // 方向校验：多头（买入开仓）止盈在上方、止损在下方；空头相反
    let long = main.side == Side::Buy;
    let mk = |trigger: Decimal, take_profit: bool| {
        OrderRequest {
            symbol: main.symbol.clone(),
            side: if long { Side::Sell } else { Side::Buy },
            kind: OrderKind::StopMarket {
                trigger,
                qty: Decimal::ONE, // closePosition 下省略
                take_profit,
            },
            position_side: FutPosSide::Both,
            tif: Tif::Gtc,
            reduce_only: false,
            close_position: true,
            client_id: Some(format!(
                "btp-tpsl-{}",
                chrono::Utc::now().timestamp_millis()
            )),
        }
    };
    let mut extras = Vec::new();
    if let Some(tp) = tp {
        let t = q(tp)?;
        if (long && t <= ref_price) || (!long && t >= ref_price) {
            return Err(
                crate::i18n::tr("止盈触发价方向不对：多头应高于现价，空头应低于现价").to_string(),
            );
        }
        extras.push(mk(t, true));
    }
    if let Some(sl) = sl {
        let t = q(sl)?;
        if (long && t >= ref_price) || (!long && t <= ref_price) {
            return Err(
                crate::i18n::tr("止损触发价方向不对：多头应低于现价，空头应高于现价").to_string(),
            );
        }
        extras.push(mk(t, false));
    }
    Ok((main, extras))
}

/// 现货 OCO 构建：限价腿 + 止损限价腿，校验两腿价格方向
/// （卖出 OCO：限价 > 触发价；买入 OCO：限价 < 触发价）
pub fn build_oco(app: &BnApp) -> Result<crate::binance::client::OcoRequest, String> {
    let state = &app.state;
    let market = Market::Spot;
    let symbol = app.page.symbol.clone();
    let Some(r) = state.rule_of(market, &symbol) else {
        return Err(crate::i18n::tr("交易规则尚未加载").to_string());
    };
    let qty = widgets::parse_dec(&app.page.form.qty).ok_or(crate::i18n::tr("请输入数量"))?;
    let price = widgets::parse_dec(&app.page.form.price).ok_or(crate::i18n::tr("请输入价格"))?;
    let trigger =
        widgets::parse_dec(&app.page.form.trigger).ok_or(crate::i18n::tr("请输入触发价"))?;
    let stop_limit =
        widgets::parse_dec(&app.page.form.stop_limit).ok_or(crate::i18n::tr("请输入止损委托价"))?;
    let q = |v: Decimal| {
        crate::binance::symbols::quantize_price(v, r.tick)
            .ok_or_else(|| crate::i18n::tr("价格超出可表示范围").to_string())
    };
    let qp = q(price)?;
    let qtr = q(trigger)?;
    let qsl = q(stop_limit)?;
    let side = app.page.form.side;
    if side == Side::Sell && qp <= qtr {
        return Err(crate::i18n::tr("卖出 OCO：限价腿应高于止损触发价").to_string());
    }
    if side == Side::Buy && qp >= qtr {
        return Err(crate::i18n::tr("买入 OCO：限价腿应低于止损触发价").to_string());
    }
    Ok(crate::binance::client::OcoRequest {
        symbol,
        side,
        qty: crate::binance::symbols::quantize_qty(qty, r.step)
            .ok_or_else(|| crate::i18n::tr("数量超出可表示范围").to_string())?,
        price: qp,
        stop_price: qtr,
        stop_limit_price: qsl,
        client_id: Some(format!("btp-oco-{}", chrono::Utc::now().timestamp_millis())),
    })
}

/// OCO 确认弹窗行
fn oco_rows(o: &crate::binance::client::OcoRequest) -> Vec<(String, String)> {
    let tr = crate::i18n::tr;
    vec![
        (tr("交易对").to_string(), o.symbol.clone()),
        (tr("方向").to_string(), o.side.label().to_string()),
        (tr("类型").to_string(), "OCO".to_string()),
        (tr("价格").to_string(), o.price.normalize().to_string()),
        (
            tr("触发价").to_string(),
            o.stop_price.normalize().to_string(),
        ),
        (
            tr("止损委托价").to_string(),
            o.stop_limit_price.normalize().to_string(),
        ),
        (tr("数量").to_string(), o.qty.normalize().to_string()),
    ]
}

/// 百分比快捷填充
fn fill_by_pct(app: &mut BnApp, pct: f64) {
    let state = &app.state;
    let market = app.market;
    let symbol = app.page.symbol.clone();
    let Some(r) = state.rule_of(market, &symbol) else {
        return;
    };

    // 参考价：优先表单价格，其次最新价
    let ref_price = widgets::parse_dec(&app.page.form.price).or_else(|| {
        state
            .md
            .lock()
            .get(market, &symbol)
            .and_then(|md| md.ticker.as_ref().map(|t| t.last_price))
    });
    let Some(ref_price) = ref_price else { return };
    let price_f = widgets::f64_decimal(&ref_price);
    if price_f <= 0.0 {
        return;
    }

    // 余额获取 + 校验统一处理：缺失 → "加载中"，≤0 → "余额为 0"，
    // 返回 None 时直接放弃填充，避免静默填入 "0"
    let take = |f_opt: Option<f64>| -> Option<f64> {
        let Some(f) = f_opt else {
            state.push_toast(
                crate::engine::state::ToastKind::Warn,
                crate::i18n::tr("账户数据加载中").to_string(),
            );
            return None;
        };
        if f <= 0.0 {
            state.push_toast(
                crate::engine::state::ToastKind::Warn,
                crate::i18n::tr("可用余额为 0").to_string(),
            );
            return None;
        }
        Some(f)
    };

    let qty: Option<Decimal> = match (market, app.page.form.side) {
        (Market::Spot, Side::Buy) => {
            let f_opt = state.acct.lock().spot.as_ref().and_then(|a| {
                a.balances
                    .iter()
                    .find(|b| b.asset == r.quote)
                    .map(|b| widgets::f64_decimal(&b.free))
            });
            // 预留 taker 手续费（约 0.1%），避免 100% 买入因余额不足被交易所拒绝
            take(f_opt)
                .map(|f| Decimal::try_from(f * pct * 0.999 / price_f).unwrap_or(Decimal::ZERO))
        }
        (Market::Spot, Side::Sell) => {
            let f_opt = state.acct.lock().spot.as_ref().and_then(|a| {
                a.balances
                    .iter()
                    .find(|b| b.asset == r.base)
                    .map(|b| widgets::f64_decimal(&b.free))
            });
            take(f_opt).map(|f| Decimal::try_from(f * pct).unwrap_or(Decimal::ZERO))
        }
        (Market::UmFutures, _) => {
            let f_opt = state
                .acct
                .lock()
                .fut
                .as_ref()
                .map(|a| widgets::f64_decimal(&a.available));
            let lev = state
                .acct
                .lock()
                .fut_leverage
                .get(&symbol)
                .and_then(|d| d.to_string().parse::<f64>().ok())
                .unwrap_or(1.0);
            take(f_opt).map(|f| {
                Decimal::try_from(f * lev * pct * 0.999 / price_f).unwrap_or(Decimal::ZERO)
            })
        }
    };
    let Some(qty) = qty else { return };
    // 量化失败（溢出）时不填充
    if let Some(q) = crate::binance::symbols::quantize_qty(qty, r.step) {
        app.page.form.qty = q.normalize().to_string();
    }
}

/// 构建市价平仓请求并弹确认（订单页与资产页共用，防止两份实现漂移）
pub fn confirm_market_close(
    app: &mut BnApp,
    symbol: String,
    side: Side,
    qty: Decimal,
    pos_side: FutPosSide,
) {
    let state = &app.state;
    let mark = state
        .acct
        .lock()
        .fut_positions
        .iter()
        .find(|p| p.symbol == symbol)
        .map(|p| p.mark_price)
        .unwrap_or_default();
    let dual = state.acct.lock().fut_dual_side;
    // 平仓量按步进量化：与仓位原值不一致时改用量化值并在确认弹窗中提示，
    // 避免依赖引擎侧静默向下取整而留下残余仓位
    let step = state.rule_of(Market::UmFutures, &symbol).map(|r| r.step);
    let (qty, adj_row) = match step.and_then(|s| crate::binance::symbols::quantize_qty(qty, s)) {
        Some(q) if q != qty => (
            q,
            Some((
                crate::i18n::tr("数量已按步进调整").to_string(),
                format!("{} → {}", qty.normalize(), q.normalize()),
            )),
        ),
        _ => (qty, None),
    };
    let req = OrderRequest {
        symbol: symbol.clone(),
        side,
        kind: OrderKind::Market { qty },
        tif: Tif::Gtc,
        position_side: pos_side,
        reduce_only: !dual,
        close_position: false,
        client_id: Some(format!("btp-{}", chrono::Utc::now().timestamp_millis())),
    };
    // 走确认弹窗，避免误触直接发真实市价单；给出金额量级兜住输错数量的失误
    let mut rows = order_rows(&req);
    if let Some(row) = adj_row {
        rows.push(row);
    }
    if !mark.is_zero() {
        rows.push((
            crate::i18n::tr("预估金额").to_string(),
            format!("≈ {}", (qty * mark).normalize().round_dp(8)),
        ));
    }
    app.confirm = Some(ConfirmOrder {
        oco: None,
        market: Market::UmFutures,
        side: req.side,
        reqs: vec![req],
        rows,
    });
}

/// 一键平仓：全部非零持仓聚合为一个确认弹窗，确认后逐单市价平仓
pub fn confirm_close_all(app: &mut BnApp) {
    let tr = crate::i18n::tr;
    let state = &app.state;
    let (positions, dual) = {
        let a = state.acct.lock();
        (
            a.fut_positions
                .iter()
                .filter(|p| p.amt != Decimal::ZERO)
                .cloned()
                .collect::<Vec<_>>(),
            a.fut_dual_side,
        )
    };
    if positions.is_empty() {
        return;
    }
    let mut reqs = Vec::with_capacity(positions.len());
    let mut rows = Vec::new();
    let mut first_side = Side::Buy;
    for p in &positions {
        let long = p.amt > Decimal::ZERO;
        let side = if long { Side::Sell } else { Side::Buy };
        if reqs.is_empty() {
            first_side = side;
        }
        // 数量按步进量化：与仓位原值不一致时在行内标注，避免留下残余仓位
        let step = state.rule_of(Market::UmFutures, &p.symbol).map(|r| r.step);
        let qty = p.amt.abs();
        let (qty, adj) = match step.and_then(|s| crate::binance::symbols::quantize_qty(qty, s)) {
            Some(q) if q != qty => (q, true),
            _ => (qty, false),
        };
        let req = OrderRequest {
            symbol: p.symbol.clone(),
            side,
            kind: OrderKind::Market { qty },
            tif: Tif::Gtc,
            position_side: FutPosSide::parse(&p.side),
            reduce_only: !dual,
            close_position: false,
            client_id: Some(format!("btp-{}", chrono::Utc::now().timestamp_millis())),
        };
        rows.push((tr("交易对").to_string(), p.symbol.clone()));
        rows.push((
            tr("方向").to_string(),
            format!("{} {}", req.side.label(), tr("市价平仓")),
        ));
        rows.push((
            tr("数量").to_string(),
            if adj {
                format!("{}（{}）", qty.normalize(), tr("已按步进调整"))
            } else {
                qty.normalize().to_string()
            },
        ));
        if !p.mark_price.is_zero() {
            rows.push((
                tr("预估金额").to_string(),
                format!("≈ {}", (qty * p.mark_price).normalize().round_dp(8)),
            ));
        }
        reqs.push(req);
    }
    app.confirm = Some(ConfirmOrder {
        oco: None,
        market: Market::UmFutures,
        side: first_side,
        reqs,
        rows,
    });
}

/// 持仓行内 TP/SL：构建 STOP_MARKET closePosition 单（触发即市价全平该方向持仓），
/// 校验触发价方向（多头 SL < 参考价 < TP，空头相反）后进入确认弹窗。
/// 参考价取持仓标记价。返回 Err 时弹 toast，输入保留待修正
pub fn confirm_position_tpsl(
    app: &mut BnApp,
    symbol: String,
    long: bool,
    pos_side: FutPosSide,
    sl_txt: String,
    tp_txt: String,
) -> Result<(), String> {
    let tr = crate::i18n::tr;
    let state = &app.state;
    let ref_price = {
        let a = state.acct.lock();
        a.fut_positions
            .iter()
            .find(|p| p.symbol == symbol && FutPosSide::parse(&p.side) == pos_side)
            .map(|p| p.mark_price)
            .unwrap_or_default()
    };
    let Some(r) = state.rule_of(Market::UmFutures, &symbol) else {
        return Err(crate::i18n::tr("交易规则尚未加载").to_string());
    };
    let tp = widgets::parse_dec(&tp_txt);
    let sl = widgets::parse_dec(&sl_txt);
    if tp.is_none() && sl.is_none() {
        return Err(crate::i18n::tr("至少填写一个触发价").to_string());
    }
    let q = |v: Decimal| {
        crate::binance::symbols::quantize_price(v, r.tick)
            .ok_or_else(|| crate::i18n::tr("触发价超出可表示范围").to_string())
    };
    let mk = |trigger: Decimal, take_profit: bool| OrderRequest {
        symbol: symbol.clone(),
        side: if long { Side::Sell } else { Side::Buy },
        kind: OrderKind::StopMarket {
            trigger,
            qty: Decimal::ONE, // closePosition 下省略
            take_profit,
        },
        position_side: pos_side,
        tif: Tif::Gtc,
        reduce_only: false,
        close_position: true,
        client_id: Some(format!(
            "btp-tpsl-{}",
            chrono::Utc::now().timestamp_millis()
        )),
    };
    let mut reqs = Vec::new();
    let mut rows = vec![(tr("交易对").to_string(), symbol.clone())];
    if let Some(tp) = tp {
        let t = q(tp)?;
        if (long && t <= ref_price) || (!long && t >= ref_price) {
            return Err(
                crate::i18n::tr("止盈触发价方向不对：多头应高于标记价，空头应低于标记价")
                    .to_string(),
            );
        }
        rows.push((tr("止盈触发").to_string(), t.normalize().to_string()));
        reqs.push(mk(t, true));
    }
    if let Some(sl) = sl {
        let t = q(sl)?;
        if (long && t >= ref_price) || (!long && t <= ref_price) {
            return Err(
                crate::i18n::tr("止损触发价方向不对：多头应低于标记价，空头应高于标记价")
                    .to_string(),
            );
        }
        rows.push((tr("止损触发").to_string(), t.normalize().to_string()));
        reqs.push(mk(t, false));
    }
    app.confirm = Some(ConfirmOrder {
        oco: None,
        market: Market::UmFutures,
        side: reqs[0].side,
        reqs,
        rows,
    });
    Ok(())
}

/// 反手：市价平掉当前持仓并立即反向开同等数量，聚合为一个确认弹窗
/// （确认后顺序提交两张单）。双向持仓下平仓/开仓各用对应 positionSide，
/// 单向持仓均 Both、开仓单不带只减仓
pub fn confirm_market_reverse(
    app: &mut BnApp,
    symbol: String,
    long: bool,
    qty: Decimal,
    pos_side: FutPosSide,
) {
    let state = &app.state;
    let dual = state.acct.lock().fut_dual_side;
    let mark = state
        .acct
        .lock()
        .fut_positions
        .iter()
        .find(|p| p.symbol == symbol)
        .map(|p| p.mark_price)
        .unwrap_or_default();
    // 平仓量按步进量化：与仓位原值不一致时改用量化值并在确认弹窗中提示，
    // 避免依赖引擎侧静默向下取整而留下残余仓位
    let step = state.rule_of(Market::UmFutures, &symbol).map(|r| r.step);
    let (qty, adj) = match step.and_then(|s| crate::binance::symbols::quantize_qty(qty, s)) {
        Some(q) if q != qty => (q, true),
        _ => (qty, false),
    };
    let close_side = if long { Side::Sell } else { Side::Buy };
    let open_side = if long { Side::Buy } else { Side::Sell };
    let client_id = format!("btp-{}", chrono::Utc::now().timestamp_millis());
    let close = OrderRequest {
        symbol: symbol.clone(),
        side: close_side,
        kind: OrderKind::Market { qty },
        tif: Tif::Gtc,
        position_side: pos_side,
        reduce_only: !dual,
        close_position: false,
        client_id: Some(client_id.clone()),
    };
    let open = OrderRequest {
        symbol: symbol.clone(),
        side: open_side,
        kind: OrderKind::Market { qty },
        tif: Tif::Gtc,
        position_side: if dual {
            if long {
                FutPosSide::Short
            } else {
                FutPosSide::Long
            }
        } else {
            FutPosSide::Both
        },
        reduce_only: false,
        close_position: false,
        client_id: Some(client_id),
    };
    let tr = crate::i18n::tr;
    let mut rows = vec![(tr("交易对").to_string(), symbol.clone())];
    rows.push((
        tr("方向").to_string(),
        format!(
            "{} {} → {} {}",
            close.side.label(),
            tr("平仓"),
            open.side.label(),
            tr("开仓")
        ),
    ));
    rows.push((
        tr("数量").to_string(),
        if adj {
            format!("{}（{}）", qty.normalize(), tr("已按步进调整"))
        } else {
            qty.normalize().to_string()
        },
    ));
    if !mark.is_zero() {
        rows.push((
            tr("预估金额").to_string(),
            format!("≈ {}", (qty * mark).normalize().round_dp(8)),
        ));
    }
    app.confirm = Some(ConfirmOrder {
        oco: None,
        market: Market::UmFutures,
        side: close_side,
        reqs: vec![close, open],
        rows,
    });
}

pub fn order_rows(req: &OrderRequest) -> Vec<(String, String)> {
    let tr = crate::i18n::tr;
    let mut rows = vec![
        (tr("交易对").to_string(), req.symbol.clone()),
        (tr("方向").to_string(), req.side.label().to_string()),
        (tr("类型").to_string(), req.kind.label().to_string()),
        (
            tr("数量").to_string(),
            req.kind.qty().normalize().to_string(),
        ),
    ];
    match &req.kind {
        OrderKind::Limit { price, .. } => {
            rows.push((tr("价格").to_string(), price.normalize().to_string()))
        }
        OrderKind::Market { .. } => {}
        OrderKind::StopLimit { trigger, price, .. } => {
            rows.push((tr("触发价").into(), trigger.normalize().to_string()));
            rows.push((tr("委托价").into(), price.normalize().to_string()));
        }
        OrderKind::StopMarket { trigger, .. } => {
            rows.push((tr("触发价").into(), trigger.normalize().to_string()));
        }
        OrderKind::TrailingStop { callback_rate, .. } => {
            rows.push((
                tr("回调幅度").into(),
                format!("{}%", callback_rate.normalize()),
            ));
        }
    }
    if req.reduce_only {
        rows.push((tr("只减仓").into(), tr("是").into()));
    }
    if req.position_side != FutPosSide::Both {
        rows.push((tr("持仓方向").into(), req.position_side.label().into()));
    }
    rows
}

// ============================================================
// 里程碑 7：表单 UI（GPUI）
// ============================================================

/// 下单表单输入框集合（GPUI TextField 是独立 Entity，文本状态在框内）。
/// 现货/期货页共用一套：切页时由渲染帧把活动页表单字符串同步进框。
/// OrderForm 的 String 仍是单一数据源（build_request 族读字符串），
/// on_change 写回、渲染帧回灌，双向一致性由"谁改谁通知"保证
#[derive(Clone)]
pub struct TradeFields {
    pub price: Entity<TextField>,
    pub qty: Entity<TextField>,
    pub trigger: Entity<TextField>,
    pub stop_limit: Entity<TextField>,
    pub callback_rate: Entity<TextField>,
    pub tp: Entity<TextField>,
    pub sl: Entity<TextField>,
    pub leverage: Entity<TextField>,
}

impl TradeFields {
    pub fn new(cx: &mut Context<BnApp>) -> Self {
        let mk = |cx: &mut Context<BnApp>, id: &'static str, ph: &'static str| {
            cx.new(|c| {
                let mut f = TextField::new(c);
                // 数字过滤：与 Decimal::from_str_exact 的合法字符对齐，
                // 粘贴/IME 注入同样被过滤
                f.set_id(id)
                    .set_numeric(true)
                    .set_placeholder(ph)
                    .set_font_size(theme::font_md());
                f
            })
        };
        let tr = crate::i18n::tr;
        Self {
            price: mk(cx, "price_in", tr("价格")),
            qty: mk(cx, "qty_in", tr("数量")),
            trigger: mk(cx, "trig_in", tr("触发价")),
            stop_limit: mk(cx, "sl_limit_in", tr("委托价")),
            callback_rate: mk(cx, "cb_rate_in", tr("百分比")),
            tp: mk(cx, "tp_in", tr("触发价")),
            sl: mk(cx, "sl_in", tr("触发价")),
            leverage: mk(cx, "lev_in", tr("杠杆倍数")),
        }
    }

    /// 挂接写回回调（BnApp::new 里实体创建后调用：回调需要 WeakEntity 句柄）。
    /// 写回目标是 app.page.form.*：字段只在其所属页为活动页时才渲染/接收输入，
    /// 回调触发时 app.page 即所有者页
    pub fn attach(&mut self, weak: &WeakEntity<BnApp>, cx: &mut Context<BnApp>) {
        macro_rules! bind {
            ($e:expr, $set:expr) => {
                $e.update(cx, |tf, _| {
                    let weak = weak.clone();
                    tf.set_on_change(Box::new(move |s, _, cx| {
                        let s = s.to_string();
                        let _ = weak.update(cx, |app: &mut BnApp, _| $set(app, s));
                    }));
                });
            };
        }
        bind!(self.price, |app: &mut BnApp, v: String| app.page.form.price = v);
        bind!(self.qty, |app: &mut BnApp, v: String| app.page.form.qty = v);
        bind!(self.trigger, |app: &mut BnApp, v: String| app.page.form.trigger = v);
        bind!(self.stop_limit, |app: &mut BnApp, v: String| app.page.form.stop_limit = v);
        bind!(self.callback_rate, |app: &mut BnApp, v: String| app
            .page
            .form
            .callback_rate = v);
        bind!(self.tp, |app: &mut BnApp, v: String| app.page.form.tp_price = v);
        bind!(self.sl, |app: &mut BnApp, v: String| app.page.form.sl_price = v);
        bind!(self.leverage, |app: &mut BnApp, v: String| app.page.leverage_input = v);
    }

    /// 外部变更回灌：文本不同才 set（正在输入的框两侧已一致，不会重置光标）
    fn sync(e: &Entity<TextField>, val: &str, cx: &mut Context<BnApp>) {
        e.update(cx, |f, _| {
            if f.text() != val {
                f.set_text(val);
            }
        });
    }

    pub fn sync_all(&self, app: &BnApp, cx: &mut Context<BnApp>) {
        let f = &app.page.form;
        Self::sync(&self.price, &f.price, cx);
        Self::sync(&self.qty, &f.qty, cx);
        Self::sync(&self.trigger, &f.trigger, cx);
        Self::sync(&self.stop_limit, &f.stop_limit, cx);
        Self::sync(&self.callback_rate, &f.callback_rate, cx);
        Self::sync(&self.tp, &f.tp_price, cx);
        Self::sync(&self.sl, &f.sl_price, cx);
        Self::sync(&self.leverage, &app.page.leverage_input, cx);
    }
}

/// 表单行：行高 22（egui 版 compact_row 语义），标签列固定 48pt 容下
/// "止损限价"四字，输入框左缘全行对齐
fn row(label: &'static str) -> Div {
    let tr = crate::i18n::tr;
    div()
        .h(px(22.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .w(px(48.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .overflow_hidden()
                .child(widgets::card_label(tr(label))),
        )
}

/// 买/卖切换大按钮：选中侧涨/跌色实心 + on_fill 文字，未选中 btn_idle 底。
/// （egui 版选中色有 animate_bool 渐变；GPUI 与胶囊页签同理由改为静态填充）
fn side_btn(on: bool, side: Side, market: Market, tag: &'static str) -> Stateful<Div> {
    let t = cur();
    let color = if side == Side::Buy { t.up } else { t.down };
    let tr = crate::i18n::tr;
    let txt = match (side, market) {
        (Side::Buy, Market::UmFutures) => tr("买入 / 开多"),
        (Side::Sell, Market::UmFutures) => tr("卖出 / 开空"),
        (Side::Buy, _) => tr("买入"),
        (Side::Sell, _) => tr("卖出"),
    };
    div()
        .id(tag)
        .flex_1()
        .h(px(theme::BTN_H))
        .rounded(px(theme::R_MD))
        .flex()
        .items_center()
        .justify_center()
        .bg(if on { color } else { t.btn_idle })
        .text_size(font_md())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if on { t.on_fill } else { t.text })
        .whitespace_nowrap()
        .cursor_pointer()
        .when(!on, |d| d.hover(|s| s.bg(t.row_hover)))
        .child(txt)
}

/// 小复选框：14×14 方框，选中 accent 填充 + ✓。egui Checkbox 对应物
fn checkbox(tag: &'static str, checked: bool, enabled: bool) -> Stateful<Div> {
    let t = cur();
    div()
        .id(tag)
        .w(px(14.0))
        .h(px(14.0))
        .rounded(px(3.0))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if checked { t.accent } else { t.text_dim })
        .text_size(px(10.0))
        .text_color(t.on_accent)
        .when(checked, |d| d.bg(t.accent).child("✓"))
        .when(!enabled, |d| d.opacity(0.5))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.border_color(t.accent)))
}

/// 价格行一键填值按钮可见性：买一 / 卖一 / 最新。
/// 窄行只显示"最新"，买一/卖一随行内控件区宽度依次展开，保证输入框
/// 不被挤没（egui 版原样迁移）
fn best_px_vis(
    inner_w: f32,
    bid: Option<Decimal>,
    ask: Option<Decimal>,
    _last: Option<Decimal>,
) -> (bool, bool, bool) {
    let mut show_mid = inner_w > 150.0;
    let show_ask = inner_w > 200.0 && ask.is_some();
    let show_bid = inner_w > 250.0 && bid.is_some();
    if !show_ask && !show_bid {
        show_mid = true;
    }
    (show_bid, show_ask, show_mid)
}

/// 下单面板：买/卖、类型下拉、有效期、价格/数量、附加止盈止损、比例填充、提交。
/// 由 app.rs 右栏容器承载（宽度 trade_width 300~360），内容紧凑无滚动
pub fn panel(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let state = app.state.clone();
    let market = app.market;
    let symbol = app.page.symbol.clone();
    let rule = state.rule_of(market, &symbol);
    let t = cur();
    let tr = crate::i18n::tr;

    // 外部变更回灌（盘口点击填价/比例填充/切交易对清空后，下一帧同步进框）
    let fields = app.trade_fields.clone();
    fields.sync_all(app, cx);

    // ---------- 行内控件区宽度（一键填价按钮可见性阈值） ----------
    // 面板内容宽 - 卡片内边距 20 - 标签列 48 - 行内间距 8
    let row_inner = (app.trade_width - 76.0).max(0.0);
    let (bid_px, ask_px, last_px) = {
        let md = state.md.lock();
        md.get(market, &symbol).map_or((None, None, None), |m| {
            (
                m.depth.bids.first().map(|l| l.price),
                m.depth.asks.first().map(|l| l.price),
                m.ticker.as_ref().map(|tk| tk.last_price),
            )
        })
    };
    let px_vis = best_px_vis(row_inner, bid_px, ask_px, last_px);

    // ---------- 根容器：未配置密钥提示 + 买卖切换 + 类型卡片 + 提交 ----------
    let mut root = div()
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(3.0))
        // 点击面板任意空白处收起订单类型下拉
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|app: &mut BnApp, _, _, cx| {
                if app.kind_menu_open {
                    app.kind_menu_open = false;
                    cx.notify();
                }
            }),
        );

    if !state.has_creds() {
        root = root.child(
            div()
                .text_size(font_sm())
                .text_color(t.warn)
                .child(tr("未配置 API 密钥，仅可查看行情").to_string()),
        );
    }

    // ---------- 买/卖切换 ----------
    // 双向持仓模式下买卖即决定开多/开空，持仓方向随方向按钮联动（见 build_request）
    root = root.child(
        div()
            .w_full()
            .pb(px(2.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                side_btn(app.page.form.side == Side::Buy, Side::Buy, market, "side_buy")
                    .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                        app.page.form.side = Side::Buy;
                        cx.notify();
                    })),
            )
            .child(
                side_btn(
                    app.page.form.side == Side::Sell,
                    Side::Sell,
                    market,
                    "side_sell",
                )
                .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                    app.page.form.side = Side::Sell;
                    cx.notify();
                })),
            ),
    );

    // ---------- 订单类型 + 价格数量（合并为一张紧凑卡片） ----------
    let kinds = UiKind::available(market);
    let cur_kind = if kinds.contains(&app.page.form.kind) {
        app.page.form.kind
    } else {
        kinds[0]
    };
    let mut card = widgets::card()
        .px(px(10.0))
        .py(px(6.0))
        .my(px(2.0))
        .flex()
        .flex_col()
        .gap(px(3.0));

    // 类型行：下拉按钮（菜单覆盖层在 root 末尾统一渲染，保证盖住后续行）
    let mut kind_row = row("类型");
    kind_row = kind_row.child(
        widgets::btn_ghost("order_kind", format!("{} ▾", cur_kind.label()), true)
            // on_mouse_down 而非 on_click：先于根容器的"点空白收起"触发，
            // 并吃掉事件，避免关闭又立刻重开
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app: &mut BnApp, _, _, cx| {
                    app.kind_menu_open = !app.kind_menu_open;
                    cx.notify();
                    cx.stop_propagation();
                }),
            ),
    );
    card = card.child(kind_row);

    // 有效期（限价类订单；GTX 仅合约 = 只做 maker）
    if matches!(
        app.page.form.kind,
        UiKind::Limit | UiKind::StopLossLimit | UiKind::TakeProfitLimit
    ) {
        let tifs = Tif::available(market);
        let mut tif_row = row("有效期");
        for (i, &tif) in tifs.iter().enumerate() {
            let on = app.page.form.tif == tif;
            let tip = match tif {
                Tif::Gtc => tr("成交为止"),
                Tif::Ioc => tr("立即成交，剩余撤销"),
                Tif::Fok => tr("全部成交或立即撤销"),
                Tif::Gtx => tr("只做 maker，不会吃单"),
            };
            tif_row = tif_row.child(
                div()
                    .id(ElementId::from(("tif", i)))
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(18.0))
                    .rounded(px(theme::R_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(font_sm())
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_color(if on { t.accent } else { t.text_dim })
                    .when(on, |d| d.bg(faded(t.accent, 0.16)))
                    .when(!on, |d| d.hover(|s| s.bg(t.row_hover)))
                    .cursor_pointer()
                    .child(tif.as_str())
                    .tooltip(move |_, cx| widgets::tooltip_view(tip.to_string(), cx))
                    .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                        app.page.form.tif = tif;
                        cx.notify();
                    })),
            );
        }
        card = card.child(tif_row);
    }

    // ---------- 合约持仓方向 / 只减仓 / 杠杆 ----------
    if market == Market::UmFutures {
        let dual = state.acct.lock().fut_dual_side;
        if dual {
            // 双向模式：持仓方向跟随买/卖（买入=开多 LONG，卖出=开空 SHORT），
            // 避免方向按钮与持仓侧独立选择导致 "SELL+LONG" 平多误操作
            let pos_txt = if app.page.form.side == Side::Buy {
                tr("开多 (LONG)")
            } else {
                tr("开空 (SHORT)")
            };
            let dir_color = if app.page.form.side == Side::Buy {
                theme::up_color()
            } else {
                theme::down_color()
            };
            let mut r = row("持仓方向");
            r = r.child(
                div()
                    .text_size(font_md())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(dir_color)
                    .child(pos_txt.to_string()),
            );
            card = card.child(r);
        }
        if !dual {
            let ro = app.page.form.reduce_only;
            card = card.child(
                row("只减仓").child(
                    checkbox("reduce_only", ro, true).on_click(cx.listener(
                        |app: &mut BnApp, _, _, cx| {
                            app.page.form.reduce_only = !app.page.form.reduce_only;
                            cx.notify();
                        },
                    )),
                ),
            );
        }
        // 杠杆：倍数、保证金模式徽章、倍数输入与"设定"收进同一行
        let lev = state
            .acct
            .lock()
            .fut_leverage
            .get(&symbol)
            .and_then(|d| d.to_string().parse::<u32>().ok())
            .unwrap_or(0);
        let mt = state.acct.lock().fut_margin_type.get(&symbol).cloned();
        let mut lev_row = row("杠杆");
        if lev > 0 {
            lev_row = lev_row.child(
                div()
                    .text_size(font_md())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{lev}x")),
            );
        }
        if let Some(mt) = mt {
            let isolated = mt == "ISOLATED";
            let txt = if isolated {
                tr("逐仓")
            } else {
                tr("全仓")
            };
            lev_row = lev_row.child(
                widgets::btn_ghost("margin_type", txt, true)
                    .tooltip(move |_, cx| {
                        widgets::tooltip_view(tr("点击切换保证金模式（全仓/逐仓）").to_string(), cx)
                    })
                    .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                        app.state.send_cmd(Command::SetMarginType {
                            symbol: app.page.symbol.clone(),
                            isolated: !isolated,
                        });
                        cx.notify();
                    })),
            );
        }
        lev_row = lev_row.child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .child(app.trade_fields.leverage.clone()),
        );
        lev_row = lev_row.child(
            widgets::btn_ghost("lev_set", tr("设定"), true).on_click(cx.listener(
                move |app: &mut BnApp, _, _, cx| {
                    match app.page.leverage_input.trim().parse::<u32>() {
                        Ok(l) if (1..=125).contains(&l) => {
                            app.state.send_cmd(Command::SetLeverage {
                                symbol: app.page.symbol.clone(),
                                leverage: l,
                            });
                        }
                        _ => {
                            app.state.push_toast(
                                crate::engine::state::ToastKind::Warn,
                                tr("杠杆倍数需为 1-125 的整数").to_string(),
                            );
                        }
                    }
                    cx.notify();
                },
            )),
        );
        card = card.child(lev_row);
    }

    // 价格（市价与跟踪委托不需要委托价）
    if !matches!(app.page.form.kind, UiKind::Market | UiKind::TrailingStop) {
        let mut price_row = row("价格");
        price_row = price_row.child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .child(app.trade_fields.price.clone()),
        );
        // 一键填价按钮组（可见性按行宽折叠，与 egui 版同一套阈值）。
        // 捕获闭包借用 price_row 再回填：编译器不允许，改为直接顺序追加
        if px_vis.0 {
            if let Some(p) = bid_px {
                price_row = price_row.child(
                    widgets::btn_ghost("px_bid", tr("买一"), true)
                        .tooltip(move |_, cx| {
                            widgets::tooltip_view(tr("点击填入下单价格").to_string(), cx)
                        })
                        .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                            app.page.form.price = p.normalize().to_string();
                            cx.notify();
                        })),
                );
            }
        }
        if px_vis.1 {
            if let Some(p) = ask_px {
                price_row = price_row.child(
                    widgets::btn_ghost("px_ask", tr("卖一"), true)
                        .tooltip(move |_, cx| {
                            widgets::tooltip_view(tr("点击填入下单价格").to_string(), cx)
                        })
                        .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                            app.page.form.price = p.normalize().to_string();
                            cx.notify();
                        })),
                );
            }
        }
        if px_vis.2 {
            if let Some(p) = last_px {
                price_row = price_row.child(
                    widgets::btn_ghost("px_last", tr("最新"), true)
                        .tooltip(move |_, cx| {
                            widgets::tooltip_view(tr("点击填入下单价格").to_string(), cx)
                        })
                        .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                            app.page.form.price = p.normalize().to_string();
                            cx.notify();
                        })),
                );
            }
        }
        card = card.child(price_row);
    }

    // 触发价（止损/止盈限价/市价与 OCO）
    if matches!(
        app.page.form.kind,
        UiKind::StopLossLimit
            | UiKind::TakeProfitLimit
            | UiKind::StopLossMarket
            | UiKind::TakeProfitMarket
            | UiKind::Oco
    ) {
        card = card.child(
            row("触发价").child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(app.trade_fields.trigger.clone()),
            ),
        );
    }
    // OCO 止损腿限价（触发后挂出的限价单）
    if app.page.form.kind == UiKind::Oco {
        card = card.child(
            row("止损限价").child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(app.trade_fields.stop_limit.clone()),
            ),
        );
    }
    // 数量行：步进标签贴行右缘
    let step_txt = rule
        .as_ref()
        .map(|r| format!("{} {}", tr("步进"), r.step.normalize()))
        .unwrap_or_default();
    let mut qty_row = row("数量");
    qty_row = qty_row.child(
        div()
            .flex_1()
            .min_w(px(0.0))
            .child(app.trade_fields.qty.clone()),
    );
    if !step_txt.is_empty() {
        qty_row = qty_row.child(
            div()
                .text_size(font_sm())
                .text_color(t.text_dim)
                .whitespace_nowrap()
                .overflow_hidden()
                .child(step_txt),
        );
    }
    card = card.child(qty_row);

    // 跟踪委托：回调幅度（百分数，价格回撤该幅度触发市价全平）
    if app.page.form.kind == UiKind::TrailingStop {
        card = card.child(
            row("回调幅度")
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(app.trade_fields.callback_rate.clone()),
                )
                .child(
                    div()
                        .text_size(font_sm())
                        .text_color(t.text_dim)
                        .whitespace_nowrap()
                        .child(tr("%（0.1-10）").to_string()),
                ),
        );
    }

    // ---------- 附加止盈/止损（合约单向持仓模式，触发即全平） ----------
    if market == Market::UmFutures && matches!(app.page.form.kind, UiKind::Limit | UiKind::Market)
    {
        let dual = state.acct.lock().fut_dual_side;
        let enabled = !dual;
        let tpsl_on = app.page.form.tpsl_on;
        card = card.child(
            row("止盈/止损").child(
                checkbox("tpsl_on", tpsl_on, enabled)
                    .tooltip(move |_, cx| {
                        let tip = if enabled {
                            tr("下单同时挂出触发即全平的止盈/止损单").to_string()
                        } else {
                            tr("仅单向持仓模式可用（双向模式请用持仓页设置）").to_string()
                        };
                        widgets::tooltip_view(tip, cx)
                    })
                    .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                        if enabled {
                            app.page.form.tpsl_on = !app.page.form.tpsl_on;
                            cx.notify();
                        }
                    })),
            ),
        );
        if app.page.form.tpsl_on {
            card = card.child(
                row("止盈触发").child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(app.trade_fields.tp.clone()),
                ),
            );
            card = card.child(
                row("止损触发").child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(app.trade_fields.sl.clone()),
                ),
            );
        }
    }

    // ---------- 百分比快捷填充 ----------
    let mut pct_row = row("比例");
    for (i, pct) in [25.0, 50.0, 75.0, 100.0].into_iter().enumerate() {
        pct_row = pct_row.child(
            widgets::btn_ghost(ElementId::from(("pct", i)), format!("{}%", pct as i32), true)
                .tooltip(move |_, cx| widgets::tooltip_view(tr("按可用余额估算").to_string(), cx))
                .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                    fill_by_pct(app, pct / 100.0);
                    cx.notify();
                })),
        );
    }
    card = card.child(pct_row);
    root = root.child(card);

    // ---------- 汇总与量化预览 ----------
    let (qty_dec, price_dec) = (
        widgets::parse_dec(&app.page.form.qty),
        widgets::parse_dec(&app.page.form.price),
    );
    let mut total: Option<Decimal> = None;
    // 限价单严格要求表单价格；其余类型未填委托价时退回最新价（市价单始终如此）。
    // checked_mul 防超大输入导致 rust_decimal 溢出 panic
    let ref_px: Option<Decimal> = match app.page.form.kind {
        UiKind::Limit => price_dec,
        _ => price_dec.or_else(|| {
            state
                .md
                .lock()
                .get(market, &symbol)
                .and_then(|md| md.ticker.as_ref().map(|tk| tk.last_price))
        }),
    };
    if let (Some(q), Some(p)) = (qty_dec, ref_px) {
        total = q.checked_mul(p);
    }
    // 预估金额：紧凑单行小字
    let total_val = div()
        .text_size(font_sm())
        .font_family(theme::FONT_MONO)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(match total {
            Some(t) => t.normalize().round_dp(8).to_string().into(),
            None => SharedString::from("—"),
        });
    root = root.child(
        div()
            .h(px(18.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(font_sm())
                    .text_color(t.text_dim)
                    .child(tr("预估金额").to_string()),
            )
            .child(total_val),
    );
    // 量化预览提示
    if let Some(r) = &rule {
        let mut hints: Vec<String> = Vec::new();
        if let Some(p) = price_dec {
            if let Some(qp) = crate::binance::symbols::quantize_price(p, r.tick) {
                if qp != p {
                    hints.push(format!("{} {}", tr("价格将量化为"), qp.normalize()));
                }
            } else {
                hints.push(tr("价格超出可表示范围").to_string());
            }
        }
        if let Some(q) = qty_dec {
            match crate::binance::symbols::quantize_qty(q, r.step) {
                Some(qq) => {
                    if qq != q {
                        hints.push(format!("{} {}", tr("数量将量化为"), qq.normalize()));
                    }
                    if !r.min_qty.is_zero() && qq < r.min_qty {
                        hints.push(format!("{} {}", tr("小于最小下单数量"), r.min_qty.normalize()));
                    }
                    if let Some(tp) = total {
                        if !r.min_notional.is_zero() && tp < r.min_notional {
                            hints.push(format!(
                                "{}{}{}",
                                tr("低于最小名义金额"),
                                r.min_notional.normalize(),
                                tr("（限价单）")
                            ));
                        }
                    }
                }
                None => hints.push(tr("数量超出可表示范围").to_string()),
            }
        }
        if !hints.is_empty() {
            root = root.child(
                div()
                    .text_size(font_sm())
                    .text_color(t.warn)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(hints.join("；")),
            );
        }
    }

    // ---------- 提交 ----------
    let busy = state.is_busy("place");
    // 现货 OCO 走独立构建器（占位主单仅用于按钮启用判定，不会发送）
    let is_oco = market == Market::Spot && app.page.form.kind == UiKind::Oco;
    let check_oco = if is_oco { Some(build_oco(app)) } else { None };
    // 提交前校验：错误以红色提示条显示在按钮上方（输入时实时清红条，
    // 避免错误残留造成"界面在骂人"），真正的下单仍以确认弹窗兜底
    let check = if is_oco { None } else { Some(build_request_bundle(app)) };
    let err_now: Option<&String> = match (&check_oco, &check) {
        (Some(Err(e)), _) | (_, Some(Err(e))) => Some(e),
        _ => None,
    };
    let check_ok = err_now.is_none() && (check_oco.is_some() || check.is_some());
    if let Some(e) = err_now {
        if app.page.form.err != *e {
            app.page.form.err = e.clone();
        }
    }
    let btn_enabled = !busy && state.has_creds() && check_ok;
    if btn_enabled && !app.page.form.err.is_empty() {
        app.page.form.err.clear();
    }
    // 校验错误红条：err 语义色淡填充（down 色留给行情涨跌，错误与"跌"不是一个概念）
    if !app.page.form.err.is_empty() {
        root = root.child(
            div()
                .rounded(px(theme::R_SM))
                .px(px(8.0))
                .py(px(3.0))
                .bg(faded(t.err, 0.16))
                // 字号与输入框占位一致：错误条与输入框相邻出现时文字高低统一
                .child(
                    div()
                        .text_size(font_md())
                        .text_color(t.err)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(app.page.form.err.clone()),
                ),
        );
    }
    // 提交按钮文案：交易对买卖的标的其实是 base 资产（BTCUSDT 买的是 BTC），
    // 按钮文案用 base；规则未加载时退回交易对原文
    let (word, color) = match (app.page.form.side, market) {
        (Side::Buy, Market::Spot) => (tr("买入"), t.up),
        (Side::Sell, Market::Spot) => (tr("卖出"), t.down),
        (Side::Buy, Market::UmFutures) => (tr("买入开多"), t.up),
        (Side::Sell, Market::UmFutures) => (tr("卖出开空"), t.down),
    };
    let base = rule
        .as_ref()
        .map(|r| r.base.clone())
        .unwrap_or_else(|| symbol.clone());
    root = root.child(
        div()
            .id("order_submit")
            .w_full()
            .h(px(30.0))
            .rounded(px(theme::R_MD))
            .flex()
            .items_center()
            .justify_center()
            .bg(color)
            .text_size(font_body())
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.on_fill)
            .whitespace_nowrap()
            .overflow_hidden()
            .child(format!("{word} {base}"))
            .when(!btn_enabled, |d| d.opacity(0.5))
            .when(btn_enabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(theme::lerp_color(color, t.text, 0.12)))
            })
            .when(btn_enabled, |d| {
                d.on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                    submit_form(app, cx);
                }))
            }),
    );
    if busy {
        root = root.child(
            div()
                .text_size(font_sm())
                .text_color(t.text_dim)
                .child(tr("提交中…").to_string()),
        );
    }

    // ---------- 订单类型下拉菜单（覆盖层最后渲染，盖住表单行） ----------
    if app.kind_menu_open {
        // 位置：买/卖行(28+pb2) + 卡片 my2/py6 ≈ 类型行顶 40，菜单自按钮下缘展开
        let menu = div()
            .absolute()
            .top(px(62.0))
            .left(px(56.0))
            .w(px(96.0))
            .py(px(4.0))
            .rounded(px(theme::R_MD))
            .bg(t.card)
            .border_1()
            .border_color(faded(t.border, 0.7))
            .shadow_md();
        let menu = kinds.into_iter().enumerate().fold(menu, |m, (i, k)| {
            let selected = app.page.form.kind == k;
            m.child(
                div()
                    .id(ElementId::from(("order_kind_item", i)))
                    .px(px(10.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .text_size(font_sm())
                    .whitespace_nowrap()
                    .text_color(if selected { t.accent } else { t.text })
                    .when(selected, |d| d.bg(faded(t.accent, 0.12)))
                    .hover(|s| s.bg(t.row_hover))
                    .cursor_pointer()
                    .child(k.label())
                    // on_mouse_down 选中：先于根容器"点空白收起"，吃掉事件
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |app: &mut BnApp, _, _, cx| {
                            app.page.form.kind = k;
                            app.kind_menu_open = false;
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    ),
            )
        });
        root = root.child(menu);
    }

    root
}

/// 提交按钮点击：重新校验（监听器闭包不能消费渲染期构建的 Result，
/// 就地重建同样廉价），弹确认框
fn submit_form(app: &mut BnApp, cx: &mut Context<BnApp>) {
    let market = app.market;
    let is_oco = market == Market::Spot && app.page.form.kind == UiKind::Oco;
    if is_oco {
        if let Ok(oco) = build_oco(app) {
            let rows = oco_rows(&oco);
            app.confirm = Some(ConfirmOrder {
                market: Market::Spot,
                oco: Some(oco.clone()),
                reqs: Vec::new(),
                side: oco.side,
                rows,
            });
        }
    } else if let Ok((main, extras)) = build_request_bundle(app) {
        let mut rows = order_rows(&main);
        // 确认弹窗给出金额量级，兜住市价单输错数量的低级失误
        let (qty_dec, price_dec) = (
            widgets::parse_dec(&app.page.form.qty),
            widgets::parse_dec(&app.page.form.price),
        );
        let ref_px: Option<Decimal> = match app.page.form.kind {
            UiKind::Limit => price_dec,
            _ => price_dec.or_else(|| {
                app.state
                    .md
                    .lock()
                    .get(market, &app.page.symbol)
                    .and_then(|md| md.ticker.as_ref().map(|tk| tk.last_price))
            }),
        };
        if let (Some(q), Some(p)) = (qty_dec, ref_px) {
            if let Some(t) = q.checked_mul(p) {
                rows.push((
                    crate::i18n::tr("预估金额").to_string(),
                    format!("≈ {}", t.normalize().round_dp(8)),
                ));
            }
        }
        // 附加止盈/止损单入列（主单在前，触发即全平）
        for e in &extras {
            if let OrderKind::StopMarket {
                trigger,
                take_profit,
                ..
            } = &e.kind
            {
                rows.push((
                    if *take_profit {
                        crate::i18n::tr("附加止盈").to_string()
                    } else {
                        crate::i18n::tr("附加止损").to_string()
                    },
                    format!("{} {}", crate::i18n::tr("触发"), trigger.normalize()),
                ));
            }
        }
        // 弹窗头部的买卖方向必须与实际请求一致：
        // 对冲保护单会把 side 翻转为平仓方向（如 LONG→SELL+LONG）
        let side = main.side;
        let mut reqs = vec![main];
        reqs.extend(extras);
        app.confirm = Some(ConfirmOrder {
            market: app.market,
            oco: None,
            reqs,
            rows,
            side,
        });
    }
    cx.notify();
}

/// 确认下单弹窗：全屏遮罩 + 居中卡片。返回 None 时不渲染。
/// 提交键按方向着色（买=涨/卖=跌），busy 时禁用防双击双单；
/// Esc/点遮罩/取消键均可退出（真实资金操作必须有键盘退出路径）
pub fn confirm_dialog(app: &mut BnApp, cx: &mut Context<BnApp>) -> Option<Div> {
    let c = app.confirm.as_ref()?;
    let t = cur();
    let tr = crate::i18n::tr;
    // 弹窗描边随方向着色：买=涨色 / 卖=跌色，色即语义（与提交键同源）
    let side_color = if c.side == Side::Buy { t.up } else { t.down };
    let color = side_color;
    let word = if c.side == Side::Buy {
        tr("买入")
    } else {
        tr("卖出")
    };
    let testnet = app.state.cfg.read().testnet;
    let header = if testnet {
        format!("{}{}{}", tr("【测试网】"), word, tr("确认"))
    } else {
        format!("{}{} · {}", tr("【实盘】"), word, tr("确认 · 真实资金！"))
    };
    let placing = app.state.is_busy("place");
    let rows = c.rows.clone();

    // 行值列等宽字体 + 方向色：数字一眼可辨（egui 版 Grid + monospace 同语义）
    let mut rows_col = div().w_full().flex().flex_col().gap(px(8.0));
    for (k, v) in rows {
        rows_col = rows_col.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(16.0))
                .child(
                    div()
                        .w(px(88.0))
                        .flex_shrink_0()
                        .text_size(font_body())
                        .text_color(t.text_dim)
                        .child(k),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .font_family(theme::FONT_MONO)
                        .text_size(font_body())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(v),
                ),
        );
    }

    let (submit_txt, submit_fill) = if c.side == Side::Buy {
        (tr("确认买入"), t.up)
    } else {
        (tr("确认卖出"), t.down)
    };

    let card = div()
        .w(px(360.0))
        .max_w(px(480.0))
        .rounded(px(theme::R_LG))
        .bg(t.panel2)
        .border_1()
        .border_color(faded(side_color, 0.55))
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        // 警示行是弹窗真正的信息核心，层级必须压过装饰性标题
        .child(
            div()
                .text_size(theme::font_lg())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if testnet { t.accent } else { t.err })
                .whitespace_nowrap()
                .overflow_hidden()
                .child(header),
        )
        .child(widgets::separator())
        .child(rows_col)
        .child(
            div()
                .pt(px(4.0))
                .flex()
                .flex_row()
                .justify_end()
                .items_center()
                .gap(px(8.0))
                .child(
                    widgets::btn_default("confirm_cancel", tr("取消"), true)
                        // 初始焦点语义（egui 版取消键默认聚焦走安全路径）在
                        // GPUI 下由 Esc 承担：键盘退出路径恒可用
                        .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                            app.confirm = None;
                            cx.notify();
                        })),
                )
                .child(
                    widgets::btn_solid("confirm_submit", submit_txt, submit_fill, !placing)
                        .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                            if app.state.is_busy("place") {
                                return;
                            }
                            let Some(c) = app.confirm.take() else {
                                return;
                            };
                            // 入队即置忙（引擎负责释放）：覆盖"下单命令排队在被
                            // 卡住的刷新之后"的窗口，期间提交按钮禁用，防止双击双单。
                            // OCO 单条命令提交；多单（主单+附加止盈止损）顺序入队。
                            // send_cmd 失败时（引擎命令循环已退出）由它统一报错，
                            // 这里只需回滚 busy，否则提交键永久禁用
                            app.state.set_busy("place", true);
                            let mut ok = true;
                            if let Some(oco) = c.oco {
                                ok &= app.state.send_cmd(Command::PlaceOco { req: oco });
                            } else {
                                for req in c.reqs {
                                    ok &= app.state.send_cmd(Command::PlaceOrder {
                                        market: c.market,
                                        req,
                                    });
                                }
                            }
                            if !ok {
                                app.state.set_busy("place", false);
                            }
                            cx.notify();
                        })),
                ),
        );

    // 全屏遮罩：视觉聚焦弹窗 + 点击空白即取消（与 Esc 等效）
    let dim_alpha = if theme::is_light() { 0.27 } else { 0.47 };
    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("confirm_dim")
                    .absolute()
                    .inset_0()
                    .bg(gpui::hsla(0.0, 0.0, 0.0, dim_alpha))
                    .cursor_pointer()
                    .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                        app.confirm = None;
                        cx.notify();
                    })),
            )
            .child(card),
    )
}
