//! 下单面板：买/卖、限价/市价/止损/止盈、数量百分比快捷填充、确认下单

use crate::app::{BnApp, ConfirmOrder};
use crate::binance::client::{OrderKind, OrderRequest};
use crate::binance::{FutPosSide, Market, Side};
use crate::engine::{state::ToastKind, Command};
use crate::ui::{theme, widgets};
use eframe::egui;
use rust_decimal::Decimal;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiKind {
    Limit,
    Market,
    StopLossLimit,
    TakeProfitLimit,
    StopLossMarket,
    TakeProfitMarket,
}

impl UiKind {
    pub fn label(&self) -> &'static str {
        match self {
            UiKind::Limit => "限价",
            UiKind::Market => "市价",
            UiKind::StopLossLimit => "止损限价",
            UiKind::TakeProfitLimit => "止盈限价",
            UiKind::StopLossMarket => "止损市价",
            UiKind::TakeProfitMarket => "止盈市价",
        }
    }

    pub fn available(market: Market) -> Vec<UiKind> {
        match market {
            Market::Spot => vec![
                UiKind::Limit,
                UiKind::Market,
                UiKind::StopLossLimit,
                UiKind::TakeProfitLimit,
            ],
            Market::UmFutures => vec![
                UiKind::Limit,
                UiKind::Market,
                UiKind::StopLossMarket,
                UiKind::TakeProfitMarket,
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct OrderForm {
    pub side: Side,
    pub kind: UiKind,
    pub price: String,
    pub qty: String,
    pub trigger: String,
    pub reduce_only: bool,
}

impl Default for OrderForm {
    fn default() -> Self {
        Self {
            side: Side::Buy,
            kind: UiKind::Limit,
            price: String::new(),
            qty: String::new(),
            trigger: String::new(),
            reduce_only: false,
        }
    }
}

impl OrderForm {
    /// 盘口点击价格
    pub fn set_price(&mut self, price: &str) {
        self.price = price.to_string();
    }
}

pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let market = app.market;
    let symbol = app.symbol.clone();
    let rule = state.rule_of(market, &symbol);

    ui.add_space(4.0);
    ui.heading(egui::RichText::new(format!("{}{symbol}", crate::i18n::tr("下单 · "))).size(14.0));
    ui.separator();

    if !state.has_creds() {
        ui.label(
            egui::RichText::new("未配置 API 密钥，仅可查看行情")
                .color(theme::cur().warn)
                .size(12.0),
        );
        ui.add_space(2.0);
    }

    // ---------- 买/卖切换 ----------
    // 双向持仓模式下买卖即决定开多/开空，持仓方向随方向按钮联动（见 build_request）
    ui.columns(2, |cols| {
        let buy = egui::RichText::new(if market == Market::UmFutures {
            crate::i18n::tr("买入 / 开多")
        } else {
            crate::i18n::tr("买入")
        })
        .strong()
        .size(14.5)
        .color(egui::Color32::WHITE);
        let buy = egui::Button::new(buy)
            .fill(if app.form.side == Side::Buy {
                theme::cur().up
            } else {
                theme::cur().btn_idle
            })
            .rounding(5.0);
        if cols[0]
            .add_sized([cols[0].available_width(), 36.0], buy)
            .clicked()
        {
            app.form.side = Side::Buy;
        }
        let sell = egui::RichText::new(if market == Market::UmFutures {
            crate::i18n::tr("卖出 / 开空")
        } else {
            crate::i18n::tr("卖出")
        })
        .strong()
        .size(14.5)
        .color(egui::Color32::WHITE);
        let sell = egui::Button::new(sell)
            .fill(if app.form.side == Side::Sell {
                theme::cur().down
            } else {
                theme::cur().btn_idle
            })
            .rounding(5.0);
        if cols[1]
            .add_sized([cols[1].available_width(), 36.0], sell)
            .clicked()
        {
            app.form.side = Side::Sell;
        }
    });
    ui.add_space(2.0);

    // ---------- 订单类型 ----------
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::card_label(crate::i18n::tr("类型")));
            let kinds = UiKind::available(market);
            let cur = if kinds.contains(&app.form.kind) {
                app.form.kind
            } else {
                kinds[0]
            };
            egui::ComboBox::from_id_salt("order_kind")
                .selected_text(cur.label())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for k in kinds {
                        ui.selectable_value(&mut app.form.kind, k, k.label());
                    }
                });
        });

        // ---------- 合约持仓方向 / 只减仓 / 杠杆 ----------
        if market == Market::UmFutures {
            let dual = state.acct.lock().fut_dual_side;
            if dual {
                // 双向模式：持仓方向跟随买/卖（买入=开多 LONG，卖出=开空 SHORT），
                // 避免方向按钮与持仓侧独立选择导致 "SELL+LONG" 平多误操作
                let pos_txt = if app.form.side == Side::Buy {
                    crate::i18n::tr("开多 (LONG)")
                } else {
                    crate::i18n::tr("开空 (SHORT)")
                };
                ui.horizontal(|ui| {
                    ui.label(theme::card_label(crate::i18n::tr("持仓方向")));
                    ui.label(
                        egui::RichText::new(pos_txt)
                            .strong()
                            .color(theme::cur().accent),
                    );
                });
            }
            if !dual {
                ui.checkbox(
                    &mut app.form.reduce_only,
                    crate::i18n::tr("只减仓 (ReduceOnly)"),
                );
            }
            // 杠杆
            ui.horizontal(|ui| {
                ui.label(theme::card_label(crate::i18n::tr("杠杆")));
                let lev = state
                    .acct
                    .lock()
                    .fut_leverage
                    .get(&symbol)
                    .and_then(|d| d.to_string().parse::<u32>().ok())
                    .unwrap_or(0);
                if lev > 0 {
                    ui.label(
                        egui::RichText::new(format!("{lev}x"))
                            .strong()
                            .color(theme::cur().accent),
                    );
                }
                let _ = widgets::num_field(
                    ui,
                    "lev_in",
                    &mut app.leverage_input,
                    crate::i18n::tr("杠杆倍数"),
                );
                if ui.button(crate::i18n::tr("设定")).clicked() {
                    match app.leverage_input.trim().parse::<u32>() {
                        Ok(l) if (1..=125).contains(&l) => {
                            state.send_cmd(Command::SetLeverage {
                                symbol: symbol.clone(),
                                leverage: l,
                            });
                        }
                        _ => {
                            state.push_toast(
                                ToastKind::Warn,
                                crate::i18n::tr("杠杆倍数需为 1-125 的整数").to_string(),
                            );
                        }
                    }
                }
            });
        }
    }); // 类型与合约参数卡片

    // ---------- 价格 / 触发价 / 数量 ----------
    theme::card(ui, |ui| {
        if app.form.kind != UiKind::Market {
            ui.horizontal(|ui| {
                ui.label(theme::card_label(crate::i18n::tr("价格")));
                widgets::num_field(ui, "price_in", &mut app.form.price, crate::i18n::tr("价格"));
                if ui.button(crate::i18n::tr("市价")).clicked() {
                    let last = state
                        .md
                        .lock()
                        .get(market, &symbol)
                        .and_then(|md| md.ticker.as_ref().map(|t| t.last_price));
                    if let Some(p) = last {
                        app.form.price = p.normalize().to_string();
                    }
                }
            });
        }
        if matches!(
            app.form.kind,
            UiKind::StopLossLimit
                | UiKind::TakeProfitLimit
                | UiKind::StopLossMarket
                | UiKind::TakeProfitMarket
        ) {
            ui.horizontal(|ui| {
                ui.label(theme::card_label(crate::i18n::tr("触发价")));
                widgets::num_field(
                    ui,
                    "trig_in",
                    &mut app.form.trigger,
                    crate::i18n::tr("触发价"),
                );
            });
        }
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("数量"));
            widgets::num_field(ui, "qty_in", &mut app.form.qty, crate::i18n::tr("数量"));
            if let Some(r) = &rule {
                ui.label(
                    egui::RichText::new(format!(
                        "{} {}",
                        crate::i18n::tr("步进"),
                        r.step.normalize()
                    ))
                    .size(10.5)
                    .color(theme::cur().text_dim),
                );
            }
        });

        // ---------- 百分比快捷填充 ----------
        ui.horizontal(|ui| {
            ui.label(theme::card_label(crate::i18n::tr("比例")));
            for pct in [25.0, 50.0, 75.0, 100.0] {
                if ui
                    .button(format!("{}%", pct as i32))
                    .on_hover_text(crate::i18n::tr("按可用余额估算"))
                    .clicked()
                {
                    fill_by_pct(app, pct / 100.0);
                }
            }
        });
    }); // 价格数量卡片

    // ---------- 汇总与量化预览 ----------
    let (qty_dec, price_dec) = (
        widgets::parse_dec(&app.form.qty),
        widgets::parse_dec(&app.form.price),
    );
    let mut total: Option<Decimal> = None;
    // checked_mul 防超大输入导致 rust_decimal 溢出 panic
    match app.form.kind {
        UiKind::Limit => {
            if let (Some(q), Some(p)) = (qty_dec, price_dec) {
                total = q.checked_mul(p);
            }
        }
        UiKind::Market => {
            let last = state
                .md
                .lock()
                .get(market, &symbol)
                .and_then(|md| md.ticker.as_ref().map(|t| t.last_price));
            if let (Some(q), Some(p)) = (qty_dec, last) {
                total = q.checked_mul(p);
            }
        }
        _ => {
            if let (Some(q), Some(p)) = (qty_dec, price_dec) {
                total = q.checked_mul(p);
            } else if let Some(q) = qty_dec {
                let last = state
                    .md
                    .lock()
                    .get(market, &symbol)
                    .and_then(|md| md.ticker.as_ref().map(|t| t.last_price));
                if let Some(p) = last {
                    total = q.checked_mul(p);
                }
            }
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("预估金额")).color(theme::cur().text_dim));
        match total {
            Some(t) => {
                ui.label(egui::RichText::new(t.normalize().round_dp(8).to_string()).monospace())
            }
            None => ui.label(egui::RichText::new("—").color(theme::cur().text_dim)),
        };
    });
    // 量化预览
    if let Some(r) = &rule {
        let mut hints: Vec<String> = Vec::new();
        if let Some(p) = price_dec {
            if let Some(qp) = crate::binance::symbols::quantize_price(p, r.tick) {
                if qp != p {
                    hints.push(format!(
                        "{} {}",
                        crate::i18n::tr("价格将量化为"),
                        qp.normalize()
                    ));
                }
            } else {
                hints.push(crate::i18n::tr("价格超出可表示范围").to_string());
            }
        }
        if let Some(q) = qty_dec {
            match crate::binance::symbols::quantize_qty(q, r.step) {
                Some(qq) => {
                    if qq != q {
                        hints.push(format!(
                            "{} {}",
                            crate::i18n::tr("数量将量化为"),
                            qq.normalize()
                        ));
                    }
                    if !r.min_qty.is_zero() && qq < r.min_qty {
                        hints.push(format!(
                            "{} {}",
                            crate::i18n::tr("小于最小下单数量"),
                            r.min_qty.normalize()
                        ));
                    }
                    if let Some(tp) = total {
                        if !r.min_notional.is_zero() && tp < r.min_notional {
                            hints.push(format!(
                                "{} {}{}",
                                crate::i18n::tr("低于最小名义金额"),
                                r.min_notional.normalize(),
                                crate::i18n::tr("（限价单）")
                            ));
                        }
                    }
                }
                None => hints.push(crate::i18n::tr("数量超出可表示范围").to_string()),
            }
        }
        if !hints.is_empty() {
            ui.label(
                egui::RichText::new(hints.join("；"))
                    .size(11.0)
                    .color(theme::cur().warn),
            );
        }
    }

    // ---------- 提交 ----------
    ui.add_space(4.0);
    let busy = state.is_busy("place");
    let (word, color) = match (app.form.side, market) {
        (Side::Buy, Market::Spot) => (crate::i18n::tr("买入"), theme::cur().up),
        (Side::Sell, Market::Spot) => (crate::i18n::tr("卖出"), theme::cur().down),
        (Side::Buy, Market::UmFutures) => (crate::i18n::tr("买入开多"), theme::cur().up),
        (Side::Sell, Market::UmFutures) => (crate::i18n::tr("卖出开空"), theme::cur().down),
    };
    let btn = egui::Button::new(
        egui::RichText::new(format!("{word} {symbol}"))
            .strong()
            .size(15.5)
            .color(egui::Color32::WHITE),
    )
    .fill(color)
    .rounding(5.0)
    .min_size(egui::vec2(ui.available_width(), 38.0));
    let resp = ui.add_enabled(!busy && state.has_creds(), btn);
    if busy {
        ui.label(
            egui::RichText::new("提交中…")
                .size(11.5)
                .color(theme::cur().text_dim),
        );
    }
    if resp.clicked() {
        match build_request(app) {
            Ok(req) => {
                let rows = order_rows(&req);
                app.confirm = Some(ConfirmOrder {
                    market: app.market,
                    req,
                    rows,
                    side: app.form.side,
                });
            }
            Err(e) => {
                app.state
                    .push_toast(crate::engine::state::ToastKind::Warn, e);
            }
        }
    }
}

