//! 应用主体：启动流程路由（设置密码/解锁/配置密钥/主界面）与交易终端布局

use crate::binance::client::OrderRequest;
use crate::binance::{FutPosSide, Interval, Market, Side};
use crate::config::{self, AppConfig};
use crate::engine::state::SharedState;
use crate::engine::Command;
use crate::security::vault;
use crate::ui::{chart, orders, settings, theme, trade, unlock, widgets};
use eframe::egui;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

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
    Flow,
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
    /// 图表副图指标（成交量/MACD/RSI…，随交易页镜像保存）
    pub sub_indicator: chart::SubIndicator,
    /// 盘口聚合档位在候选步长中的下标（0 = 原始 tick 不聚合）
    pub book_group: usize,
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
            sub_indicator: chart::SubIndicator::Vol,
            book_group: 0,
        }
    }
}

pub struct ConfirmOrder {
    pub market: Market,
    /// 主单在前，附加止盈/止损随后（一次确认、顺序提交）
    pub reqs: Vec<OrderRequest>,
    /// 现货 OCO：非空时忽略 reqs，提交单条 PlaceOco
    pub oco: Option<crate::binance::client::OcoRequest>,
    pub side: Side,
    pub rows: Vec<(String, String)>,
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
    /// 涨跌颜色方向：green_up / red_up
    pub price_dir: String,
    pub favorites_spot: Vec<String>,
    pub favorites_futures: Vec<String>,
    /// 合约下单走 WS-API 通道（失败自动回退 REST）
    pub ws_order: bool,
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
            price_dir: cfg.price_dir.clone(),
            favorites_spot: cfg.favorites_spot.clone(),
            favorites_futures: cfg.favorites_futures.clone(),
            ws_order: cfg.ws_order,
        }
    }

    pub fn into_cfg(self) -> AppConfig {
        AppConfig {
            testnet: self.testnet,
            // trim 后为空会发出非法订阅请求，回退默认交易对
            default_symbol: {
                let s = self.default_symbol.trim().to_uppercase();
                if s.is_empty() {
                    "BTCUSDT".to_string()
                } else {
                    s
                }
            },
            default_interval: self.default_interval,
            recv_window_ms: self.recv_window_ms,
            http_timeout_secs: self.http_timeout_secs,
            log_level: self.log_level,
            lang: self.lang,
            theme_mode: self.theme_mode,
            price_dir: self.price_dir,
            favorites_spot: self.favorites_spot,
            favorites_futures: self.favorites_futures,
            ws_order: self.ws_order,
            override_spot_rest: self.override_spot_rest,
            override_spot_ws: self.override_spot_ws,
            override_fut_rest: self.override_fut_rest,
            override_fut_ws: self.override_fut_ws,
            proxy: crate::config::ProxyConfig {
                enabled: self.proxy_enabled,
                url: self.proxy_url,
            },
            // 预警不在设置页编辑：调用方（settings 保存）会以 live 值覆盖此空表
            alerts: Vec::new(),
        }
    }
}

pub struct BnApp {
    pub state: Arc<SharedState>,

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
    /// 侧栏符号列表缓存（市场，过滤词大写，规则代次，结果）
    pub symbols_cache: Option<(Market, String, u64, std::sync::Arc<Vec<String>>)>,
    /// 进入主界面后的首帧账户数据加载标记（历史），解锁/存钥后重新触发
    pub initial_load_done: bool,
    /// 引擎当前唯一的行情订阅（后订阅替换前订阅），用于去重
    pub active_sub: Option<(Market, String, Interval)>,
    /// 删除凭据库的两步确认（第一次点击进入待确认态）
    pub vault_delete_armed: bool,
    /// 「撤销本市场全部」两步确认（第一次点击进入待确认态，点击其他位置解除）
    pub market_cancel_armed: bool,
    /// 「撤销全部（当前交易对）」两步确认：武装的交易对名，点击其他位置解除
    pub sym_cancel_armed: Option<String>,
    /// 行级「撤销」两步确认：武装的订单号，点击其他位置解除
    pub row_cancel_armed: Option<i64>,
    /// Ctrl+Shift+C 连按确认：(交易对, 武装时刻)，4 秒窗口内第二次按下才执行
    pub shortcut_cancel_armed: Option<(String, std::time::Instant)>,
    /// 持仓行内 TP/SL 弹窗：(symbol, 多头?, 持仓方向, 止损触发价, 止盈触发价)
    pub tpsl_edit: Option<(String, bool, FutPosSide, String, String)>,
    /// 价格预警弹窗开关（页头铃铛）
    pub alerts_open: bool,
    /// 新预警目标价输入缓冲
    pub alert_input: String,
    /// 认证卡片上一帧实测高度（矮窗口下卡片居中/顶置的布局依据，滞后一帧收敛）
    pub auth_card_h: f32,
    /// 左侧交易对列表展开（false 时收起为窄条，» / « 按钮切换）
    pub sidebar_expanded: bool,
    /// 左侧交易对列表宽度（pt），右缘把手拖动调节，范围 190~240
    pub sidebar_width: f32,
    /// 盘口面板宽度（pt），右缘把手拖动调节，范围 180~340。
    /// 用 exact_size 锁死而非 resizable：egui 会把面板内容宽持久化进
    /// PanelState，盘口/成交两页签内容宽不同，来回切换面板会越撑越宽
    pub book_width: f32,
    /// 图表最大化（隐藏右侧下单/盘口面板，仅保留页头 + 图表）
    pub chart_maximized: bool,
    /// 右侧下单面板展开（false 时收起为右缘窄条，» / « 按钮切换）
    pub trade_expanded: bool,
    /// 右侧下单面板宽度（pt），左缘把手拖动调节，范围 236~360
    pub trade_width: f32,
    /// 盘口逐档价格变动高亮（按 BookSite 分组，长度随 BOOK_MAX_LEVELS）
    pub flash_book: crate::ui::flash::FlashBook,
    /// 逐笔成交最新行价格变动高亮
    pub flash_trade: crate::ui::flash::FlashTrade,
    /// 页头最新价变动高亮
    pub flash_header: crate::ui::flash::FlashHeader,
    /// 盘口消失档位残影（淡出动画 + 上一帧价格集合）
    pub book_anim: crate::ui::market::BookAnim,
    /// 右栏内容页签：盘口 / 成交明细
    pub book_tab: crate::ui::market::BookTab,
    /// 资金流水页签进入时已自动拉取过当前市场（防每帧重复下发；
    /// 切市场时重置，重新触发一次）
    pub flow_auto_done: bool,
    /// 订单历史筛选：全部 / 仅基础单 / 仅条件单（引擎合并了 Algo 条件单
    /// 历史，默认两类都展示）
    pub hist_kind: crate::ui::orders::HistKind,
    /// 缩放防抖：上一帧窗口物理尺寸（zoom 切换不改变物理尺寸，仅 resize 改变）
    ppp_last_phys: Option<(f32, f32)>,
    /// 缩放防抖：物理尺寸自该时刻起未变（egui time 域，秒）
    ppp_stable_since: Option<f64>,
}

