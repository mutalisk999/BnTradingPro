//! 下单面板：买/卖、限价/市价/止损/止盈、数量百分比快捷填充、确认下单

use crate::app::{BnApp, ConfirmOrder};
use crate::binance::client::{OrderKind, OrderRequest, Tif};
use crate::binance::{FutPosSide, Market, Side};
use crate::engine::{state::ToastKind, Command};
use crate::ui::{flash, theme, widgets};
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

pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let market = app.market;
    let symbol = app.page.symbol.clone();
    let rule = state.rule_of(market, &symbol);

    // 紧凑布局：下单面板夹在顶部栏与底部订单区之间（高度预算 ~300pt），
    // 行距 3 / 行高 22 / 按钮压到 28，保证默认窗口下全部可见、无滚动条
    ui.spacing_mut().item_spacing.y = 3.0;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{}{symbol}", crate::i18n::tr("下单 · ")))
                .strong()
                .size(theme::FONT_BODY),
        );
        if !state.has_creds() {
            ui.label(
                egui::RichText::new(crate::i18n::tr("未配置 API 密钥，仅可查看行情"))
                    .color(theme::cur().warn)
                    .size(theme::FONT_SM),
            );
        }
    });
    ui.add_space(1.0);

    // ---------- 买/卖切换 ----------
    // 双向持仓模式下买卖即决定开多/开空，持仓方向随方向按钮联动（见 build_request）
    // 两钮之间留 8px 间隙：columns(2) 无间距，选中态色块会连成一条
    let side_gap = 8.0;
    let side_w = ((ui.available_width() - side_gap) / 2.0).max(60.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = side_gap;
        // 选中态经 animate_bool 渐变（色块淡入淡出，与全站选中态动画一致），
        // 未选中侧用主题文字色：浅色主题下 btn_idle 底近白，白字对比度不足
        let buy_on = app.page.form.side == Side::Buy;
        let buy_anim = ui.ctx().animate_bool(egui::Id::new("side_buy"), buy_on);
        let buy = egui::RichText::new(if market == Market::UmFutures {
            crate::i18n::tr("买入 / 开多")
        } else {
            crate::i18n::tr("买入")
        })
        .strong()
        .size(theme::FONT_MD)
        .color(if buy_on {
            theme::cur().on_fill
        } else {
            theme::cur().text
        });
        let buy = egui::Button::new(buy)
            .fill(flash::lerp_color(
                theme::cur().btn_idle,
                theme::cur().up,
                buy_anim,
            ))
            .corner_radius(f32::from(theme::R_MD));
        if ui
            .add_sized([side_w, theme::BTN_H], buy)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            app.page.form.side = Side::Buy;
        }
        let sell_on = app.page.form.side == Side::Sell;
        let sell_anim = ui.ctx().animate_bool(egui::Id::new("side_sell"), sell_on);
        let sell = egui::RichText::new(if market == Market::UmFutures {
            crate::i18n::tr("卖出 / 开空")
        } else {
            crate::i18n::tr("卖出")
        })
        .strong()
        .size(theme::FONT_MD)
        .color(if sell_on {
            theme::cur().on_fill
        } else {
            theme::cur().text
        });
        let sell = egui::Button::new(sell)
            .fill(flash::lerp_color(
                theme::cur().btn_idle,
                theme::cur().down,
                sell_anim,
            ))
            .corner_radius(f32::from(theme::R_MD));
        if ui
            .add_sized([side_w, theme::BTN_H], sell)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            app.page.form.side = Side::Sell;
        }
    });
    ui.add_space(2.0);

    // ---------- 价格/数量输入框公共尾宽（两行右缘对齐成列） ----------
    // 尾区宽 = max(一键填价按钮组, 步进标签) + 行内间距 8；两行输入框
    // 等宽同位，尾部内容各自贴行右缘。按钮组可见性按行内控件区宽度
    // 折叠（与绘制端同一套阈值）
    let md_px = {
        let md = state.md.lock();
        md.get(market, &symbol).map_or((None, None, None), |m| {
            (
                m.depth.bids.first().map(|l| l.price),
                m.depth.asks.first().map(|l| l.price),
                m.ticker.as_ref().map(|t| t.last_price),
            )
        })
    };
    // 行内控件区宽 = 面板内容宽 - 卡片内边距 20 - 标签列 48 - 行内间距 8
    let row_inner = (ui.available_width() - 76.0).max(0.0);
    let px_vis = best_px_vis(row_inner, md_px.0, md_px.1, md_px.2);
    let px_btns_w = best_px_w(ui, px_vis);
    let step_txt = rule
        .as_ref()
        .map(|r| format!("{} {}", crate::i18n::tr("步进"), r.step.normalize()))
        .unwrap_or_default();
    let step_w = if step_txt.is_empty() {
        0.0
    } else {
        ui.painter()
            .layout_no_wrap(
                step_txt.clone(),
                egui::FontId::proportional(theme::FONT_SM),
                theme::cur().text_dim,
            )
            .size()
            .x
    };
    let aux_tail = px_btns_w.max(step_w) + 8.0;

    // ---------- 订单类型 + 价格数量（合并为一张紧凑卡片） ----------
    egui::Frame::default()
        .fill(theme::cur().card)
        .stroke(egui::Stroke::new(
            1.0_f32,
            theme::cur().border.gamma_multiply(0.7),
        ))
        .corner_radius(f32::from(theme::R_LG))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            compact_row(ui, |ui| {
                field_label(ui, crate::i18n::tr("类型"));
                let kinds = UiKind::available(market);
                let cur = if kinds.contains(&app.page.form.kind) {
                    app.page.form.kind
                } else {
                    kinds[0]
                };
                egui::ComboBox::from_id_salt("order_kind")
                    .selected_text(crate::i18n::tr(cur.label()))
                    .width(ui.available_width().max(60.0))
                    .show_ui(ui, |ui| {
                        for k in kinds {
                            ui.selectable_value(
                                &mut app.page.form.kind,
                                k,
                                crate::i18n::tr(k.label()),
                            );
                        }
                    });
            });

            // ---------- 有效期（限价类订单；GTX 仅合约 = 只做 maker） ----------
            if matches!(
                app.page.form.kind,
                UiKind::Limit | UiKind::StopLossLimit | UiKind::TakeProfitLimit
            ) {
                compact_row(ui, |ui| {
                    field_label(ui, crate::i18n::tr("有效期"));
                    for &t in Tif::available(market) {
                        let on = app.page.form.tif == t;
                        // 按钮只留缩写（GTX 全长文案会挤占行宽，期货最小面板宽下
                        // 第四枚胶囊被裁剪）；完整含义在悬停提示中给出
                        let txt = t.as_str().to_string();
                        let btn = egui::Button::new(
                            egui::RichText::new(txt).size(theme::FONT_SM).color(if on {
                                theme::cur().accent
                            } else {
                                theme::cur().text_dim
                            }),
                        )
                        .fill(if on {
                            theme::cur().accent.gamma_multiply(0.16)
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .corner_radius(f32::from(theme::R_SM))
                        .min_size(egui::vec2(0.0, 18.0));
                        if ui
                            .add(btn)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(match t {
                                Tif::Gtc => crate::i18n::tr("成交为止"),
                                Tif::Ioc => crate::i18n::tr("立即成交，剩余撤销"),
                                Tif::Fok => crate::i18n::tr("全部成交或立即撤销"),
                                Tif::Gtx => crate::i18n::tr("只做 maker，不会吃单"),
                            })
                            .clicked()
                        {
                            app.page.form.tif = t;
                        }
                    }
                });
            }

            // ---------- 合约持仓方向 / 只减仓 / 杠杆 ----------
            if market == Market::UmFutures {
                let dual = state.acct.lock().fut_dual_side;
                if dual {
                    // 双向模式：持仓方向跟随买/卖（买入=开多 LONG，卖出=开空 SHORT），
                    // 避免方向按钮与持仓侧独立选择导致 "SELL+LONG" 平多误操作
                    let pos_txt = if app.page.form.side == Side::Buy {
                        crate::i18n::tr("开多 (LONG)")
                    } else {
                        crate::i18n::tr("开空 (SHORT)")
                    };
                    compact_row(ui, |ui| {
                        field_label(ui, crate::i18n::tr("持仓方向"));
                        // 方向信息用涨跌语义色：金色 accent 是主操作/激活态专用，不做信息着色
                        let dir_color = if pos_txt.contains("LONG") {
                            theme::up_color()
                        } else {
                            theme::down_color()
                        };
                        ui.label(egui::RichText::new(pos_txt).strong().color(dir_color));
                    });
                }
                if !dual {
                    compact_row(ui, |ui| {
                        field_label(ui, crate::i18n::tr("只减仓"));
                        ui.checkbox(&mut app.page.form.reduce_only, "");
                    });
                }
                // 杠杆：倍数、保证金模式徽章、倍数输入与"设定"收进同一行，
                // 输入弹性吃剩余宽、"设定"贴行右缘（行数更少，与上下表单连成一体）
                let lev = state
                    .acct
                    .lock()
                    .fut_leverage
                    .get(&symbol)
                    .and_then(|d| d.to_string().parse::<u32>().ok())
                    .unwrap_or(0);
                let mt = state.acct.lock().fut_margin_type.get(&symbol).cloned();
                compact_row(ui, |ui| {
                    field_label(ui, crate::i18n::tr("杠杆"));
                    if lev > 0 {
                        ui.label(egui::RichText::new(format!("{lev}x")).strong());
                    }
                    // 保证金模式（symbolConfig）：点击在 全仓/逐仓 间切换
                    if let Some(mt) = mt {
                        let isolated = mt == "ISOLATED";
                        if theme::btn_ghost(
                            ui,
                            if isolated {
                                crate::i18n::tr("逐仓")
                            } else {
                                crate::i18n::tr("全仓")
                            },
                            true,
                        )
                        .on_hover_text(crate::i18n::tr("点击切换保证金模式（全仓/逐仓）"))
                        .clicked()
                        {
                            state.send_cmd(Command::SetMarginType {
                                symbol: symbol.clone(),
                                isolated: !isolated,
                            });
                        }
                    }
                    // "设定"按钮宽度与 btn_ghost 同源（FONT_SM 文本 + 左右 8 padding）
                    let set_w = ui
                        .painter()
                        .layout_no_wrap(
                            crate::i18n::tr("设定").to_string(),
                            egui::FontId::proportional(theme::FONT_SM),
                            theme::cur().text,
                        )
                        .size()
                        .x
                        + 16.0;
                    widgets::num_field_flex(
                        ui,
                        "lev_in",
                        &mut app.page.leverage_input,
                        crate::i18n::tr("杠杆倍数"),
                        22.0,
                        set_w + 8.0,
                    );
                    if theme::btn_ghost(ui, crate::i18n::tr("设定"), true).clicked() {
                        match app.page.leverage_input.trim().parse::<u32>() {
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
            // 限价/止损限价需要委托价；市价与跟踪委托不需要
            if !matches!(app.page.form.kind, UiKind::Market | UiKind::TrailingStop) {
                compact_row(ui, |ui| {
                    field_label(ui, crate::i18n::tr("价格"));
                    // 输入吃掉公共尾宽以外的全部行宽，与数量行右缘成列；
                    // 一键填价按钮组在尾区内贴行右缘
                    widgets::num_field_flex(
                        ui,
                        "price_in",
                        &mut app.page.form.price,
                        crate::i18n::tr("价格"),
                        22.0,
                        aux_tail,
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        best_px_buttons(ui, app, px_vis, md_px);
                    });
                });
            }
            if matches!(
                app.page.form.kind,
                UiKind::StopLossLimit
                    | UiKind::TakeProfitLimit
                    | UiKind::StopLossMarket
                    | UiKind::TakeProfitMarket
                    | UiKind::Oco
            ) {
                compact_row(ui, |ui| {
                    field_label(ui, crate::i18n::tr("触发价"));
                    widgets::num_field_flex(
                        ui,
                        "trig_in",
                        &mut app.page.form.trigger,
                        crate::i18n::tr("触发价"),
                        22.0,
                        0.0,
                    );
                });
            }
            // OCO 止损腿限价（触发后挂出的限价单）
            if app.page.form.kind == UiKind::Oco {
                compact_row(ui, |ui| {
                    field_label(ui, crate::i18n::tr("止损限价"));
                    widgets::num_field_flex(
                        ui,
                        "sl_limit_in",
                        &mut app.page.form.stop_limit,
                        crate::i18n::tr("委托价"),
                        22.0,
                        0.0,
                    );
                });
            }
            compact_row(ui, |ui| {
                field_label(ui, crate::i18n::tr("数量"));
                // 与价格行共用公共尾宽：输入右缘成列，步进标签贴行右缘
                widgets::num_field_flex(
                    ui,
                    "qty_in",
                    &mut app.page.form.qty,
                    crate::i18n::tr("数量"),
                    22.0,
                    aux_tail,
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !step_txt.is_empty() {
                        ui.label(
                            egui::RichText::new(step_txt)
                                .size(theme::FONT_SM)
                                .color(theme::cur().text_dim),
                        );
                    }
                });
            });

            // 跟踪委托：回调幅度（百分数，价格回撤该幅度触发市价全平）
            if app.page.form.kind == UiKind::TrailingStop {
                compact_row(ui, |ui| {
                    field_label(ui, crate::i18n::tr("回调幅度"));
                    let tail = ui
                        .painter()
                        .layout_no_wrap(
                            crate::i18n::tr("%（0.1-10）").to_string(),
                            egui::FontId::proportional(theme::FONT_SM),
                            theme::cur().text_dim,
                        )
                        .size()
                        .x
                        + 16.0;
                    widgets::num_field_flex(
                        ui,
                        "cb_rate_in",
                        &mut app.page.form.callback_rate,
                        crate::i18n::tr("百分比"),
                        22.0,
                        tail,
                    );
                    ui.label(
                        egui::RichText::new(crate::i18n::tr("%（0.1-10）"))
                            .size(theme::FONT_SM)
                            .color(theme::cur().text_dim),
                    );
                });
            }

            // ---------- 附加止盈/止损（合约单向持仓模式，触发即全平） ----------
            if market == Market::UmFutures
                && matches!(app.page.form.kind, UiKind::Limit | UiKind::Market)
            {
                let dual = state.acct.lock().fut_dual_side;
                compact_row(ui, |ui| {
                    field_label(ui, crate::i18n::tr("止盈/止损"));
                    let enabled = !dual;
                    // add_enabled：双向持仓模式下禁用（禁用态点击无响应，无需复位）
                    ui.add_enabled(enabled, egui::Checkbox::new(&mut app.page.form.tpsl_on, ""))
                        .on_hover_text(if enabled {
                            crate::i18n::tr("下单同时挂出触发即全平的止盈/止损单")
                        } else {
                            crate::i18n::tr("仅单向持仓模式可用（双向模式请用持仓页设置）")
                        });
                });
                if app.page.form.tpsl_on {
                    compact_row(ui, |ui| {
                        field_label(ui, crate::i18n::tr("止盈触发"));
                        widgets::num_field_flex(
                            ui,
                            "tp_in",
                            &mut app.page.form.tp_price,
                            crate::i18n::tr("触发价"),
                            22.0,
                            0.0,
                        );
                    });
                    compact_row(ui, |ui| {
                        field_label(ui, crate::i18n::tr("止损触发"));
                        widgets::num_field_flex(
                            ui,
                            "sl_in",
                            &mut app.page.form.sl_price,
                            crate::i18n::tr("触发价"),
                            22.0,
                            0.0,
                        );
                    });
                }
            }

            // ---------- 百分比快捷填充 ----------
            compact_row(ui, |ui| {
                field_label(ui, crate::i18n::tr("比例"));
                for pct in [25.0, 50.0, 75.0, 100.0] {
                    if theme::btn_ghost(ui, &format!("{}%", pct as i32), true)
                        .on_hover_text(crate::i18n::tr("按可用余额估算"))
                        .clicked()
                    {
                        fill_by_pct(app, pct / 100.0);
                    }
                }
            });
        });

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
                .and_then(|md| md.ticker.as_ref().map(|t| t.last_price))
        }),
    };
    if let (Some(q), Some(p)) = (qty_dec, ref_px) {
        total = q.checked_mul(p);
    }
    // 预估金额：紧凑单行小字（不用表单行，省 8pt）
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(crate::i18n::tr("预估金额"))
                .size(theme::FONT_SM)
                .color(theme::cur().text_dim),
        );
        match total {
            Some(t) => ui.label(
                egui::RichText::new(t.normalize().round_dp(8).to_string())
                    .monospace()
                    .size(theme::FONT_SM),
            ),
            None => ui.label(
                egui::RichText::new("—")
                    .size(theme::FONT_SM)
                    .color(theme::cur().text_dim),
            ),
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
                    .size(theme::FONT_SM)
                    .color(theme::cur().warn),
            );
        }
    }

    // ---------- 提交 ----------
    ui.add_space(4.0);
    let busy = state.is_busy("place");
    // 现货 OCO 走独立构建器（占位主单仅用于按钮启用判定，不会发送）
    let is_oco = market == Market::Spot && app.page.form.kind == UiKind::Oco;
    let check_oco = if is_oco { Some(build_oco(app)) } else { None };
    // 提交前校验：错误以红色提示条显示在按钮上方（输入时实时清红条，
    // 避免错误残留造成"界面在骂人"），真正的下单仍以确认弹窗兜底
    let check = if is_oco {
        Ok((
            OrderRequest {
                symbol: symbol.clone(),
                side: app.page.form.side,
                kind: OrderKind::Market { qty: Decimal::ONE },
                tif: Tif::Gtc,
                position_side: crate::binance::FutPosSide::Both,
                reduce_only: false,
                close_position: false,
                client_id: None,
            },
            Vec::new(),
        ))
    } else {
        build_request_bundle(app)
    };
    let check_ok = if let Some(r) = &check_oco {
        match r {
            Ok(_) => true,
            Err(e) => {
                if app.page.form.err != *e {
                    app.page.form.err = e.clone();
                }
                false
            }
        }
    } else {
        match &check {
            Ok(_) => true,
            Err(e) => {
                if app.page.form.err != *e {
                    app.page.form.err = e.clone();
                }
                false
            }
        }
    };
    let btn_enabled = !busy && state.has_creds() && check_ok;
    if btn_enabled && !app.page.form.err.is_empty() {
        app.page.form.err.clear();
    }
    err_bar(ui, &app.page.form.err);
    let (word, color) = match (app.page.form.side, market) {
        (Side::Buy, Market::Spot) => (crate::i18n::tr("买入"), theme::cur().up),
        (Side::Sell, Market::Spot) => (crate::i18n::tr("卖出"), theme::cur().down),
        (Side::Buy, Market::UmFutures) => (crate::i18n::tr("买入开多"), theme::cur().up),
        (Side::Sell, Market::UmFutures) => (crate::i18n::tr("卖出开空"), theme::cur().down),
    };
    // 交易对买卖的标的其实是 base 资产（BTCUSDT 买的是 BTC），
    // 按钮文案用 base；规则未加载时退回交易对原文
    let base = rule
        .as_ref()
        .map(|r| r.base.clone())
        .unwrap_or_else(|| symbol.clone());
    let btn = egui::Button::new(
        egui::RichText::new(format!("{word} {base}"))
            .strong()
            .size(theme::FONT_BODY)
            .color(theme::cur().on_fill),
    )
    .fill(color)
    .corner_radius(f32::from(theme::R_MD))
    .min_size(egui::vec2(ui.available_width(), 30.0));
    let resp = ui
        .add_enabled(btn_enabled, btn)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let resp = if !btn_enabled {
        resp.on_disabled_hover_text(if busy {
            crate::i18n::tr("上一笔请求处理中…")
        } else {
            crate::i18n::tr("请先修正表单中标红的错误")
        })
    } else {
        resp
    };
    if busy {
        ui.horizontal(|ui| {
            theme::spinner(ui);
            ui.label(
                egui::RichText::new(crate::i18n::tr("提交中…"))
                    .size(theme::FONT_SM)
                    .color(theme::cur().text_dim),
            );
        });
    }
    if resp.clicked() {
        // 校验已在按钮上方红条提示，这里不再重复弹 toast
        if is_oco {
            if let Some(Ok(oco)) = &check_oco {
                let rows = oco_rows(oco);
                app.confirm = Some(ConfirmOrder {
                    market: Market::Spot,
                    oco: Some(oco.clone()),
                    reqs: Vec::new(),
                    side: oco.side,
                    rows,
                });
            }
        } else if let Ok((main, extras)) = check {
            let mut rows = order_rows(&main);
            // 确认弹窗给出金额量级，兜住市价单输错数量的低级失误
            if let Some(t) = total {
                rows.push((
                    crate::i18n::tr("预估金额").to_string(),
                    format!("≈ {}", t.normalize().round_dp(8)),
                ));
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
    }
}

/// 表单行标签：固定宽度（48pt 容下"止损限价"四字 + 留白），让价格/触发价/
/// 数量等输入框左缘对齐；紧凑行高 22（theme::row_label 用 ROW_H=30，下单面板高度预算紧张）
fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui(egui::vec2(48.0, 22.0), |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.label(theme::card_label(text));
        });
    });
}