/// 从 UI 状态构建订单请求（量化后），供提交与确认弹窗使用
pub fn build_request(app: &BnApp) -> Result<OrderRequest, String> {
    let state = &app.state;
    let market = app.market;
    let symbol = app.symbol.clone();
    let rules = state.rules_of(market);
    let Some(r) = rules.get(&symbol) else {
        return Err(crate::i18n::tr("交易规则尚未加载").to_string());
    };
    let qty = widgets::parse_dec(&app.form.qty).ok_or(crate::i18n::tr("请输入数量"))?;
    let price = widgets::parse_dec(&app.form.price);
    let trigger = widgets::parse_dec(&app.form.trigger);

    // 量化（Option 化防溢出）
    let q_price = |v: Decimal| {
        crate::binance::symbols::quantize_price(v, r.tick)
            .ok_or_else(|| crate::i18n::tr("价格超出可表示范围").to_string())
    };
    let q_qty = |v: Decimal| {
        crate::binance::symbols::quantize_qty(v, r.step)
            .ok_or_else(|| crate::i18n::tr("数量超出可表示范围").to_string())
    };
    let kind = match app.form.kind {
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
    };
    let dual = state.acct.lock().fut_dual_side;
    // 双向持仓：持仓方向跟随买卖方向（买入=开多 LONG，卖出=开空 SHORT）
    let position_side = if market == Market::UmFutures && dual {
        if app.form.side == Side::Buy {
            FutPosSide::Long
        } else {
            FutPosSide::Short
        }
    } else {
        FutPosSide::Both
    };
    Ok(OrderRequest {
        symbol,
        side: app.form.side,
        kind,
        position_side,
        reduce_only: if dual { false } else { app.form.reduce_only },
        client_id: None,
    })
}

