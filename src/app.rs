//! 应用主体：启动流程路由（设置密码/解锁/配置密钥/主界面）与交易终端布局

use crate::binance::client::OrderRequest;
use crate::binance::{Interval, Market, Side};
use crate::config::{self, AppConfig};
use crate::engine::state::SharedState;
use crate::engine::{Command, Engine};
use crate::security::vault;
use zeroize::Zeroizing;
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
    /// 主密码/密钥输入缓冲：Zeroizing 保证 drop 时清零堆内存，
    /// 与全链路（Creds/客户端 secret）的清零纪律一致
    pub master: Option<Zeroizing<String>>,
    pub pw1: Zeroizing<String>,
    pub pw2: Zeroizing<String>,
    pub pw_err: String,
    pub api_key: Zeroizing<String>,
    pub api_secret: Zeroizing<String>,
    pub keys_err: String,

    /// 当前顶层页签
    pub tab: Tab,
    /// 已应用的主题/语言（用于配置变更时重新应用）
    pub applied_theme: String,
    pub applied_lang: String,
    /// 当前交易市场（随交易页签切换）
    pub market: Market,
    /// 活动交易页状态：交易页签内唯一活跃副本，切页时与 spot/fut 整体 swap。
    /// 不做逐字段镜像同步——新增字段只改 TradingPage 本身，不存在漏同步
    pub page: TradingPage,
    pub confirm: Option<ConfirmOrder>,
    pub settings: SettingsDraft,

    /// 现货/期货页各自的持久状态
    pub spot: TradingPage,
    pub fut: TradingPage,
    /// 侧栏符号列表缓存（市场，过滤词大写，结果）：每帧重建 2600 个字符串克隆太浪费
    pub symbols_cache: Option<(Market, String, std::sync::Arc<Vec<String>>)>,
    /// 进入主界面后的首帧账户数据加载标记（历史），解锁/存钥后重新触发
    pub initial_load_done: bool,
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
            pw1: Zeroizing::default(),
            pw2: Zeroizing::default(),
            pw_err: String::new(),
            api_key: Zeroizing::default(),
            api_secret: Zeroizing::default(),
            keys_err: String::new(),
            tab: Tab::Spot,
            applied_theme: cfg.theme_mode.clone(),
            applied_lang: cfg.lang.clone(),
            market: Market::Spot,
            page: TradingPage::new(default_symbol.clone(), interval),
            confirm: None,
            settings: SettingsDraft::from_cfg(&cfg),
            spot: TradingPage::new(default_symbol.clone(), interval),
            fut: TradingPage::new(default_symbol.clone(), interval),
            symbols_cache: None,
            initial_load_done: false,
            // 引擎启动时已订阅现货默认交易对
            active_sub: Some((Market::Spot, default_symbol, interval)),
            vault_delete_armed: false,
            // 首帧定位用：收紧内边距后 EnterKeys 表单实测约 340，Unlock 约 240。
            // 首帧用两页的中间值，第二帧起由实测值收敛，避免开场跳一下
            auth_card_h: 300.0,
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
        page.symbol = self.page.symbol.clone();
        page.interval = self.page.interval;
        page.symbol_filter = self.page.symbol_filter.clone();
        page.form = self.page.form.clone();
        page.chart = self.page.chart.clone();
        page.sub_tab = self.page.sub_tab;
        page.leverage_input = self.page.leverage_input.clone();
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
        self.page.symbol = page.symbol;
        self.page.interval = page.interval;
        self.page.symbol_filter = page.symbol_filter;
        self.page.form = page.form;
        self.page.chart = page.chart;
        self.page.sub_tab = page.sub_tab;
        self.page.leverage_input = page.leverage_input;

        self.subscribe(m, self.page.symbol.clone(), self.page.interval);
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
        if self.page.symbol == s {
            return;
        }
        self.page.symbol = s.clone();
        // 表单残留清空 + 图表回到最新
        self.page.form.price.clear();
        self.page.form.qty.clear();
        self.page.form.trigger.clear();
        self.page.chart.right_off = 0.0;
        self.subscribe(self.market, s.clone(), self.page.interval);
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
        if self.page.interval == iv {
            return;
        }
        self.page.interval = iv;
        // 新快照只有 500 根，保留旧平移偏移会把图表卡在 1 根 K 线上
        self.page.chart.right_off = 0.0;
        let symbol = self.page.symbol.clone();
        self.subscribe(self.market, symbol, iv);
    }

    // ---------- 布局 ----------

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        // 进入主界面首帧（含解锁/存钥后）：ReloadAll 已覆盖挂单与账户，
        // 成交/订单历史需要指定交易对，这里补发当前页的
        if !self.initial_load_done {
            self.initial_load_done = true;
            if self.state.has_creds() {
                self.state.send_cmd(Command::RefreshHistory {
                    market: self.market,
                    symbol: self.page.symbol.clone(),
                });
            }
        }
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
                    theme::set_mode_str(&cfg.theme_mode);
                    theme::apply(ui.ctx());
                    self.applied_theme = cfg.theme_mode.clone();
                    // 同步进设置草稿：否则设置页"保存并重连"会用旧草稿把主题切换回滚
                    self.settings.theme_mode = cfg.theme_mode.clone();
                    // 落盘走引擎命令，UI 线程不做同步磁盘 IO
                    *self.state.cfg.write() = cfg;
                    self.state.send_cmd(Command::PersistConfig);
                }
                if ui
                    .small_button(egui::RichText::new(lang_txt).size(12.0))
                    .on_hover_text(crate::i18n::tr("切换语言"))
                    .clicked()
                {
                    let mut cfg = self.state.cfg.read().clone();
                    let to_en = cfg.lang == "en";
                    cfg.lang = if to_en { "zh".into() } else { "en".into() };
                    crate::i18n::set_lang(if cfg.lang == "en" {
                        crate::i18n::EN
                    } else {
                        crate::i18n::ZH
                    });
                    self.applied_lang = cfg.lang.clone();
                    // 同步进设置草稿：否则设置页"保存并重连"会用旧草稿把语言切换回滚
                    self.settings.lang = cfg.lang.clone();
                    // 落盘走引擎命令，UI 线程不做同步磁盘 IO
                    *self.state.cfg.write() = cfg;
                    self.state.send_cmd(Command::PersistConfig);
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
                crate::ui::symbols::show(self, ui);
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

        // 底部订单区（顶层面板，横贯全宽）
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

        // 中央：页头 + 图表
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().bg)
                    .inner_margin(8.0),
            )
            .show(ui, |ui| {
                crate::ui::market::show_header(self, ui);
                ui.add_space(2.0);
                chart::show(self, ui);
            });
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
                if ui.selectable_label(self.page.sub_tab == t, label).clicked() {
                    self.page.sub_tab = t;
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
            .show(ui, |ui| match self.page.sub_tab {
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
        // Esc 取消：真实资金操作必须有键盘退出路径
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.confirm = None;
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
                        // busy（含入队未执行）时禁用提交：防止重复点击发出双倍订单
                        let placing = self.state.is_busy("place");
                        let submit_btn = ui.add_enabled(
                            !placing,
                            egui::Button::new(
                                egui::RichText::new(crate::i18n::tr("确认提交")).strong(),
                            ),
                        );
                        if submit_btn.clicked() {
                            submit = true;
                        }
                        let cancel_btn = ui.button(crate::i18n::tr("取消"));
                        // 初始焦点落在取消上：回车/空格默认走安全路径
                        cancel_btn.request_focus();
                        if cancel_btn.clicked() {
                            cancel = true;
                        }
                    });
                }
            });
        if submit {
            let c = self.confirm.take().unwrap();
            // 入队即置忙（引擎负责释放）：覆盖"下单命令排队在被卡住的刷新
            // 之后"的窗口，期间提交按钮禁用，防止双击双单
            self.state.set_busy("place", true);
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
        // 开发辅助：BTP_DEMO=1 跳过解锁直接进主界面，便于无凭据调试/截图（仅 debug 构建编译）
        #[cfg(debug_assertions)]
        if self.phase != Phase::Main
            && std::env::var("BTP_DEMO").ok().as_deref() == Some("1")
        {
            self.phase = Phase::Main;
        }
        if !matches!(self.phase, Phase::Main) {
            // 离开主界面（解锁页/换钥页）后重新进入时需要再做一次初始加载
            self.initial_load_done = false;
        }
        match self.phase {
            Phase::Main => self.main_ui(ui),
            _ => unlock::show(self, ui),
        }
        widgets::draw_toasts(&ctx, &self.state);
        ctx.request_repaint_after(Duration::from_millis(400));
    }
}
