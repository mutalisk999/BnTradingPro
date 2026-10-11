//! 应用主体：启动流程路由（设置密码/解锁/配置密钥/主界面）与交易终端布局

use crate::binance::client::OrderRequest;
use crate::binance::{Interval, Market, Side};
use crate::config::{self, AppConfig};
use crate::engine::state::SharedState;
use crate::engine::{Command, Engine};
use crate::security::vault;
use crate::ui::{chart, orders, settings, theme, trade, unlock, widgets};
use eframe::egui;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    SetupMaster,
    Unlock,
    EnterKeys,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomTab {
    Open,
    Positions,
    History,
    Trades,
    Assets,
    Settings,
}

pub struct ConfirmOrder {
    pub market: Market,
    pub req: OrderRequest,
    pub rows: Vec<(String, String)>,
    pub side: Side,
}

pub struct BnApp {
    pub state: Arc<SharedState>,
    pub engine: Engine,

    pub phase: Phase,
    pub master: Option<String>,
    pub pw1: String,
    pub pw2: String,
    pub pw_err: String,
    pub api_key: String,
    pub api_secret: String,
    pub keys_err: String,

    pub market: Market,
    pub symbol: String,
    pub interval: Interval,
    pub symbol_filter: String,
    pub form: trade::OrderForm,
    pub leverage_input: String,
    pub confirm: Option<ConfirmOrder>,
    pub bottom_tab: BottomTab,
    pub chart: chart::ChartState,
}

impl BnApp {
    pub fn new(cc: &eframe::CreationContext, cfg: AppConfig) -> Self {
        theme::apply_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);

        let dir = config::data_dir();
        let state = Arc::new(SharedState::default());
        *state.cfg.write() = cfg.clone();
        let engine = crate::engine::spawn(cc.egui_ctx.clone(), Arc::clone(&state));

        let phase = if vault::vault_exists(&dir) {
            Phase::Unlock
        } else {
            Phase::SetupMaster
        };
        let interval = Interval::from_str(&cfg.default_interval);

