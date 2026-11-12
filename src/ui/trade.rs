//! 下单面板：买/卖、限价/市价/止损/止盈、数量百分比快捷填充、确认下单
//!
//! 里程碑 1：类型与全部纯逻辑（build_request 族/confirm_* 族/fill_by_pct/
//! order_rows）原样迁移，UI 表单在里程碑 7 移植。

use crate::app::{BnApp, ConfirmOrder};
use crate::binance::client::{OrderKind, OrderRequest, Tif};
use crate::binance::{FutPosSide, Market, Side};
use crate::ui::widgets;
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
