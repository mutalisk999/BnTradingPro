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
            Tab::Portfolio => crate::i18n::tr("资产总览"),
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
    pub form: trade::OrderForm,
    pub chart: chart::ChartState,
    pub leverage_input: String,
    pub sub_tab: SubTab,
    pub confirm: Option<ConfirmOrder>,
    pub settings: SettingsDraft,

    /// 现货/期货页各自的持久状态
    pub spot: TradingPage,
    pub fut: TradingPage,
    /// 引擎当前唯一的行情订阅（后订阅替换前订阅），用于去重
    pub active_sub: Option<(Market, String, Interval)>,
    /// 删除凭据库的两步确认（第一次点击进入待确认态）
    pub vault_delete_armed: bool,
    /// 认证卡片上一帧实测高度（矮窗口下卡片居中/顶置的布局依据，滞后一帧收敛）
    pub auth_card_h: f32,
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
            form: trade::OrderForm::default(),
            chart: chart::ChartState::default(),
            leverage_input: String::new(),
            sub_tab: SubTab::Open,
            confirm: None,
            settings: SettingsDraft::from_cfg(&cfg),
            spot: TradingPage::new(default_symbol.clone(), interval),
            fut: TradingPage::new(default_symbol.clone(), interval),
            // 引擎启动时已订阅现货默认交易对
            active_sub: Some((Market::Spot, default_symbol, interval)),
            vault_delete_armed: false,
            auth_card_h: 420.0,
        }
    }

    // ---------- 页签切换（镜像保存/恢复） ----------

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
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

    /// 订阅行情（引擎只有一条行情流，后订阅替换前订阅，因此按"当前活跃订阅"去重；
    /// 若按市场分别记录，切页返回时会误判为已订阅而漏发，行情将冻结）
    fn subscribe(&mut self, market: Market, symbol: String, interval: Interval) {
        let same = self
            .active_sub
            .as_ref()
            .map(|(m, s, i)| *m == market && *s == symbol && *i == interval)
            .unwrap_or(false);
        if !same {
            self.state.send_cmd(Command::Subscribe {
                market,
                symbol: symbol.clone(),
                interval,
            });
            self.active_sub = Some((market, symbol, interval));
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

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_bar")
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel)
                    .stroke(egui::Stroke::new(1.0_f32, theme::cur().border))
                    .inner_margin(egui::Margin::symmetric(14, 6)),
            )
            .show(ui, |ui| {
                ui.set_min_height(40.0);
                self.topbar(ui);
            });

        match self.tab {
            Tab::Spot | Tab::Futures => self.trading_ui(ui),
            Tab::Portfolio => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| crate::ui::assets::show(self, ui));
                });
            }
            Tab::Settings => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| settings::show(self, ui));
                });
            }
            Tab::Help => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| crate::ui::help::show(self, ui));
                });
            }
        }

        self.confirm_dialog(ui.ctx());
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
                // 语言按钮显示"目标语言"，只随语言状态变，与主题无关
                // （此前两个文字捆在同一个主题分支里，切主题时中/EN 会跟着翻转）
                let theme_txt = if theme::is_light() { "🌙" } else { "☀" };
                let lang_txt = if self.state.cfg.read().lang == "en" {
                    "中"
                } else {
                    "EN"
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
    fn trading_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("symbols_panel")
            .resizable(true)
            .default_size(185.0)
            .min_size(140.0)
            .max_size(320.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel)
                    .inner_margin(8),
            )
            .show(ui, |ui| {
                // 列表自身用 show_rows 虚拟化滚动，外层不再包 ScrollArea
                self.symbols_sidebar(ui);
            });

        egui::Panel::right("trade_panel")
            .resizable(true)
            .default_size(300.0)
            .min_size(250.0)
            .max_size(420.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel)
                    .inner_margin(8),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| trade::show(self, ui));
            });

        egui::Panel::right("book_panel")
            .resizable(true)
            .default_size(230.0)
            .min_size(180.0)
            .max_size(340.0)
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel)
                    .inner_margin(8),
            )
            .show(ui, |ui| {
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
            .show(ui, |ui| {
                self.page_header(ui);
                ui.add_space(2.0);

                egui::Panel::bottom("orders_panel")
                    .resizable(true)
                    .default_size(240.0)
                    .min_size(120.0)
                    .max_size(500.0)
                    .frame(
                        egui::Frame::default()
                            .fill(theme::cur().panel)
                            .inner_margin(8.0),
                    )
                    .show(ui, |ui| self.sub_tabs(ui));

                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::default()
                            .fill(theme::cur().bg)
                            .inner_margin(0.0),
                    )
                    .show(ui, |ui| chart::show(self, ui));
            });
    }

    /// 页头：交易对 / 属性ⓘ / 周期 / 最新价
    fn page_header(&mut self, ui: &mut egui::Ui) {
        let state = self.state.clone();
        let dp = state
            .with_rule(self.market, &self.symbol, |r| {
                r.map(|x| x.price_dp() as usize)
            })
            .unwrap_or(2);
        let (base, quote) = state
            .with_rule(self.market, &self.symbol, |r| {
                r.map(|x| (x.base.clone(), x.quote.clone()))
            })
            .unwrap_or((String::new(), String::new()));

        // 第一行：交易对 | 最新价/涨跌 | 周期直选按钮
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(self.symbol.clone()).strong().size(16.5));
            ui.separator();
            let tick = state
                .md
                .lock()
                .get(self.market, &self.symbol)
                .and_then(|md| md.ticker.clone());
            if let Some(t) = &tick {
                let chg: f64 = t.price_change_percent.to_string().parse().unwrap_or(0.0);
                let c = theme::signed_color(chg);
                let px: f64 = t.last_price.to_string().parse().unwrap_or(0.0);
                ui.label(
                    egui::RichText::new(widgets::fmt_thousands(px, dp))
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
            }
            ui.separator();
            // 周期直选（无需下拉）
            for iv in Interval::all() {
                let active = self.interval == iv;
                let txt = if active {
                    egui::RichText::new(iv.label())
                        .size(12.5)
                        .strong()
                        .color(theme::cur().accent)
                } else {
                    egui::RichText::new(iv.label())
                        .size(12.0)
                        .color(theme::cur().text_dim)
                };
                if ui.selectable_label(active, txt).clicked() {
                    self.switch_interval(iv);
                }
            }
        });

        // 第二行：24h OHLC / 交易量 / 交易额 /（合约）资金费率
        ui.horizontal(|ui| {
            let tick = state
                .md
                .lock()
                .get(self.market, &self.symbol)
                .and_then(|md| md.ticker.clone());
            if let Some(t) = tick {
                let stat = |ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32| {
                    ui.label(theme::card_label(label));
                    ui.label(
                        egui::RichText::new(value)
                            .monospace()
                            .size(12.0)
                            .color(color),
                    );
                    ui.add_space(4.0);
                };
                let o = widgets::fmt_thousands(t.open_price.to_string().parse().unwrap_or(0.0), dp);
                let h = widgets::fmt_thousands(t.high_price.to_string().parse().unwrap_or(0.0), dp);
                let l = widgets::fmt_thousands(t.low_price.to_string().parse().unwrap_or(0.0), dp);
                let c = widgets::fmt_thousands(t.last_price.to_string().parse().unwrap_or(0.0), dp);
                stat(ui, crate::i18n::tr("24h 开"), o, theme::cur().text);
                stat(ui, crate::i18n::tr("24h 高"), h, theme::cur().up);
                stat(ui, crate::i18n::tr("24h 低"), l, theme::cur().down);
                stat(ui, crate::i18n::tr("24h 收"), c, theme::cur().text);
                ui.separator();
                let base_txt = if base.is_empty() {
                    String::new()
                } else {
                    format!(" {base}")
                };
                let quote_txt = if quote.is_empty() {
                    String::new()
                } else {
                    format!(" {quote}")
                };
                stat(
                    ui,
                    crate::i18n::tr("24h 量"),
                    format!(
                        "{}{base_txt}",
                        widgets::fmt_compact(t.volume.to_string().parse().unwrap_or(0.0))
                    ),
                    theme::cur().text,
                );
                stat(
                    ui,
                    crate::i18n::tr("24h 额"),
                    format!(
                        "{}{quote_txt}",
                        widgets::fmt_compact(t.quote_volume.to_string().parse().unwrap_or(0.0))
                    ),
                    theme::cur().text,
                );
            }
            // 合约：资金费率（正/负着色 + 显式符号）
            if self.market == crate::binance::Market::UmFutures {
                let funding = state
                    .md
                    .lock()
                    .get(self.market, &self.symbol)
                    .and_then(|md| md.mark.clone())
                    .and_then(|m| m.funding_rate);
                if let Some(r) = funding {
                    let rf = r.to_string().parse::<f64>().unwrap_or(0.0);
                    let color = if rf > 0.0 {
                        theme::cur().up
                    } else if rf < 0.0 {
                        theme::cur().down
                    } else {
                        theme::cur().text_dim
                    };
                    ui.separator();
                    ui.label(theme::card_label(crate::i18n::tr("资金费率")));
                    ui.label(
                        egui::RichText::new(format!("{rf:+.4}%"))
                            .monospace()
                            .size(12.0)
                            .strong()
                            .color(color),
                    );
                }
            }
        });
        ui.separator();
    }

    fn symbols_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.symbol_filter)
                .id_salt(egui::Id::new("sidebar_search"))
                .hint_text(crate::i18n::tr("搜索交易对…"))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(2.0);

        let filter = self.symbol_filter.trim().to_uppercase();
        let favs_all = self.state.favorites(self.market);
        let mut switched: Option<String> = None;
        let mut toggled: Option<String> = None;

        // 自选区（置顶）
        let favs: Vec<String> = favs_all
            .iter()
            .filter(|s| filter.is_empty() || s.contains(&filter))
            .cloned()
            .collect();
        if !favs.is_empty() {
            theme::section(ui, crate::i18n::tr("⭐ 自选"));
            for sym in &favs {
                symbol_row(ui, self, sym, true, &mut switched, &mut toggled);
            }
            ui.add_space(2.0);
        }

        // 全部交易对：只借读规则簿、克隆过滤后的符号列表（字符串克隆成本远低于
        // 整本规则簿深克隆）。列表用 show_rows 虚拟化——每帧只构造可见行的控件，
        // 现货全量约 2600 个交易对也能流畅滚动，因此不再需要截断上限
        let all_syms: Vec<String> = {
            let rules = self.state.rules.read();
            let book = &rules[self.market as usize];
            book.symbols
                .iter()
                .filter(|s| filter.is_empty() || s.contains(&filter))
                .filter(|s| !favs.contains(s))
                .cloned()
                .collect()
        };
        theme::section(ui, crate::i18n::tr("全部"));
        // 槽位高度 ≥ 行内控件自然高度（星标按钮约 24px），避免行间点击区重叠
        const SYMBOL_ROW_H: f32 = 24.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, SYMBOL_ROW_H, all_syms.len(), |ui, range| {
                // 每行放进精确槽位：不依赖控件自然高度（声明行距与实际行距
                // 不一致时，内容会随滚动深度漂移、与滚动条错位）
                let x_range = ui.max_rect().x_range();
                let top = ui.max_rect().top();
                let start = range.start;
                for i in range {
                    let y0 = top + (i - start) as f32 * SYMBOL_ROW_H;
                    let rect = egui::Rect::from_x_y_ranges(x_range, y0..=(y0 + SYMBOL_ROW_H));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |row_ui| {
                        symbol_row(
                            row_ui,
                            self,
                            &all_syms[i],
                            false,
                            &mut switched,
                            &mut toggled,
                        );
                    });
                }
            });
        if all_syms.is_empty() && favs.is_empty() {
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

    /// 交易页底部子页签：挂单 / 持仓(期货) / 历史 / 成交
    fn sub_tabs(&mut self, ui: &mut egui::Ui) {
        let mut tabs: Vec<(SubTab, &str)> = vec![
            (SubTab::Open, crate::i18n::tr("当前挂单")),
            (SubTab::History, crate::i18n::tr("订单历史")),
            (SubTab::Trades, crate::i18n::tr("成交历史")),
        ];
        if self.market == Market::UmFutures {
            tabs.insert(1, (SubTab::Positions, crate::i18n::tr("持仓")));
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
                    egui::RichText::new(format!("{}{}", open_n, crate::i18n::tr("笔挂单")))
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
                    let word = if c.side == Side::Buy {
                        crate::i18n::tr("买入")
                    } else {
                        crate::i18n::tr("卖出")
                    };
                    let testnet = self.state.cfg.read().testnet;
                    let header = if testnet {
                        format!(
                            "{}{}{}",
                            crate::i18n::tr("【测试网】"),
                            word,
                            crate::i18n::tr("确认")
                        )
                    } else {
                        format!(
                            "{}{} · {}",
                            crate::i18n::tr("【实盘】"),
                            word,
                            crate::i18n::tr("确认 · 真实资金！")
                        )
                    };
                    ui.label(egui::RichText::new(header).strong().color(if testnet {
                        theme::cur().accent
                    } else {
                        theme::cur().err
                    }));
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
    // 行悬停高亮：先在控件下层铺底色，再放按钮
    let row_rect =
        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 22.0));
    let hovered = ui.rect_contains_pointer(row_rect);
    if hovered && !selected {
        ui.painter()
            .rect_filled(row_rect, 3.0, theme::cur().row_hover);
    }
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
            .corner_radius(3)
            .min_size(egui::vec2(ui.available_width() - 26.0, 20.0));
        if ui
            .add(name_btn)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
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
                    .corner_radius(3),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
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
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, c);
    resp.on_hover_text(if ok {
        crate::i18n::tr("REST 接口正常")
    } else {
        crate::i18n::tr("REST 接口异常")
    });
}

impl eframe::App for BnApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone(); // Context 内部是 Arc，克隆廉价；避免与 ui 的可变借用冲突
                                    // 主题/语言跟随配置（设置页或顶栏修改后即时生效）
        {
            let cfg = self.state.cfg.read();
            if self.applied_theme != cfg.theme_mode {
                theme::set_mode_str(&cfg.theme_mode);
                theme::apply(&ctx);
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
            Phase::Main => self.main_ui(ui),
            _ => unlock::show(self, ui),
        }
        widgets::draw_toasts(&ctx, &self.state);
        ctx.request_repaint_after(Duration::from_millis(400));
    }
}