impl BnApp {
    pub fn new(cc: &eframe::CreationContext, cfg: AppConfig) -> Self {
        theme::apply_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        // 启动预置缩放（详见 theme::auto_ppp_initial 注释）：在首帧绘制前落位
        // 目标 ppi，否则窗口先以偏大的原生 DPI 渲染再骤缩
        if let Some(p) = theme::auto_ppp_initial(&cc.egui_ctx) {
            tracing::info!("启动预置 ppi: {p}");
            cc.egui_ctx.set_pixels_per_point(p);
        }

        let dir = config::data_dir();
        let state = Arc::new(SharedState::default());
        *state.cfg.write() = cfg.clone();
        crate::engine::spawn(cc.egui_ctx.clone(), Arc::clone(&state));

        let phase = if vault::vault_exists(dir) {
            Phase::Unlock
        } else {
            Phase::EnterKeys
        };
        let interval = Interval::from_str(&cfg.default_interval);
        theme::set_mode_str(&cfg.theme_mode);
        theme::set_price_dir_str(&cfg.price_dir);
        theme::apply(&cc.egui_ctx);
        crate::i18n::set_lang(if cfg.lang == "en" {
            crate::i18n::EN
        } else {
            crate::i18n::ZH
        });

        let default_symbol = cfg.default_symbol.to_uppercase();
        Self {
            state,
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
            market_cancel_armed: false,
            sym_cancel_armed: None,
            row_cancel_armed: None,
            shortcut_cancel_armed: None,
            tpsl_edit: None,
            alerts_open: false,
            alert_input: String::new(),
            // 首帧定位用：收紧内边距后 EnterKeys 表单实测约 340，Unlock 约 240。
            // 首帧用两页的中间值，第二帧起由实测值收敛，避免开场跳一下
            auth_card_h: 300.0,
            sidebar_expanded: true,
            sidebar_width: 190.0,
            book_width: 230.0,
            chart_maximized: false,
            trade_expanded: true,
            trade_width: 300.0,
            flash_book: Default::default(),
            flash_trade: Default::default(),
            flash_header: Default::default(),
            book_anim: Default::default(),
            book_tab: crate::ui::market::BookTab::Book,
            flow_auto_done: false,
            hist_kind: crate::ui::orders::HistKind::All,
            ppp_last_phys: None,
            ppp_stable_since: None,
        }
    }

    /// 下单面板的最小宽度：两市场统一 300（= 默认面板宽）。字号上调后
    /// 272 下合约表单的 TIF 胶囊/杠杆行/价格辅助钮过挤，300 保证完整
    /// 显示；分割线位置在现货/期货间保持一致
    fn min_trade_width(&self) -> f32 {
        300.0
    }

    // ---------- 价格预警 ----------