/// 紧凑表单行：行高 22（theme::form_row 为 30），行内控件统一 22 高
fn compact_row<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let w = ui.available_width();
    ui.allocate_ui(egui::vec2(w, 22.0), |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().button_padding.y = 2.0;
            ui.spacing_mut().interact_size.y = 22.0;
            add_contents(ui)
        })
        .inner
    })
}

/// 价格行一键填值按钮可见性：买一 / 卖一 / 最新。
/// 窄行只显示"最新"，买一/卖一随行内控件区宽度依次展开，保证输入框
/// 不被挤没。与绘制端共用同一判定，测量端据此估算按钮组总宽
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

/// 可见按钮组总宽（组内间距含，与输入框的间隙不含）。
/// 单钮宽度与 btn_ghost 同源：FONT_SM 文本宽 + 左右各 8 padding
fn best_px_w(ui: &egui::Ui, vis: (bool, bool, bool)) -> f32 {
    let mut n = 0_u32;
    let mut w = 0.0_f32;
    for (show, label) in [(vis.0, "买一"), (vis.1, "卖一"), (vis.2, "最新")] {
        if show {
            let t = ui.painter().layout_no_wrap(
                crate::i18n::tr(label).to_string(),
                egui::FontId::proportional(theme::FONT_SM),
                theme::cur().text,
            );
            w += t.size().x + 16.0;
            n += 1;
        }
    }
    if n > 1 {
        w += (n - 1) as f32 * ui.spacing().item_spacing.x;
    }
    w
}