        Self {
            state,
            engine,
            phase,
            master: None,
            pw1: String::new(),
            pw2: String::new(),
            pw_err: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            keys_err: String::new(),
            market: Market::Spot,
            symbol: cfg.default_symbol.to_uppercase(),
            interval,
            symbol_filter: String::new(),
            form: trade::OrderForm::default(),
            leverage_input: String::new(),
            confirm: None,
            bottom_tab: BottomTab::Open,
            chart: chart::ChartState::default(),
        }
    }

    // ---------- 订阅切换 ----------

    pub fn switch_market(&mut self, m: Market) {
        if self.market == m {
            return;
        }
        self.market = m;
        // 订单类型可用性
        if !trade::UiKind::available(m).contains(&self.form.kind) {
            self.form.kind = trade::UiKind::Limit;
        }
        self.state.send_cmd(Command::Subscribe {
            market: m,
            symbol: self.symbol.clone(),
            interval: self.interval,
        });
        if self.state.has_creds() {
            self.state
                .send_cmd(Command::RefreshOpenOrders { market: m });
            if m == Market::UmFutures {
                self.state.send_cmd(Command::RefreshAccount);
            }
        }
    }

    pub fn switch_symbol(&mut self, s: String) {
        if self.symbol == s {
            return;
        }
        self.symbol = s.clone();
        self.form.price.clear();
        self.state.send_cmd(Command::Subscribe {
            market: self.market,
            symbol: s.clone(),
            interval: self.interval,
        });
        if self.state.has_creds() {
            self.state.send_cmd(Command::RefreshOpenOrders {
                market: self.market,
            });
            self.state.send_cmd(Command::RefreshHistory {
                market: self.market,
                symbol: s,
            });
        }
    }

    pub fn switch_interval(&mut self, iv: Interval) {
        if self.interval == iv {
            return;
        }
        self.interval = iv;
        self.state.send_cmd(Command::Subscribe {
            market: self.market,
            symbol: self.symbol.clone(),
            interval: iv,
        });
    }

    // ---------- eframe::App ----------

    fn main_ui(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| self.topbar(ui));

        egui::SidePanel::right("trade_panel")
            .resizable(true)
            .default_width(300.0)
            .width_range(240.0..=420.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| trade::show(self, ui));
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::TopBottomPanel::bottom("bottom_tabs")
                .resizable(true)
                .default_height(250.0)
                .height_range(120.0..=500.0)
                .show_inside(ui, |ui| self.bottom_tabs(ui));

            egui::SidePanel::right("book_panel")
                .resizable(true)
                .default_width(230.0)
                .width_range(170.0..=340.0)
                .show_inside(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        crate::ui::market::show_book(self, ui);
                        ui.add_space(6.0);
                        crate::ui::market::show_trades(self, ui);
                    });
                });

            egui::CentralPanel::default().show_inside(ui, |ui| chart::show(self, ui));
        });

        self.confirm_dialog(ctx);
    }

    fn topbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("BnTradingPro")
                    .strong()
                    .color(theme::ACCENT)
                    .size(16.0),
            );
            ui.separator();

            // 市场切换
            for m in Market::all() {
                if ui.selectable_label(self.market == m, m.label()).clicked() {
                    self.switch_market(m);
                }
            }
            ui.separator();

            // 交易对
            egui::ComboBox::from_id_salt("symbol_cb")
                .selected_text(egui::RichText::new(self.symbol.clone()).strong())
                .width(130.0)
                .show_ui(ui, |ui| {
                    let book = self.state.rules_of(self.market);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.symbol_filter)
                            .hint_text("搜索交易对…"),
                    );
                    let f = self.symbol_filter.trim().to_uppercase();
                    let mut count = 0;
                    for s in &book.symbols {
                        if count >= 200 {
                            break;
                        }
                        if f.is_empty() || s.contains(&f) {
                            count += 1;
                            if ui.selectable_label(*s == self.symbol, s.as_str()).clicked() {
                                self.switch_symbol(s.clone());
                            }
                        }
                    }
                    if count >= 200 {
                        ui.label(
                            egui::RichText::new("（结果过多，请输入过滤条件）")
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                        );
                    }
                });

            // 周期
            for iv in Interval::all() {
                if ui
                    .selectable_label(self.interval == iv, iv.label())
                    .clicked()
                {
                    self.switch_interval(iv);
                }
            }

            // 右侧：连接状态 + 最新价
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let testnet = self.state.cfg.read().testnet;
                if testnet {
                    theme::badge(ui, "TESTNET", theme::ACCENT);
                } else {
                    theme::badge(ui, "实盘 · 真实资金", theme::ERR);
                }
                ui.separator();
                ui.label(
                    egui::RichText::new("REST")
                        .size(10.5)
                        .color(theme::TEXT_DIM),
                );
                let rest_ok = self
                    .state
                    .rest_ok
                    .load(std::sync::atomic::Ordering::Relaxed);
                status_dot_bare(ui, rest_ok);
                ui.label(
                    egui::RichText::new("行情")
                        .size(10.5)
                        .color(theme::TEXT_DIM),
                );
                let mws = self.state.md.lock().market_ws(self.market).clone();
                widgets::status_dot(ui, &mws);
                ui.label(
                    egui::RichText::new("账户")
                        .size(10.5)
                        .color(theme::TEXT_DIM),
                );
                if self.state.has_creds() {
                    let uws = self.state.md.lock().user_ws(self.market).clone();
                    widgets::status_dot(ui, &uws);
                } else {
                    ui.label(
                        egui::RichText::new("未配置密钥")
                            .size(10.5)
                            .color(theme::WARN),
                    );
                }
                ui.separator();
                let tick = self
                    .state
                    .md
                    .lock()
                    .get(self.market, &self.symbol)
                    .and_then(|md| md.ticker.clone());
                if let Some(t) = tick {
                    let chg: f64 = t.price_change_percent.to_string().parse().unwrap_or(0.0);
                    let c = theme::signed_color(chg);
                    ui.label(
                        egui::RichText::new(widgets::fmt_dec(&t.last_price))
                            .monospace()
                            .strong()
                            .size(17.0)
                            .color(c),
                    );
                    ui.label(
                        egui::RichText::new(format!("{chg:+.2}%"))
                            .monospace()
                            .size(12.5)
                            .color(c),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "24h 高 {} / 低 {}",
                            widgets::fmt_dec(&t.high_price),
                            widgets::fmt_dec(&t.low_price)
                        ))
                        .size(11.0)
                        .color(theme::TEXT_DIM),
                    );
                } else {
                    ui.label(egui::RichText::new("获取行情中…").color(theme::TEXT_DIM));
                }
            });
        });
    }

    fn bottom_tabs(&mut self, ui: &mut egui::Ui) {
        let tabs = [
            (BottomTab::Open, "当前挂单"),
            (BottomTab::Positions, "合约持仓"),
            (BottomTab::History, "订单历史"),
            (BottomTab::Trades, "成交历史"),
            (BottomTab::Assets, "资产"),
            (BottomTab::Settings, "设置"),
        ];
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            for (t, label) in tabs {
                if ui.selectable_label(self.bottom_tab == t, label).clicked() {
                    self.bottom_tab = t;
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.bottom_tab {
                BottomTab::Open => orders::show_open(self, ui),
                BottomTab::Positions => orders::show_positions(self, ui),
                BottomTab::History => orders::show_history(self, ui),
                BottomTab::Trades => orders::show_trades_history(self, ui),
                BottomTab::Assets => crate::ui::assets::show(self, ui),
                BottomTab::Settings => settings::show(self, ui),
            });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        if self.confirm.is_none() {
            return;
        }
        let mut submit = false;
        let mut keep = true;
        let mut cancel = false;
        egui::Window::new(egui::RichText::new("确认下单").strong())
            .open(&mut keep)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if let Some(c) = &self.confirm {
                    let color = if c.side == Side::Buy {
                        theme::UP
                    } else {
                        theme::DOWN
                    };
                    let (word, testnet) = (
                        if c.side == Side::Buy {
                            "买入"
                        } else {
                            "卖出"
                        },
                        self.state.cfg.read().testnet,
                    );
                    ui.label(
                        egui::RichText::new(if testnet {
                            format!("【测试网】{word}确认")
                        } else {
                            format!("【实盘】{word}确认 · 真实资金！")
                        })
                        .strong()
                        .color(if testnet {
                            theme::ACCENT
                        } else {
                            theme::ERR
                        }),
                    );
                    ui.separator();
                    egui::Grid::new("confirm_grid")
                        .num_columns(2)
                        .spacing([16.0, 5.0])
                        .show(ui, |ui| {
                            for (k, v) in &c.rows {
                                ui.label(egui::RichText::new(k).color(theme::TEXT_DIM));
                                ui.label(egui::RichText::new(v).monospace().strong().color(color));
                                ui.end_row();
                            }
                        });
                    ui.add_space(6.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(egui::RichText::new("确认提交").strong()))
                            .clicked()
                        {
                            submit = true;
                        }
                        if ui.button("取消").clicked() {
                            cancel = true;
                        }
                    });
                }
            });
        if submit {
            let c = self.confirm.take().unwrap();
            self.engine.send(Command::PlaceOrder {
                market: c.market,
                req: c.req,
            });
        } else if !keep || cancel {
            self.confirm = None;
        }
    }
}

/// 简化状态灯（布尔）
fn status_dot_bare(ui: &mut egui::Ui, ok: bool) {
    let c = if ok { theme::UP } else { theme::ERR };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, c);
}

impl eframe::App for BnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match self.phase {
            Phase::Main => self.main_ui(ctx),
            _ => unlock::show(self, ctx),
        }
        widgets::draw_toasts(ctx, &self.state);
        ctx.request_repaint_after(Duration::from_millis(400));
    }
}