    /// 逐帧评估预警：最新价来自全市场迷你 ticker（约 1s 一批，覆盖任意
    /// 交易对），活跃交易对回退 ticker24。触发即 toast + 移除（一次性）
    fn check_alerts(&mut self) {
        let targets: Vec<(u8, String, rust_decimal::Decimal, bool)> = {
            let md = self.state.md.lock();
            let cfg = self.state.cfg.read();
            cfg.alerts
                .iter()
                .filter_map(|a| {
                    // market 合法性守卫：config.toml 可手编，market >= 2 的条目会
                    // 让 tickers[a.market as usize] 数组越界 panic（每帧崩溃点）
                    if a.market > 1 {
                        return None;
                    }
                    let last = md.tickers[a.market as usize]
                        .get(&a.symbol)
                        .map(|t| t.close)
                        .or_else(|| {
                            let m = if a.market == 0 {
                                crate::binance::Market::Spot
                            } else {
                                crate::binance::Market::UmFutures
                            };
                            md.get(m, &a.symbol)
                                .and_then(|s| s.ticker.as_ref())
                                .map(|t| t.last_price)
                        })?;
                    // 目标价与最新价都是 Decimal：直接比较，不经 f64 中转
                    // （f64 比较在边界值上可能误判 ±1 ulp）
                    let hit = if a.above {
                        last >= a.price
                    } else {
                        last <= a.price
                    };
                    hit.then(|| (a.market, a.symbol.clone(), a.price, a.above))
                })
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        {
            let mut cfg = self.state.cfg.write();
            for (market, symbol, price, above) in &targets {
                cfg.alerts.retain(|a| {
                    !(a.market == *market
                        && &a.symbol == symbol
                        && a.price == *price
                        && a.above == *above)
                });
            }
        }
        self.state.send_cmd(crate::engine::Command::PersistConfig);
        for (_market, symbol, price, above) in targets {
            let dir = if above { "≥" } else { "≤" };
            self.state.push_toast(
                crate::engine::state::ToastKind::Ok,
                format!("{} {} {}", symbol, dir, price),
            );
        }
    }

    /// 添加预警：方向按目标价相对最新价自动判定（目标在上方=上破，
    /// 在下方=下破）；目标价已越过节拍价时拒绝创建
    pub fn add_alert(&mut self) {
        let Some(price) = widgets::parse_dec(&self.alert_input) else {
            self.state.push_toast(
                crate::engine::state::ToastKind::Warn,
                crate::i18n::tr("请输入有效的目标价").to_string(),
            );
            return;
        };
        if price <= rust_decimal::Decimal::ZERO {
            self.state.push_toast(
                crate::engine::state::ToastKind::Warn,
                crate::i18n::tr("请输入有效的目标价").to_string(),
            );
            return;
        }
        let market = self.market;
        let symbol = self.page.symbol.clone();
        let last = self
            .state
            .ticker_of(market, &symbol)
            .map(|t| t.close)
            .or_else(|| {
                self.state
                    .md
                    .lock()
                    .get(market, &symbol)
                    .and_then(|s| s.ticker.as_ref().map(|t| t.last_price))
            });
        let Some(last) = last else {
            self.state.push_toast(
                crate::engine::state::ToastKind::Warn,
                crate::i18n::tr("行情加载中，请稍后再试").to_string(),
            );
            return;
        };
        let above = price >= last;
        {
            let mut cfg = self.state.cfg.write();
            cfg.alerts
                .retain(|a| !(a.market == market as u8 && a.symbol == symbol && a.price == price));
            cfg.alerts.push(crate::config::AlertConfig {
                market: market as u8,
                symbol: symbol.clone(),
                price,
                above,
            });
        }
        self.state.send_cmd(crate::engine::Command::PersistConfig);
        self.alert_input.clear();
        let dir = if above { "≥" } else { "≤" };
        self.state.push_toast(
            crate::engine::state::ToastKind::Info,
            format!(
                "{} {} {} {}",
                crate::i18n::tr("已添加预警 "),
                symbol,
                dir,
                price
            ),
        );
    }

    /// 删除一条预警（按下标）
    pub fn remove_alert(&mut self, idx: usize) {
        let removed = self.state.cfg.write().alerts.remove(idx);
        self.state.send_cmd(crate::engine::Command::PersistConfig);
        self.state.push_toast(
            crate::engine::state::ToastKind::Info,
            format!(
                "{} {} {} {}",
                crate::i18n::tr("已删除预警 "),
                removed.symbol,
                if removed.above { "≥" } else { "≤" },
                removed.price
            ),
        );
    }

    // ---------- 页签切换（活动页整体换入换出） ----------

    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        // 离开交易页签：活动页整体存回所属市场槽位
        match self.tab {
            Tab::Spot => std::mem::swap(&mut self.spot, &mut self.page),
            Tab::Futures => std::mem::swap(&mut self.fut, &mut self.page),
            _ => {}
        }
        self.tab = tab;
        // 换市场后盘口属于另一套数据，flash 与残影一并清空
        self.flash_reset();
        self.book_anim.clear();
        // 全撤待确认态不得跨市场残留：换市场后第一次点击应重新确认
        self.market_cancel_armed = false;
        self.sym_cancel_armed = None;
        self.row_cancel_armed = None;
        self.shortcut_cancel_armed = None;
        // 切换顶层页签退出图表最大化：资产/设置页没有图表，残留状态会让
        // 交易页在下次进入时仍处于全屏、找不到还原按钮以外的面板
        self.chart_maximized = false;
        // 进入交易页签：整体换出目标市场页状态。
        // 整体 swap 而非逐字段复制：新增字段只改 TradingPage，不存在漏同步
        if let Some(m) = tab.market() {
            match tab {
                Tab::Spot => std::mem::swap(&mut self.spot, &mut self.page),
                Tab::Futures => std::mem::swap(&mut self.fut, &mut self.page),
                _ => {}
            }
            self.market = m;
            // 不再按市场钳制面板宽度：现货/期货共用同一 trade_width，
            // 切换页签时图表右侧分割线保持原位（用户明确要求两市场一致）
            self.refresh_account_data(m);
            // 资金流水按市场查询：切市场后允许流水页签重新自动拉取
            self.flow_auto_done = false;
        }
    }

    /// 切到某交易市场后刷新其私有数据（挂单 + 历史；合约再拉账户）
    fn refresh_account_data(&mut self, m: Market) {
        self.subscribe(m, self.page.symbol.clone(), self.page.interval);
        if self.state.has_creds() {
            self.state
                .send_cmd(Command::RefreshOpenOrders { market: m });
            self.state.send_cmd(Command::RefreshHistory {
                market: m,
                symbol: self.page.symbol.clone(),
            });
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
        // 表单残留清空 + 图表回到最新（校验红条一并清：其文案针对旧交易对）
        self.page.form.price.clear();
        self.page.form.qty.clear();
        self.page.form.trigger.clear();
        self.page.form.err.clear();
        self.page.chart.right_off = 0.0;
        // 旧价位点的"上一次值"属于上一个交易对，清空后避免被误判为变动
        self.flash_reset();
        // 残影同理：跨符号的价格集合必须清空
        self.book_anim.clear();
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
            // 关闭 egui 面板自带的内缘分隔线：它会叠在 Frame 描边外侧 0.5px，
            // 把导航栏下缘画成双线（粗细/颜色与上缘不一致的来源）。
            // 上下缘只剩 Frame 的 1pt border 描边，天然等粗同色
            .show_separator_line(false)
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel)
                    .stroke(egui::Stroke::new(1.0_f32, theme::cur().border))
                    // 纵向内边距 4：内容（28pt 按钮/页签）+ 8 = 36pt 总高，
                    // 比此前 46pt 省 10pt 给图表/订单区
                    .inner_margin(egui::Margin::symmetric(14, 4)),
            )
            .show(ui, |ui| {
                // 不设 min_height：面板高 = 行内容高 + 上下各 4 边距，
                // 按键到上下两线的距离必然相等（min_height 大于内容时
                // egui 内容顶对齐，多余空间全堆在下面，正是间距不均的根源）
                self.topbar(ui);
            });