/// 价格行一键填值按钮绘制：可见性与行情数据由调用方在公共尾宽测量时
/// 一并取得，绘制端不再自取锁（保证测量宽度与实际绘制一致）
fn best_px_buttons(
    ui: &mut egui::Ui,
    app: &mut BnApp,
    vis: (bool, bool, bool),
    px: (Option<Decimal>, Option<Decimal>, Option<Decimal>),
) {
    let mut fill = |ui: &mut egui::Ui, label: &str, price: Decimal| {
        if theme::btn_ghost(ui, label, true)
            .on_hover_text(crate::i18n::tr("点击填入下单价格"))
            .clicked()
        {
            app.page.form.price = price.normalize().to_string();
        }
    };
    if vis.0 {
        if let Some(p) = px.0 {
            fill(ui, crate::i18n::tr("买一"), p);
        }
    }
    if vis.1 {
        if let Some(p) = px.1 {
            fill(ui, crate::i18n::tr("卖一"), p);
        }
    }
    if vis.2 {
        if let Some(p) = px.2 {
            fill(ui, crate::i18n::tr("最新"), p);
        }
    }
}

/// 校验错误红条：非空时显示在提交按钮上方。
/// 用语义 err 色——down 色留给行情涨跌，错误与"跌"不是一个概念
fn err_bar(ui: &mut egui::Ui, err: &str) {
    if err.is_empty() {
        return;
    }
    egui::Frame::default()
        .fill(theme::cur().err.gamma_multiply(0.16))
        .corner_radius(theme::R_SM)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(err)
                    .size(theme::FONT_SM)
                    .color(theme::cur().err),
            );
        });
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
