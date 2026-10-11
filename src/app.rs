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

/// 顶层功能页签
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Spot,
    Futures,
    Portfolio,
    Settings,
}

impl Tab {
    pub fn label(&self) -> &'static str {
        match self {
            Tab::Spot => "现货交易",
            Tab::Futures => "期货交易",
            Tab::Portfolio => "Portfolio",
            Tab::Settings => "设置",
        }
    }

    pub fn market(&self) -> Option<Market> {
        match self {
            Tab::Spot => Some(Market::Spot),
            Tab::Futures => Some(Market::UmFutures),
            _ => None,
        }
    }
}

/// 交易页内的底部子页签
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubTab {
    Open,
    Positions,
    History,
    Trades,
}

/// 单个交易页（现货/期货各一份）的独立状态
#[derive(Debug, Clone)]
pub struct TradingPage {
    pub symbol: String,
    pub interval: Interval,
    pub symbol_filter: String,
    pub form: trade::OrderForm,
    pub chart: chart::ChartState,
    pub sub_tab: SubTab,
    pub leverage_input: String,
}

impl TradingPage {
    pub fn new(symbol: String, interval: Interval) -> Self {
        Self {
            symbol,
            interval,
            symbol_filter: String::new(),
            form: trade::OrderForm::default(),
            chart: chart::ChartState::default(),
            sub_tab: SubTab::Open,
            leverage_input: String::new(),
        }
    }
}

pub struct ConfirmOrder {
    pub market: Market,
    pub req: OrderRequest,
    pub rows: Vec<(String, String)>,
    pub side: Side,
}

/// 设置页的编辑草稿：存放在 BnApp 上，避免 egui 每帧从快照重建导致输入被重置
#[derive(Debug, Clone, Default)]
pub struct SettingsDraft {
    pub testnet: bool,
    pub recv_window_ms: u64,
    pub http_timeout_secs: u64,
    pub log_level: String,
    pub default_symbol: String,
    pub default_interval: String,
    pub override_spot_rest: String,
    pub override_spot_ws: String,
    pub override_fut_rest: String,
    pub override_fut_ws: String,
    pub proxy_enabled: bool,
    pub proxy_url: String,
}

impl SettingsDraft {
    pub fn from_cfg(cfg: &AppConfig) -> Self {
        Self {
            testnet: cfg.testnet,
            recv_window_ms: cfg.recv_window_ms,
            http_timeout_secs: cfg.http_timeout_secs,
            log_level: cfg.log_level.clone(),
            default_symbol: cfg.default_symbol.clone(),
            default_interval: cfg.default_interval.clone(),
            override_spot_rest: cfg.override_spot_rest.clone(),
            override_spot_ws: cfg.override_spot_ws.clone(),
            override_fut_rest: cfg.override_fut_rest.clone(),
            override_fut_ws: cfg.override_fut_ws.clone(),
            proxy_enabled: cfg.proxy.enabled,
            proxy_url: cfg.proxy.url.clone(),
        }
    }

    pub fn into_cfg(self) -> AppConfig {
        AppConfig {
            testnet: self.testnet,
            default_symbol: self.default_symbol.trim().to_uppercase(),
            default_interval: self.default_interval,
            recv_window_ms: self.recv_window_ms,
            http_timeout_secs: self.http_timeout_secs,
            log_level: self.log_level,
            override_spot_rest: self.override_spot_rest,
            override_spot_ws: self.override_spot_ws,
            override_fut_rest: self.override_fut_rest,
            override_fut_ws: self.override_fut_ws,
            proxy: crate::config::ProxyConfig {
                enabled: self.proxy_enabled,
                url: self.proxy_url,
            },
        }
    }
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

    /// 当前顶层页签
    pub tab: Tab,
    /// 活动交易上下文（镜像自 spot/fut 页，切页时保存/恢复，UI 代码直接读写）
    pub market: Market,
    pub symbol: String,
    pub interval: Interval,
    pub symbol_filter: String,
    pub symbol_picker_open: bool,
    pub symbol_focus_search: bool,
    pub form: trade::OrderForm,
    pub chart: chart::ChartState,
    pub leverage_input: String,
    pub sub_tab: SubTab,
    pub confirm: Option<ConfirmOrder>,
    pub settings: SettingsDraft,