        match self.tab {
            Tab::Spot | Tab::Futures => self.trading_ui(ui),
            Tab::Portfolio => {
                egui::CentralPanel::default().show(ui, |ui| {
                    // 三个顶层页的 ScrollArea 显式区隔 Id：CentralPanel 子 ui 的
                    // 稳定 Id 与页签无关，默认 salt 会让三页共享一个滚动状态——
                    // 设置页滚到底再切到资产页时仍停在滚动偏移处
                    egui::ScrollArea::vertical()
                        .id_salt("assets_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| crate::ui::assets::show(self, ui));
                });
            }
            Tab::Settings => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("settings_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| settings::show(self, ui));
                });
            }
            Tab::Help => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("help_scroll")
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
            // 品牌：与页签按钮同一行内垂直居中（不再手动 add_space 下移，
            // horizontal 布局默认 Align::Center 已保证中线等高）
            ui.label(
                egui::RichText::new("BnTradingPro")
                    .strong()
                    .color(theme::cur().accent)
                    .size(theme::FONT_XL),
            );
            ui.add_space(12.0);

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
                // 主题按钮用文字（显示目标主题）：🌙/☀ 属 emoji，等宽字体优先时缺字风险高
                let theme_txt = if theme::is_light() {
                    crate::i18n::tr("深色")
                } else {
                    crate::i18n::tr("浅色")
                };
                let lang_txt = if self.state.cfg.read().lang == "en" {
                    "中"
                } else {
                    "EN"
                };
                if theme::btn_default(ui, theme_txt, true)
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
                if theme::btn_default(ui, lang_txt, true)
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
                // 连接状态：每项 = 「名称 + 状态灯」紧邻成组，读作
                // 「账户 ● 行情 ● REST ●」；RTL 容器内先添加的更靠右，
                // 故按 REST → 行情 → 账户 的逆序添加。状态详情（连接中/
                // 未连接/错误原因）收进各项悬停提示
                let rest_ok = self
                    .state
                    .rest_ok
                    .load(std::sync::atomic::Ordering::Relaxed);
                widgets::status_light_ok(
                    ui,
                    crate::i18n::tr("REST"),
                    rest_ok,
                    crate::i18n::tr("REST 接口正常"),
                    crate::i18n::tr("REST 接口异常"),
                );
                if let Some(m) = self.tab.market() {
                    let mws = self.state.md.lock().market_ws(m).clone();
                    widgets::status_light(ui, crate::i18n::tr("行情"), &mws);
                    if self.state.has_creds() {
                        let uws = self.state.md.lock().user_ws(m).clone();
                        widgets::status_light(ui, crate::i18n::tr("账户"), &uws);
                    } else {
                        // 未配置密钥：保持「名称 + 状态」结构，用警示文字代替状态灯
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(
                                egui::RichText::new(crate::i18n::tr("账户"))
                                    .size(theme::FONT_SM)
                                    .color(theme::cur().text_dim),
                            );
                            ui.label(
                                egui::RichText::new(crate::i18n::tr("未配置密钥"))
                                    .size(theme::FONT_SM)
                                    .color(theme::cur().warn),
                            );
                        });
                    }
                }
            });
        });
    }

    /// 交易页：左侧交易对列表 + 页头 + 图表 | 盘口+成交 | 下单面板 + 底部订单区
    fn trading_ui(&mut self, ui: &mut egui::Ui) {
        // ---------- 快捷键 ----------
        // B/S 切换买卖侧并聚焦数量框（输入框聚焦时不抢键）；
        // Ctrl+Shift+C 连按两次撤销当前交易对全部挂单：单次和弦键误按风险高，
        // 与按钮路径同一两步纪律——第一击武装（4 秒窗口内）并提示，
        // 第二击才真正执行
        if self.confirm.is_none() {
            let (b, s, ctrl_shift_c) = ui.ctx().input(|i| {
                (
                    i.key_pressed(egui::Key::B),
                    i.key_pressed(egui::Key::S),
                    i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::C),
                )
            });
            let editing = ui.ctx().memory(|m| m.focused().is_some());
            if !editing {
                if b {
                    self.page.form.side = Side::Buy;
                    ui.ctx()
                        .memory_mut(|m| m.request_focus(egui::Id::new("qty_in")));
                }
                if s {
                    self.page.form.side = Side::Sell;
                    ui.ctx()
                        .memory_mut(|m| m.request_focus(egui::Id::new("qty_in")));
                }
            }
            if ctrl_shift_c && self.state.has_creds() {
                let sym = self.page.symbol.clone();
                let now = std::time::Instant::now();
                // 武装态按 (交易对, 4 秒窗口) 匹配：换交易对或超时即视为
                // 首次按下，重新武装，杜绝"多久之前误按一次后被补键撤销"
                let primed = matches!(
                    self.shortcut_cancel_armed,
                    Some((ref s, t)) if *s == sym && now.duration_since(t) < Duration::from_secs(4)
                );
                if primed {
                    self.shortcut_cancel_armed = None;
                    self.state.send_cmd(Command::CancelAll {
                        market: self.market,
                        symbol: Some(sym),
                    });
                } else {
                    self.shortcut_cancel_armed = Some((sym, now));
                    self.state.push_toast(
                        crate::engine::state::ToastKind::Warn,
                        format!(
                            "{}{}",
                            crate::i18n::tr("再按一次 Ctrl+Shift+C 确认撤销: "),
                            self.page.symbol
                        ),
                    );
                }
            }
        }

        // 侧栏宽度按用户指定 180~240（默认 180）；egui 0.36 自带的 Panel 拖拽把手
        // 在展开态不吃手势（实测），改用自绘把手：内容区右缘 8px 竖条直接驱动宽度
        if self.sidebar_expanded {
            let panel = egui::Panel::left("symbols_panel")
                .exact_size(self.sidebar_width)
                .frame(
                    egui::Frame::default()
                        .fill(theme::cur().panel)
                        .inner_margin(8),
                )
                .show(ui, |ui| {
                    // 收起按钮与搜索框同排：allocate_ui 把行区显式钳在 26px 高
                    // （直接 with_layout 的子区会吞掉面板剩余高度，内容被
                    // 居中到面板中部、顶部留大空白），Align::Center 让两控件
                    // 互相垂直居中；symbol_filter 在 app.page 上，折叠不丢状态
                    ui.allocate_ui(
                        egui::vec2(ui.available_width(), crate::ui::theme::CONTROL_H),
                        |ui| {
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button("«")
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text(crate::i18n::tr("收起侧边栏"))
                                        .clicked()
                                    {
                                        self.sidebar_expanded = false;
                                    }
                                    crate::ui::symbols::show_search(self, ui);
                                },
                            );
                        },
                    );
                    ui.add_space(2.0);
                    // 列表自身用 show_rows 虚拟化滚动，外层不再包 ScrollArea
                    crate::ui::symbols::show(self, ui);
                });

            // 拖拽把手：注册在外层 ui（裁剪区=整窗）。内容 ui 的裁剪只到 Frame 内边距，
            // 在里面注册边缘把手会被裁掉而永远收不到手势。把手 = 面板外缘内侧 8px
            let outer = panel.response.rect;
            let strip_rect = egui::Rect::from_min_max(
                egui::pos2(outer.right() - 8.0, outer.top()),
                egui::pos2(outer.right(), outer.bottom()),
            );
            let strip = ui.interact(
                strip_rect,
                egui::Id::new("sidebar_resize"),
                egui::Sense::click_and_drag(),
            );
            if strip.dragged() {
                // 指针右移 = 面板变宽（最小宽度 190 按用户要求）
                self.sidebar_width =
                    (self.sidebar_width + strip.drag_delta().x).clamp(190.0, 240.0);
            }
            strip.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        } else {
            // 折叠态：窄条面板只放展开按钮，中央区随之扩大
            egui::Panel::left("symbols_panel_collapsed")
                .exact_size(36.0)
                .frame(
                    egui::Frame::default()
                        .fill(theme::cur().panel)
                        .inner_margin(4),
                )
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        if ui
                            .small_button("»")
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(crate::i18n::tr("展开侧边栏"))
                            .clicked()
                        {
                            self.sidebar_expanded = true;
                        }
                    });
                });
        }

        // 图表最大化：右侧下单/盘口/成交与底部订单区全部隐藏，图表占满中央区
        // 下单面板左缘：中央图表的绘制右缘钳位（防越界叠画）；最大化无面板不钳
        let mut trade_left = f32::MAX;
        if !self.chart_maximized {
            // 盘口（左，全高）：一直显示到 UI 最下面，不被底部订单区挤上去。
            // 面板按显示顺序瓜分空间——盘口先于底部订单区显示才能拿到全高。
            // 宽度用 exact_size 锁死 + 自绘把手（同侧栏）：egui 可调面板会把
            // 内容宽度持久化，盘口/成交页签来回切面板会越撑越宽。
            // 内部结构（show_book_panel）：页签在顶、盘口在中间（下限档数
            // 溢出时滚动兜底）、档位胶囊钉在最下方
            let book = egui::Panel::left("book_panel")
                .exact_size(self.book_width)
                .frame(
                    egui::Frame::default()
                        .fill(theme::cur().panel)
                        .inner_margin(8),
                )
                .show(ui, |ui| {
                    crate::ui::market::show_book_panel(self, ui);
                });

            // 自绘拖宽把手：面板右缘内侧 8px 竖条，注册在外层 ui 上，
            // 避免内容区 ScrollArea 吃掉手势
            let outer = book.response.rect;
            let strip_rect = egui::Rect::from_min_max(
                egui::pos2(outer.right() - 8.0, outer.top()),
                egui::pos2(outer.right(), outer.bottom()),
            );
            let strip = ui.interact(
                strip_rect,
                egui::Id::new("book_resize"),
                egui::Sense::click_and_drag(),
            );
            if strip.dragged() {
                self.book_width = (self.book_width + strip.drag_delta().x).clamp(180.0, 340.0);
            }
            strip.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);

            // 底部订单区：盘口右边、K线与下单下面（在右侧面板之前显示，才能
            // 从盘口右缘一直铺到窗口右缘，持仓/挂单表格的列才放得下）
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

            // 右侧下单面板（与左侧交易对列表同构的折叠形态）：
            // 展开态顶部「»」收起、左缘拖宽把手；收起态退为右缘 36pt 窄条「«」展开。
            // 宽度同样用 exact_size 锁死 + 自绘把手：resizable 面板会把内容宽
            // 持久化进 PanelState，下单卡内外宽度不同来回切面板会越撑越宽
            if self.trade_expanded {
                let trade = egui::Panel::right("trade_panel")
                    .exact_size(self.trade_width)
                    .frame(
                        egui::Frame::default()
                            .fill(theme::cur().panel)
                            .inner_margin(8),
                    )
                    .show(ui, |ui| {
                        // 顶行：收起按钮居左，「下单 · <symbol>」居右——标题右缘
                        // 对齐下方卖出按钮右缘（买卖按钮行总宽=面板内容宽）。
                        // allocate_ui 把行区钳在 26px 高（直接 with_layout 会
                        // 吞掉面板剩余高度、内容被居中到面板中部）。表单状态
                        // 都在 app.page，折叠不丢输入
                        ui.allocate_ui(
                            egui::vec2(ui.available_width(), crate::ui::theme::CONTROL_H),
                            |ui| {
                                ui.with_layout(
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .small_button("»")
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text(crate::i18n::tr("收起下单面板"))
                                            .clicked()
                                        {
                                            self.trade_expanded = false;
                                        }
                                        let symbol = self.page.symbol.clone();
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{}{symbol}",
                                                        crate::i18n::tr("下单 · ")
                                                    ))
                                                    .strong()
                                                    .size(theme::FONT_BODY),
                                                );
                                            },
                                        );
                                    },
                                );
                            },
                        );
                        egui::ScrollArea::vertical().show(ui, |ui| trade::show(self, ui));
                    });
                // 图表右缘钳位用（见中央区 chart::show）
                trade_left = trade.response.rect.left();
                // 拖拽把手：注册在外层 ui（面板内 ui 的裁剪只到 Frame 内边距，
                // 在里面注册会被裁掉而收不到手势）。把手 = 面板左缘内侧 8px
                let outer = trade.response.rect;
                let strip_rect = egui::Rect::from_min_max(
                    egui::pos2(outer.left(), outer.top()),
                    egui::pos2(outer.left() + 8.0, outer.bottom()),
                );
                let strip = ui.interact(
                    strip_rect,
                    egui::Id::new("trade_resize"),
                    egui::Sense::click_and_drag(),
                );
                if strip.dragged() {
                    // 指针左移 = 面板变宽；下限两市场统一 272（分割线位置一致）
                    let min_w = self.min_trade_width();
                    self.trade_width =
                        (self.trade_width - strip.drag_delta().x).clamp(min_w, 360.0);
                }
                strip.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            } else {
                // 折叠态：右缘窄条只放展开按钮，中央区随之扩到右缘
                let collapsed = egui::Panel::right("trade_panel_collapsed")
                    .exact_size(36.0)
                    .frame(
                        egui::Frame::default()
                            .fill(theme::cur().panel)
                            .inner_margin(4),
                    )
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            if ui
                                .small_button("«")
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(crate::i18n::tr("展开下单面板"))
                                .clicked()
                            {
                                self.trade_expanded = true;
                            }
                        });
                    });
                trade_left = collapsed.response.rect.left();
            }
        }

        // 中央：页头 + 图表（right_limit = 下单面板左缘，防图表越界叠画）
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().bg)
                    .inner_margin(8.0),
            )
            .show(ui, |ui| {
                crate::ui::market::show_header(self, ui);
                ui.add_space(2.0);
                chart::show(self, ui, trade_left);
            });
    }

    /// 交易页底部子页签：挂单 / 持仓+资金流水(合约) / 历史 / 成交
    fn sub_tabs(&mut self, ui: &mut egui::Ui) {
        // 资金流水与持仓一样仅合约提供（现货无统一流水端点）：
        // 合约 = 挂单/持仓/历史/成交/流水，现货 = 挂单/历史/成交
        let mut tabs: Vec<(SubTab, &str)> = vec![
            (SubTab::Open, crate::i18n::tr("当前挂单")),
            (SubTab::History, crate::i18n::tr("订单历史")),
            (SubTab::Trades, crate::i18n::tr("成交历史")),
        ];
        if self.market == Market::UmFutures {
            tabs.insert(1, (SubTab::Positions, crate::i18n::tr("持仓")));
            tabs.push((SubTab::Flow, crate::i18n::tr("资金流水")));
        }
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            for (t, label) in tabs {
                if theme::pill_tab(ui, label, self.page.sub_tab == t).clicked() {
                    self.page.sub_tab = t;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let open_n = self.state.orders.lock().open(self.market).len();
                ui.label(
                    egui::RichText::new(format!("{}{}", open_n, crate::i18n::tr("笔挂单")))
                        .size(theme::FONT_SM)
                        .color(theme::cur().text_dim),
                );
            });
        });
        ui.separator();
        // 进入资金流水页签后自动拉取一次当前市场（切市场重置标记重新触发；
        // 手动「刷新」按钮随时可重拉，失败 toast 可见）
        if self.page.sub_tab == SubTab::Flow && !self.flow_auto_done {
            self.flow_auto_done = true;
            if self.state.has_creds() {
                self.state.send_cmd(Command::RefreshFlow {
                    market: self.market,
                });
            }
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.page.sub_tab {
                SubTab::Open => orders::show_open(self, ui),
                SubTab::Positions => orders::show_positions(self, ui),
                SubTab::History => orders::show_history(self, ui),
                SubTab::Trades => orders::show_trades_history(self, ui),
                SubTab::Flow => orders::show_flow(self, ui),
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
        // 全屏半透明遮罩：视觉聚焦弹窗 + 点击空白即取消（与 Esc 等效）。
        // 遮罩在 Middle 层，弹窗在更高的 Tooltip 层（Toast 在 Foreground，
        // 仍浮在弹窗之上）；Area 无内容，用 painter_at + allocate_rect 撑满全屏
        let screen = ctx.input(|i| i.raw.screen_rect).unwrap_or(egui::Rect::ZERO);
        egui::Area::new(egui::Id::new("confirm_dim"))
            .order(egui::Order::Middle)
            .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
            .show(ctx, |ui| {
                let alpha = if theme::is_light() { 70 } else { 120 };
                ui.painter_at(screen).rect_filled(
                    screen,
                    0.0,
                    egui::Color32::from_black_alpha(alpha),
                );
                if ui.allocate_rect(screen, egui::Sense::click()).clicked() {
                    cancel = true;
                }
            });
        // 弹窗描边随方向着色：买=涨色 / 卖=跌色，色即语义（与提交键同源）
        let side_color = match self.confirm.as_ref().map(|c| c.side) {
            Some(Side::Sell) => theme::cur().down,
            _ => theme::cur().up,
        };
        egui::Window::new(egui::RichText::new(crate::i18n::tr("确认下单")).strong())
            .open(&mut keep)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::default()
                    .fill(theme::cur().panel2)
                    .stroke(egui::Stroke::new(1.5_f32, side_color.gamma_multiply(0.55)))
                    .corner_radius(f32::from(theme::R_LG))
                    .inner_margin(egui::Margin::same(14)),
            )
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
                    // 警示行是弹窗真正的信息核心，层级必须压过装饰性标题
                    ui.label(
                        egui::RichText::new(header)
                            .strong()
                            .size(theme::FONT_LG)
                            .color(if testnet {
                                theme::cur().accent
                            } else {
                                theme::cur().err
                            }),
                    );
                    ui.separator();
                    egui::Grid::new("confirm_grid")
                        .num_columns(2)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            for (k, v) in &c.rows {
                                ui.label(egui::RichText::new(k).color(theme::cur().text_dim));
                                ui.label(
                                    egui::RichText::new(v)
                                        .monospace()
                                        .strong()
                                        .color(color)
                                        .variation(theme::WGHT, 700.0),
                                );
                                ui.end_row();
                            }
                        });
                    ui.add_space(6.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // busy（含入队未执行）时禁用提交：防止重复点击发出双倍订单
                        let placing = self.state.is_busy("place");
                        let (submit_fill, submit_txt) = if c.side == Side::Buy {
                            (theme::cur().up, crate::i18n::tr("确认买入"))
                        } else {
                            (theme::cur().down, crate::i18n::tr("确认卖出"))
                        };
                        let submit_btn = theme::btn_solid(ui, submit_txt, submit_fill, !placing);
                        let submit_btn = if placing {
                            submit_btn.on_disabled_hover_text(crate::i18n::tr("上一笔请求处理中…"))
                        } else {
                            submit_btn
                        };
                        if submit_btn.clicked() {
                            submit = true;
                        }
                        let cancel_btn = theme::btn_default(ui, crate::i18n::tr("取消"), true);
                        // 初始焦点落在取消上：回车/空格默认走安全路径；
                        // 仅当前无焦点控件时请求——每帧无条件抢占会让
                        // Tab 切换与 Enter 提交永远无法离开取消键
                        if ui.ctx().memory(|m| m.focused().is_none()) {
                            cancel_btn.request_focus();
                        }
                        if cancel_btn.clicked() {
                            cancel = true;
                        }
                    });
                }
            });
        if submit {
            let c = self.confirm.take().unwrap();
            // 入队即置忙（引擎负责释放）：覆盖"下单命令排队在被卡住的刷新
            // 之后"的窗口，期间提交按钮禁用，防止双击双单。
            // OCO 单条命令提交；多单（主单+附加止盈止损）顺序入队。
            // send_cmd 失败时（引擎命令循环已退出）由它统一报错，
            // 这里只需回滚 busy，否则提交键永久禁用
            self.state.set_busy("place", true);
            let mut ok = true;
            if let Some(oco) = c.oco {
                ok &= self.state.send_cmd(Command::PlaceOco { req: oco });
            } else {
                for req in c.reqs {
                    ok &= self.state.send_cmd(Command::PlaceOrder {
                        market: c.market,
                        req,
                    });
                }
            }
            if !ok {
                self.state.set_busy("place", false);
            }
        } else if !keep || cancel {
            self.confirm = None;
        }
    }
}

