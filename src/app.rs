//! 应用主体：启动流程路由（设置密码/解锁/配置密钥/主界面）与交易终端布局

use crate::binance::client::OrderRequest;
use crate::binance::{Interval, Market, Side};
use crate::config::{self, AppConfig};
use crate::engine::state::SharedState;
use crate::engine::{Command, Engine};
use crate::security::vault;
use crate::ui::{chart, orders, settings, theme, trade, unlock, widgets};
use chrono::TimeZone;
use eframe::egui;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 已有凭据库，等待解锁
    Unlock,
    /// 配置 API 密钥（首次/重置/更换；与主密码一体设置）
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
    Help,
}

impl Tab {
    pub fn label(&self) -> &'static str {
        match self {
            Tab::Spot => crate::i18n::tr("现货交易"),
            Tab::Futures => crate::i18n::tr("期货交易"),
            Tab::Portfolio => crate::i18n::tr("Portfolio"),
            Tab::Settings => crate::i18n::tr("设置"),
            Tab::Help => crate::i18n::tr("使用说明"),
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
    pub lang: String,
    pub theme_mode: String,
    pub favorites_spot: Vec<String>,
    pub favorites_futures: Vec<String>,
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
            lang: cfg.lang.clone(),
            theme_mode: cfg.theme_mode.clone(),
            favorites_spot: cfg.favorites_spot.clone(),
            favorites_futures: cfg.favorites_futures.clone(),
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
            lang: self.lang,
            theme_mode: self.theme_mode,
            favorites_spot: self.favorites_spot,
            favorites_futures: self.favorites_futures,
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
    /// 已应用的主题/语言（用于配置变更时重新应用）
    pub applied_theme: String,
    pub applied_lang: String,
    /// 活动交易上下文（镜像自 spot/fut 页，切页时保存/恢复，UI 代码直接读写）
    pub market: Market,
    pub symbol: String,
    pub interval: Interval,
    pub symbol_filter: String,
    pub symbol_info_open: bool,
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
            Phase::EnterKeys
        };
        let interval = Interval::from_str(&cfg.default_interval);
        theme::set_mode_str(&cfg.theme_mode);
        theme::apply(&cc.egui_ctx);
        crate::i18n::set_lang(if cfg.lang == "en" {
            crate::i18n::EN
        } else {
            crate::i18n::ZH
        });

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
            applied_theme: cfg.theme_mode.clone(),
            applied_lang: cfg.lang.clone(),
            market: Market::Spot,
            symbol: default_symbol.clone(),
            interval,
            symbol_filter: String::new(),
            symbol_info_open: false,
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
        self.symbol_info_open = false;
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
                    .fill(theme::cur().panel)
                    .stroke(egui::Stroke::new(1.0_f32, theme::cur().border))
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
            Tab::Help => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| crate::ui::help::show(self, ui));
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
                    .color(theme::cur().accent)
                    .size(16.0),
            );
            ui.add_space(10.0);

            for tab in [
                Tab::Spot,
                Tab::Futures,
                Tab::Portfolio,
                Tab::Settings,
                Tab::Help,
            ] {
                if theme::nav_tab(ui, tab.label(), self.tab == tab) {
                    self.set_tab(tab);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 主题 / 语言快捷切换
                let (theme_txt, lang_txt) = if theme::is_light() {
                    ("🌙", "EN")
                } else {
                    ("☀", "中")
                };
                if ui
                    .small_button(theme_txt)
                    .on_hover_text(crate::i18n::tr("切换主题"))
                    .clicked()
                {
                    let mut cfg = self.state.cfg.read().clone();
                    let to_light = cfg.theme_mode == "light";
                    cfg.theme_mode = if to_light {
                        "dark".into()
                    } else {
                        "light".into()
                    };
                    cfg.save(&crate::config::data_dir());
                    theme::set_mode_str(&cfg.theme_mode);
                    theme::apply(ui.ctx());
                    self.applied_theme = cfg.theme_mode.clone();
                    *self.state.cfg.write() = cfg;
                }
                if ui
                    .small_button(egui::RichText::new(lang_txt).size(12.0))
                    .on_hover_text(crate::i18n::tr("切换语言"))
                    .clicked()
                {
                    let mut cfg = self.state.cfg.read().clone();
                    let to_en = cfg.lang == "en";
                    cfg.lang = if to_en { "zh".into() } else { "en".into() };
                    cfg.save(&crate::config::data_dir());
                    crate::i18n::set_lang(if cfg.lang == "en" {
                        crate::i18n::EN
                    } else {
                        crate::i18n::ZH
                    });
                    self.applied_lang = cfg.lang.clone();
                    *self.state.cfg.write() = cfg;
                }
                ui.separator();
                let testnet = self.state.cfg.read().testnet;
                if testnet {
                    theme::badge(ui, "TESTNET", theme::cur().accent);
                } else {
                    theme::badge(ui, crate::i18n::tr("实盘 · 真实资金"), theme::cur().err);
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(crate::i18n::tr("REST"))
                        .size(10.5)
                        .color(theme::cur().text_dim),
                );
                let rest_ok = self
                    .state
                    .rest_ok
                    .load(std::sync::atomic::Ordering::Relaxed);
                status_dot_bare(ui, rest_ok);
                if let Some(m) = self.tab.market() {
                    ui.label(
                        egui::RichText::new(crate::i18n::tr("行情"))
                            .size(10.5)
                            .color(theme::cur().text_dim),
                    );
                    let mws = self.state.md.lock().market_ws(m).clone();
                    widgets::status_dot(ui, &mws);
                    ui.label(
                        egui::RichText::new(crate::i18n::tr("账户"))
                            .size(10.5)
                            .color(theme::cur().text_dim),
                    );
                    if self.state.has_creds() {
                        let uws = self.state.md.lock().user_ws(m).clone();
                        widgets::status_dot(ui, &uws);
                    } else {
                        ui.label(
                            egui::RichText::new(crate::i18n::tr("未配置密钥"))
                                .size(10.5)
                                .color(theme::cur().warn),
                        );
                    }
                }
            });
        });
    }

    /// 交易页：左侧交易对列表 + 页头 + 图表 | 盘口+成交 | 下单面板 + 底部订单区
    fn trading_ui(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("symbols_panel")
            .resizable(true)
            .default_width(185.0)
            .width_range(140.0..=320.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel)
                    .inner_margin(8.0),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.symbols_sidebar(ui));
            });

        egui::SidePanel::right("trade_panel")
            .resizable(true)
            .default_width(300.0)
            .width_range(250.0..=420.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel)
                    .inner_margin(8.0),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| trade::show(self, ui));
            });

        egui::SidePanel::right("book_panel")
            .resizable(true)
            .default_width(230.0)
            .width_range(180.0..=340.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel)
                    .inner_margin(8.0),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    crate::ui::market::show_book(self, ui);
                    ui.add_space(8.0);
                    crate::ui::market::show_trades(self, ui);
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().bg)
                    .inner_margin(8.0),
            )
            .show(ctx, |ui| {
                self.page_header(ui);
                ui.add_space(2.0);

                egui::TopBottomPanel::bottom("orders_panel")
                    .resizable(true)
                    .default_height(240.0)
                    .height_range(120.0..=500.0)
                    .frame(
                        egui::Frame::default()
                            .fill(theme::cur().panel)
                            .inner_margin(8.0),
                    )
                    .show_inside(ui, |ui| self.sub_tabs(ui));

                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::default()
                            .fill(theme::cur().bg)
                            .inner_margin(0.0),
                    )
                    .show_inside(ui, |ui| chart::show(self, ui));
            });
    }

    /// 页头：交易对 / 属性ⓘ / 周期 / 最新价
    fn page_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(self.symbol.clone()).strong().size(16.5));
            self.symbol_info(ui);
            ui.add_space(4.0);

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
                ui.label(
                    egui::RichText::new(crate::i18n::tr("获取行情中…"))
                        .color(theme::cur().text_dim),
                );
            }
        });
    }

    /// 左侧边栏：搜索 + 自选置顶 + 全部交易对
    fn symbols_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.symbol_filter)
                .id_salt(egui::Id::new("sidebar_search"))
                .hint_text(crate::i18n::tr("搜索交易对…"))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(2.0);

        let book = self.state.rules_of(self.market);
        let favs = self.state.favorites(self.market);
        let filter = self.symbol_filter.trim().to_uppercase();
        let mut switched: Option<String> = None;
        let mut toggled: Option<String> = None;

        // 自选区（置顶）
        let favs: Vec<String> = favs
            .into_iter()
            .filter(|s| filter.is_empty() || s.contains(&filter))
            .collect();
        if !favs.is_empty() {
            theme::section(ui, crate::i18n::tr("⭐ 自选"));
            for sym in &favs {
                symbol_row(ui, self, sym, true, &mut switched, &mut toggled);
            }
            ui.add_space(2.0);
        }

        // 全部交易对（过滤掉已展示的自选）
        theme::section(ui, crate::i18n::tr("全部"));
        let mut count = 0;
        for sym in &book.symbols {
            if count >= 400 {
                ui.label(
                    egui::RichText::new(crate::i18n::tr("（仅显示前 400 个，请用搜索过滤）"))
                        .size(10.5)
                        .color(theme::cur().text_dim),
                );
                break;
            }
            if !filter.is_empty() && !sym.contains(&filter) {
                continue;
            }
            if favs.contains(sym) {
                continue;
            }
            count += 1;
            symbol_row(ui, self, sym, false, &mut switched, &mut toggled);
        }
        if count == 0 && favs.is_empty() {
            ui.label(
                egui::RichText::new(crate::i18n::tr("无匹配交易对"))
                    .size(12.0)
                    .color(theme::cur().text_dim),
            );
        }

        if let Some(sym) = toggled {
            self.state.toggle_favorite(self.market, &sym);
        }
        if let Some(sym) = switched {
            self.switch_symbol(sym);
        }
    }

    /// 交易对属性弹层（ⓘ）
    fn symbol_info(&mut self, ui: &mut egui::Ui) {
        let btn = ui
            .small_button(egui::RichText::new("ⓘ").size(12.0))
            .on_hover_text(crate::i18n::tr("交易对属性"));
        if btn.clicked() {
            self.symbol_info_open = !self.symbol_info_open;
        }
        if !self.symbol_info_open {
            return;
        }
        let Some(r) = self.state.rule_of(self.market, &self.symbol) else {
            self.symbol_info_open = false;
            return;
        };
        let mut popup_rect = egui::Rect::NOTHING;
        let pos = egui::Pos2::new(btn.rect.left(), btn.rect.bottom() + 4.0);
        egui::Area::new(egui::Id::new("symbol_info_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(theme::cur().panel2)
                    .stroke(egui::Stroke::new(1.0_f32, theme::cur().border))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_min_width(300.0);
                        ui.label(
                            egui::RichText::new(crate::i18n::tr("交易对属性"))
                                .strong()
                                .size(13.5),
                        );
                        ui.separator();
                        egui::Grid::new("symbol_info_grid")
                            .num_columns(2)
                            .spacing([14.0, 3.0])
                            .show(ui, |ui| {
                                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                                    ui.label(theme::card_label(k));
                                    ui.label(egui::RichText::new(v).monospace().size(12.0));
                                    ui.end_row();
                                };
                                row(ui, crate::i18n::tr("状态"), r.status.clone());
                                row(ui, crate::i18n::tr("基础资产"), r.base.clone());
                                row(ui, crate::i18n::tr("计价资产"), r.quote.clone());
                                if self.market == crate::binance::Market::UmFutures {
                                    row(ui, crate::i18n::tr("合约类型"), r.contract_type.clone());
                                    row(ui, crate::i18n::tr("保证金资产"), r.margin_asset.clone());
                                    row(ui, crate::i18n::tr("标的类型"), {
                                        let subs = r.underlying_sub_types.join("/");
                                        if r.underlying_type.is_empty() {
                                            subs
                                        } else if subs.is_empty() {
                                            r.underlying_type.clone()
                                        } else {
                                            format!("{} · {}", r.underlying_type, subs)
                                        }
                                    });
                                    row(ui, crate::i18n::tr("上架时间"), {
                                        if r.onboard_date > 0 {
                                            chrono::Local
                                                .timestamp_millis_opt(r.onboard_date)
                                                .single()
                                                .map(|t| t.format("%Y-%m-%d").to_string())
                                                .unwrap_or_default()
                                        } else {
                                            "—".into()
                                        }
                                    });
                                }
                                row(
                                    ui,
                                    crate::i18n::tr("价格精度"),
                                    r.price_precision.to_string(),
                                );
                                row(ui, crate::i18n::tr("数量精度"), r.qty_precision.to_string());
                                row(
                                    ui,
                                    crate::i18n::tr("最小变动"),
                                    r.tick.normalize().to_string(),
                                );
                                row(
                                    ui,
                                    crate::i18n::tr("最小数量"),
                                    r.min_qty.normalize().to_string(),
                                );
                                if r.max_qty > rust_decimal::Decimal::ZERO {
                                    row(
                                        ui,
                                        crate::i18n::tr("最大数量"),
                                        r.max_qty.normalize().to_string(),
                                    );
                                }
                                row(
                                    ui,
                                    crate::i18n::tr("最小名义金额"),
                                    r.min_notional.normalize().to_string(),
                                );
                                if !r.order_types.is_empty() {
                                    row(
                                        ui,
                                        crate::i18n::tr("支持订单类型"),
                                        r.order_types.join(", "),
                                    );
                                }
                                if !r.permissions.is_empty() {
                                    row(ui, crate::i18n::tr("权限"), r.permissions.join(", "));
                                }
                            });
                    });
                popup_rect = ui.min_rect();
            });
        // 点击弹层外关闭
        if let Some(click_pos) = ui.ctx().input(|i| {
            if i.pointer.any_click() {
                i.pointer.interact_pos()
            } else {
                None
            }
        }) {
            if !popup_rect.contains(click_pos) && !btn.rect.contains(click_pos) {
                self.symbol_info_open = false;
            }
        }
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
                        .color(theme::cur().text_dim),
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
        egui::Window::new(egui::RichText::new(crate::i18n::tr("确认下单")).strong())
            .open(&mut keep)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if let Some(c) = &self.confirm {
                    let color = if c.side == Side::Buy {
                        theme::cur().up
                    } else {
                        theme::cur().down
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
                            theme::cur().accent
                        } else {
                            theme::cur().err
                        }),
                    );
                    ui.separator();
                    egui::Grid::new("confirm_grid")
                        .num_columns(2)
                        .spacing([16.0, 5.0])
                        .show(ui, |ui| {
                            for (k, v) in &c.rows {
                                ui.label(egui::RichText::new(k).color(theme::cur().text_dim));
                                ui.label(egui::RichText::new(v).monospace().strong().color(color));
                                ui.end_row();
                            }
                        });
                    ui.add_space(6.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new(crate::i18n::tr("确认提交")).strong(),
                            ))
                            .clicked()
                        {
                            submit = true;
                        }
                        if ui.button(crate::i18n::tr("取消")).clicked() {
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

/// 侧边栏交易对行：名称（点击切换）+ 星标（点击加/取自选）
fn symbol_row(
    ui: &mut egui::Ui,
    app: &BnApp,
    sym: &str,
    is_fav: bool,
    switched: &mut Option<String>,
    toggled: &mut Option<String>,
) {
    let selected = *sym == app.symbol;
    ui.horizontal(|ui| {
        let name_txt = {
            let t = egui::RichText::new(sym).size(12.5).color(if selected {
                theme::cur().accent
            } else {
                theme::cur().text
            });
            if selected {
                t.strong()
            } else {
                t
            }
        };
        let name_btn = egui::Button::new(name_txt)
            .fill(if selected {
                theme::cur().card
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::NONE)
            .rounding(3.0)
            .min_size(egui::vec2(ui.available_width() - 26.0, 20.0));
        if ui.add(name_btn).clicked() {
            *switched = Some(sym.to_string());
        }
        let star = if is_fav { "★" } else { "☆" };
        let star_txt = egui::RichText::new(star).size(13.0).color(if is_fav {
            theme::cur().accent
        } else {
            theme::cur().text_dim
        });
        if ui
            .add(
                egui::Button::new(star_txt)
                    .fill(egui::Color32::TRANSPARENT)
                    .rounding(3.0),
            )
            .on_hover_text(if is_fav {
                crate::i18n::tr("取消自选")
            } else {
                crate::i18n::tr("加入自选")
            })
            .clicked()
        {
            *toggled = Some(sym.to_string());
        }
    });
}

/// 简化状态灯（布尔）
fn status_dot_bare(ui: &mut egui::Ui, ok: bool) {
    let c = if ok {
        theme::cur().up
    } else {
        theme::cur().err
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, c);
}

impl eframe::App for BnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 主题/语言跟随配置（设置页或顶栏修改后即时生效）
        {
            let cfg = self.state.cfg.read();
            if self.applied_theme != cfg.theme_mode {
                theme::set_mode_str(&cfg.theme_mode);
                theme::apply(ctx);
                self.applied_theme = cfg.theme_mode.clone();
            }
            if self.applied_lang != cfg.lang {
                crate::i18n::set_lang(if cfg.lang == "en" {
                    crate::i18n::EN
                } else {
                    crate::i18n::ZH
                });
                self.applied_lang = cfg.lang.clone();
            }
        }
        match self.phase {
            Phase::Main => self.main_ui(ctx),
            _ => unlock::show(self, ctx),
        }
        widgets::draw_toasts(ctx, &self.state);
        ctx.request_repaint_after(Duration::from_millis(400));
    }
}