    /// 现货/期货页各自的持久状态
    pub spot: TradingPage,
    pub fut: TradingPage,
    /// 已向引擎发送的订阅（market → (symbol, interval)），避免重复拉取 K 线
    pub subscribed: [Option<(String, Interval)>; 2],
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

        let default_symbol = cfg.default_symbol.to_uppercase();
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
            tab: Tab::Spot,
            market: Market::Spot,
            symbol: default_symbol.clone(),
            interval,
            symbol_filter: String::new(),
            symbol_picker_open: false,
            symbol_focus_search: false,
            form: trade::OrderForm::default(),
            chart: chart::ChartState::default(),
            leverage_input: String::new(),
            sub_tab: SubTab::Open,
            confirm: None,
            settings: SettingsDraft::from_cfg(&cfg),
            spot: TradingPage::new(default_symbol.clone(), interval),
            fut: TradingPage::new(default_symbol, interval),
            // 引擎启动时已订阅现货默认交易对
            subscribed: [Some((cfg.default_symbol.to_uppercase(), interval)), None],
        }
    }

    // ---------- 页签切换（镜像保存/恢复） ----------

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.symbol_picker_open = false;
        self.save_active_page();
        self.tab = tab;
        self.load_active_page();
    }

    /// 把活动镜像保存回当前页
    fn save_active_page(&mut self) {
        let page = match self.tab {
            Tab::Spot => &mut self.spot,
            Tab::Futures => &mut self.fut,
            _ => return,
        };
        page.symbol = self.symbol.clone();
        page.interval = self.interval;
        page.symbol_filter = self.symbol_filter.clone();
        page.form = self.form.clone();
        page.chart = self.chart.clone();
        page.sub_tab = self.sub_tab;
        page.leverage_input = self.leverage_input.clone();
    }

    /// 从目标页恢复镜像（仅交易页）
    fn load_active_page(&mut self) {
        let Some(m) = self.tab.market() else {
            return;
        };
        let page = match self.tab {
            Tab::Spot => self.spot.clone(),
            Tab::Futures => self.fut.clone(),
            _ => return,
        };
        self.market = m;
        self.symbol = page.symbol;
        self.interval = page.interval;
        self.symbol_filter = page.symbol_filter;
        self.form = page.form;
        self.chart = page.chart;
        self.sub_tab = page.sub_tab;
        self.leverage_input = page.leverage_input;

        self.subscribe(m, self.symbol.clone(), self.interval);
        if self.state.has_creds() {
            self.state
                .send_cmd(Command::RefreshOpenOrders { market: m });
            if m == Market::UmFutures {
                self.state.send_cmd(Command::RefreshAccount);
            }
        }
    }

    /// 订阅行情（同交易对+周期不重复订阅）
    fn subscribe(&mut self, market: Market, symbol: String, interval: Interval) {
        let same = self.subscribed[market as usize]
            .as_ref()
            .map(|(s, i)| *s == symbol && *i == interval)
            .unwrap_or(false);
        if !same {
            self.state.send_cmd(Command::Subscribe {
                market,
                symbol: symbol.clone(),
                interval,
            });
            self.subscribed[market as usize] = Some((symbol, interval));
        }
    }

    // ---------- 订阅切换 ----------

    pub fn switch_symbol(&mut self, s: String) {
        if self.symbol == s {
            return;
        }
        self.symbol = s.clone();
        // 表单残留清空 + 图表回到最新
        self.form.price.clear();
        self.form.qty.clear();
        self.form.trigger.clear();
        self.chart.right_off = 0.0;
        self.subscribe(self.market, s.clone(), self.interval);
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
        let symbol = self.symbol.clone();
        self.subscribe(self.market, symbol, iv);
    }

    // ---------- eframe::App ----------

    // ---------- 布局 ----------

    fn main_ui(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar")
            .frame(
                egui::Frame::default()
                    .fill(theme::PANEL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.set_min_height(40.0);
                self.topbar(ui);
            });

        match self.tab {
            Tab::Spot | Tab::Futures => self.trading_ui(ctx),
            Tab::Portfolio => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| crate::ui::assets::show(self, ui));
                });
            }
            Tab::Settings => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| settings::show(self, ui));
                });
            }
        }

        self.confirm_dialog(ctx);
    }

    /// 顶部导航：品牌 + 四大页签 + 连接状态
    fn topbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("BnTradingPro")
                    .strong()
                    .color(theme::ACCENT)
                    .size(16.0),
            );
            ui.add_space(10.0);

            for tab in [Tab::Spot, Tab::Futures, Tab::Portfolio, Tab::Settings] {
                if theme::nav_tab(ui, tab.label(), self.tab == tab) {
                    self.set_tab(tab);
                }
            }

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
                if let Some(m) = self.tab.market() {
                    ui.label(
                        egui::RichText::new("行情")
                            .size(10.5)
                            .color(theme::TEXT_DIM),
                    );
                    let mws = self.state.md.lock().market_ws(m).clone();
                    widgets::status_dot(ui, &mws);
                    ui.label(
                        egui::RichText::new("账户")
                            .size(10.5)
                            .color(theme::TEXT_DIM),
                    );
                    if self.state.has_creds() {
                        let uws = self.state.md.lock().user_ws(m).clone();
                        widgets::status_dot(ui, &uws);
                    } else {
                        ui.label(
                            egui::RichText::new("未配置密钥")
                                .size(10.5)
                                .color(theme::WARN),
                        );
                    }
                }
            });
        });
    }

    /// 交易页：页头（交易对/周期/价格） + 图表 | 盘口+成交 | 下单面板 + 底部订单区
    fn trading_ui(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("trade_panel")
            .resizable(true)
            .default_width(300.0)
            .width_range(250.0..=420.0)
            .frame(egui::Frame::default().fill(theme::PANEL).inner_margin(8.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| trade::show(self, ui));
            });

        egui::SidePanel::right("book_panel")
            .resizable(true)
            .default_width(230.0)
            .width_range(180.0..=340.0)
            .frame(egui::Frame::default().fill(theme::PANEL).inner_margin(8.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    crate::ui::market::show_book(self, ui);
                    ui.add_space(8.0);
                    crate::ui::market::show_trades(self, ui);
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG).inner_margin(8.0))
            .show(ctx, |ui| {
                self.page_header(ui);
                ui.add_space(2.0);

                egui::TopBottomPanel::bottom("orders_panel")
                    .resizable(true)
                    .default_height(240.0)
                    .height_range(120.0..=500.0)
                    .frame(egui::Frame::default().fill(theme::PANEL).inner_margin(8.0))
                    .show_inside(ui, |ui| self.sub_tabs(ui));

                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(theme::BG).inner_margin(0.0))
                    .show_inside(ui, |ui| chart::show(self, ui));
            });
    }

    /// 页头：交易对 / 周期 / 最新价
    fn page_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.symbol_picker(ui);
            ui.add_space(2.0);

            // 周期
            egui::ComboBox::from_id_salt("interval_cb")
                .selected_text(egui::RichText::new(self.interval.label()).strong())
                .width(64.0)
                .show_ui(ui, |ui| {
                    for iv in Interval::all() {
                        if ui
                            .selectable_label(self.interval == iv, iv.label())
                            .clicked()
                        {
                            self.switch_interval(iv);
                        }
                    }
                });

            ui.separator();
            // 最新价
            let tick = self
                .state
                .md
                .lock()
                .get(self.market, &self.symbol)
                .and_then(|md| md.ticker.clone());
            if let Some(t) = tick {
                let chg: f64 = t.price_change_percent.to_string().parse().unwrap_or(0.0);
                let c = theme::signed_color(chg);
                let px: f64 = t.last_price.to_string().parse().unwrap_or(0.0);
                ui.label(
                    egui::RichText::new(widgets::fmt_thousands(px, 2))
                        .monospace()
                        .strong()
                        .size(21.0)
                        .color(c),
                );
                ui.label(
                    egui::RichText::new(format!("{chg:+.2}%"))
                        .monospace()
                        .size(13.5)
                        .color(c),
                );
            } else {
                ui.label(egui::RichText::new("获取行情中…").color(theme::TEXT_DIM));
            }
        });
    }

    /// 自绘交易对选择弹层：搜索框自动聚焦、点击外部关闭
    fn symbol_picker(&mut self, ui: &mut egui::Ui) {
        let btn = ui.add(
            egui::Button::new(
                egui::RichText::new(format!("{}  ▾", self.symbol))
                    .strong()
                    .size(15.0),
            )
            .fill(theme::PANEL2)
            .rounding(4.0)
            .min_size(egui::vec2(150.0, 28.0)),
        );
        if btn.clicked() {
            self.symbol_picker_open = !self.symbol_picker_open;
            self.symbol_focus_search = self.symbol_picker_open;
            if self.symbol_picker_open {
                self.symbol_filter.clear();
            }
        }
        if !self.symbol_picker_open {
            return;
        }

        let mut keep_open = true;
        let mut popup_rect = egui::Rect::NOTHING;
        let pos = egui::Pos2::new(btn.rect.left(), btn.rect.bottom() + 4.0);
        egui::Area::new(egui::Id::new("symbol_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(theme::PANEL2)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(280.0, 400.0));
                        // 搜索框：打开时自动聚焦
                        let search = ui.add(
                            egui::TextEdit::singleline(&mut self.symbol_filter)
                                .id_salt(egui::Id::new("symbol_search"))
                                .hint_text("搜索交易对（如 BTC）…")
                                .desired_width(f32::INFINITY),
                        );
                        if self.symbol_focus_search {
                            search.request_focus();
                            self.symbol_focus_search = false;
                        }
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let book = self.state.rules_of(self.market);
                            let f = self.symbol_filter.trim().to_uppercase();
                            let mut count = 0;
                            for s in &book.symbols {
                                if count >= 300 {
                                    break;
                                }
                                if f.is_empty() || s.contains(&f) {
                                    count += 1;
                                    if ui
                                        .selectable_label(*s == self.symbol, s.as_str())
                                        .clicked()
                                    {
                                        self.switch_symbol(s.clone());
                                        keep_open = false;
                                    }
                                }
                            }
                            if count == 0 {
                                ui.label(
                                    egui::RichText::new("无匹配交易对")
                                        .size(12.5)
                                        .color(theme::TEXT_DIM),
                                );
                            }
                        });
                    });
                popup_rect = ui.min_rect();
            });

        // 点击弹层与按钮之外的区域时关闭
        if let Some(click_pos) = ui.ctx().input(|i| {
            if i.pointer.any_click() {
                i.pointer.interact_pos()
            } else {
                None
            }
        }) {
            let outside = !popup_rect.contains(click_pos) && !btn.rect.contains(click_pos);
            if outside {
                keep_open = false;
            }
        }
        self.symbol_picker_open = keep_open;
    }



    /// 交易页底部子页签：挂单 / 持仓(期货) / 历史 / 成交
    fn sub_tabs(&mut self, ui: &mut egui::Ui) {
        let mut tabs: Vec<(SubTab, &str)> = vec![
            (SubTab::Open, "当前挂单"),
            (SubTab::History, "订单历史"),
            (SubTab::Trades, "成交历史"),
        ];
        if self.market == Market::UmFutures {
            tabs.insert(1, (SubTab::Positions, "持仓"));
        }
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            for (t, label) in tabs {
                if ui.selectable_label(self.sub_tab == t, label).clicked() {
                    self.sub_tab = t;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let open_n = self.state.orders.lock().open(self.market).len();
                ui.label(
                    egui::RichText::new(format!("{} 笔挂单", open_n))
                        .size(11.5)
                        .color(theme::TEXT_DIM),
                );
            });
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.sub_tab {
                SubTab::Open => orders::show_open(self, ui),
                SubTab::Positions => orders::show_positions(self, ui),
                SubTab::History => orders::show_history(self, ui),
                SubTab::Trades => orders::show_trades_history(self, ui),
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