impl eframe::App for BnApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone(); // Context 内部是 Arc，克隆廉价；避免与 ui 的可变借用冲突
                                    // 本地价格预警逐帧评估（内部空表时即刻返回，无锁开销）
        self.check_alerts();
        // 响应式等比缩放（防抖）：最大化/恢复会产生一串 resize 事件，在过渡帧
        // （窗口高度骤减）应用 zoom 曾引发 chart 绘制的 clamp panic（已单独修复）；
        // 仍保留防抖：仅当窗口物理尺寸稳定 ≥0.25s 后才应用 zoom，使缩放切换
        // 与 resize 事件流错开，避免过渡帧下二次布局抖动
        {
            let rect = ctx.input(|i| i.raw.screen_rect).unwrap_or(egui::Rect::ZERO);
            let size = rect.size() * ctx.pixels_per_point();
            let now = ctx.input(|i| i.time);
            // 半像素内视为未变（逻辑 rect × ppi 换算存在舍入抖动）
            let moved = match self.ppp_last_phys {
                Some((lx, ly)) => (size.x - lx).abs() > 0.5 || (size.y - ly).abs() > 0.5,
                None => true,
            };
            if moved {
                self.ppp_last_phys = Some((size.x, size.y));
                self.ppp_stable_since = Some(now);
            }
            let stable = now - self.ppp_stable_since.unwrap_or(now) >= 0.25;
            if stable {
                if let Some(p) = theme::auto_ppi(&ctx) {
                    ctx.set_pixels_per_point(p);
                    ctx.request_repaint();
                }
            }
        }
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
        // 开发辅助：BTP_DEMO=1 跳过解锁直接进主界面，便于无凭据调试/截图（仅 debug 构建编译）；
        // 同时注入假资产快照，让资产总览/持仓等需要凭据的页面也有完整布局可看
        #[cfg(debug_assertions)]
        if self.phase != Phase::Main && std::env::var("BTP_DEMO").ok().as_deref() == Some("1") {
            self.phase = Phase::Main;
            // 演示模式：可用 BTP_DEMO_TAB 指定初始页（spot/futures/portfolio/settings），
            // 便于无凭据调试/截图各页布局；默认落在现货交易页（侧栏/盘口/下单全布局）
            let tab = std::env::var("BTP_DEMO_TAB")
                .ok()
                .map(|t| match t.as_str() {
                    "futures" => Tab::Futures,
                    "portfolio" => Tab::Portfolio,
                    "settings" => Tab::Settings,
                    "help" => Tab::Help,
                    _ => Tab::Spot,
                })
                .unwrap_or(Tab::Spot);
            self.tab = tab;
            Self::seed_demo_accounts(&self.state);
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
        // 分区重绘调度：取本帧所需的最短间隔。
        // egui 的 request_repaint_after 只能取一个值（多次调用以最短者为准），
        // 所以这里按"当前可见区域各自需要多快"取最小值，而非全局一刀切
        let mut next = theme::REPAINT_IDLE;
        match self.phase {
            Phase::Main => {
                if self.tab.market().is_some() {
                    // 行情区（盘口/页头/成交）@100ms；图表仅在可见时需要 30fps
                    next = next.min(theme::REPAINT_QUOTE);
                    if !self.chart_maximized {
                        next = next.min(theme::REPAINT_CHART);
                    }
                }
                // 闪烁余晖进行中时按图表档重绘，直到衰减完毕
                if self.flash_active() {
                    next = next.min(theme::REPAINT_CHART);
                }
            }
            // 解锁/换钥页有轮询动效，也给行情档
            _ => next = next.min(theme::REPAINT_QUOTE),
        }
        ctx.request_repaint_after(Duration::from_millis(next));
    }
}