/// 百分比快捷填充
fn fill_by_pct(app: &mut BnApp, pct: f64) {
    let state = &app.state;
    let market = app.market;
    let symbol = app.symbol.clone();
    let rules = state.rules_of(market);
    let Some(r) = rules.get(&symbol) else { return };

    // 参考价：优先表单价格，其次最新价
    let ref_price = widgets::parse_dec(&app.form.price).or_else(|| {
        state
            .md
            .lock()
            .get(market, &symbol)
            .and_then(|md| md.ticker.as_ref().map(|t| t.last_price))
    });
    let Some(ref_price) = ref_price else { return };
    let price_f: f64 = ref_price.to_string().parse().unwrap_or(0.0);
    if price_f <= 0.0 {
        return;
    }

    let qty: Decimal = match (market, app.form.side) {
        (Market::Spot, Side::Buy) => {
            let free = state.acct.lock().spot.as_ref().and_then(|a| {
                a.balances
                    .iter()
                    .find(|b| b.asset == r.quote)
                    .map(|b| b.free)
            });
            let f: f64 = free
                .unwrap_or(Decimal::ZERO)
                .to_string()
                .parse()
                .unwrap_or(0.0);
            Decimal::try_from(f * pct / price_f).unwrap_or(Decimal::ZERO)
        }
        (Market::Spot, Side::Sell) => {
            let free = state.acct.lock().spot.as_ref().and_then(|a| {
                a.balances
                    .iter()
                    .find(|b| b.asset == r.base)
                    .map(|b| b.free)
            });
            let f: f64 = free
                .unwrap_or(Decimal::ZERO)
                .to_string()
                .parse()
                .unwrap_or(0.0);
            Decimal::try_from(f * pct).unwrap_or(Decimal::ZERO)
        }
        (Market::UmFutures, _) => {
            let avail = state
                .acct
                .lock()
                .fut
                .as_ref()
                .map(|a| a.available)
                .unwrap_or(Decimal::ZERO);
            let f: f64 = avail.to_string().parse().unwrap_or(0.0);
            let lev = state
                .acct
                .lock()
                .fut_leverage
                .get(&symbol)
                .and_then(|d| d.to_string().parse::<f64>().ok())
                .unwrap_or(1.0);
            Decimal::try_from(f * lev * pct / price_f).unwrap_or(Decimal::ZERO)
        }
    };
    // 量化失败（溢出）时不填充
    if let Some(q) = crate::binance::symbols::quantize_qty(qty, r.step) {
        app.form.qty = q.normalize().to_string();
    }
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
    }
    if req.reduce_only {
        rows.push((tr("只减仓").into(), tr("是").into()));
    }
    if req.position_side != FutPosSide::Both {
        rows.push((tr("持仓方向").into(), req.position_side.label().into()));
    }
    rows
}