/// 闪烁状态查询（供重绘调度判断是否需要高频刷新）
impl BnApp {
    /// 是否有闪烁动画正在进行（任一位点未衰减完毕）
    fn flash_active(&self) -> bool {
        self.flash_book.active() || self.flash_trade.active() || self.flash_header.active()
    }
}

/// BTP_DEMO 演示数据（仅 debug 构建编译）：注入假资产/持仓快照，
/// 让资产总览/持仓等需要凭据的页面在无凭据环境也有完整布局可看
#[cfg(debug_assertions)]
impl BnApp {
    fn seed_demo_accounts(state: &Arc<SharedState>) {
        use crate::binance::model::{Balance, FutAccount, FutBalance, PositionRisk, SpotAccount};
        use crate::security::vault::Creds;
        let d = |s: &str| rust_decimal::Decimal::from_str_exact(s).unwrap_or_default();
        // 假凭据让 has_creds 门槛放行（签名请求会失败并弹错误 toast，截图时可忽略）
        if !state.has_creds() {
            state.set_creds(Some(Creds {
                api_key: "demo".into(),
                api_secret: "demo".into(),
            }));
        }
        let mut acct = state.acct.lock();
        acct.spot = Some(SpotAccount {
            can_trade: true,
            balances: vec![
                Balance {
                    asset: "USDT".into(),
                    free: d("12345.678"),
                    locked: d("100.000"),
                },
                Balance {
                    asset: "BNB".into(),
                    free: d("12.5"),
                    locked: d("0"),
                },
                Balance {
                    asset: "BTC".into(),
                    free: d("0.15"),
                    locked: d("0"),
                },
            ],
        });
        acct.fut = Some(FutAccount {
            total_wallet: d("12345.67890000"),
            total_unrealized: d("-56.78000000"),
            total_margin: d("12288.89890000"),
            available: d("9876.54000000"),
            assets: vec![FutBalance {
                asset: "USDT".into(),
                wallet: d("12345.67890000"),
                unrealized: d("-56.78000000"),
                margin: d("12288.89890000"),
                available: d("9876.54000000"),
            }],
        });
        acct.fut_dual_side = false;
        acct.fut_leverage = [
            ("BTCUSDT".to_string(), d("10")),
            ("ETHUSDT".to_string(), d("5")),
        ]
        .into_iter()
        .collect();
        // 保证金模式与 symbolConfig 同源（杠杆行徽章显示/切换）
        acct.fut_margin_type = [
            ("BTCUSDT".to_string(), "CROSSED".to_string()),
            ("ETHUSDT".to_string(), "ISOLATED".to_string()),
        ]
        .into_iter()
        .collect();
        acct.fut_positions = vec![
            PositionRisk {
                symbol: "BTCUSDT".into(),
                amt: d("0.150"),
                entry_price: d("80123.50"),
                break_even_price: d("80163.21"),
                mark_price: d("80950.10"),
                unrealized: d("123.99"),
                liquidation_price: d("73500.00"),
                leverage: d("10"),
                side: "BOTH".into(),
                take_profit: d("88000.00"),
                stop_loss: d("76000.00"),
            },
            PositionRisk {
                symbol: "ETHUSDT".into(),
                amt: d("-2.500"),
                entry_price: d("3120.00"),
                break_even_price: d("3116.44"),
                mark_price: d("3050.25"),
                unrealized: d("-174.37"),
                liquidation_price: d("3620.00"),
                leverage: d("5"),
                side: "BOTH".into(),
                take_profit: d("2900.00"),
                stop_loss: d("3350.00"),
            },
        ];
    }
}
