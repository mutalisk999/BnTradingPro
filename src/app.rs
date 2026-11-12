//! 应用主体：启动流程路由（解锁/配置密钥/主界面）与交易终端布局
//!
//! GPUI 版根视图 Entity：状态与纯逻辑从 egui 版原样迁移；渲染改为 Render impl。
//! 引擎经唤醒通道（tokio unbounded channel）请求重绘——GPUI 的前台执行器
//! 非 Send，引擎线程不能直接 notify，UI 侧任务 recv 后 `cx.notify()`。

use crate::binance::client::OrderRequest;
use crate::binance::{FutPosSide, Interval, Market, Side};
use crate::config::{self, AppConfig};
use crate::engine::state::SharedState;
use crate::engine::Command;
use crate::security::vault;
use crate::ui::{chart, flash, theme, trade, widgets};
use gpui::{div, px, App, AppContext, Context, Div, Entity, FontWeight, IntoElement, ParentElement, Render, Styled, Window};
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

/// 设置页的编辑草稿：存放在 BnApp 上，避免每帧从快照重建导致输入被重置
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
    /// 字体大小档位：small / medium / large
    pub font_size: String,
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
            font_size: cfg.font_size.clone(),
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
            font_size: self.font_size,
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
    /// 字体大小档位（"small"/"medium"/"large"）：与主题/语言一样在渲染里
    /// 跟随配置即时生效
    pub applied_font_size: String,
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
    /// 盘口面板宽度（pt），右缘把手拖动调节，范围 180~340
    pub book_width: f32,
    /// 图表最大化（隐藏右侧下单/盘口面板，仅保留页头 + 图表）
    pub chart_maximized: bool,
    /// 右侧下单面板展开（false 时收起为右缘窄条，» / « 按钮切换）
    pub trade_expanded: bool,
    /// 右侧下单面板宽度（pt），左缘把手拖动调节，范围 300~360
    pub trade_width: f32,
    /// 价格变动高亮位点集合（盘口/成交/页头）
    pub flash: flash::FlashState,
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
}

impl BnApp {
    /// 创建根视图 Entity：初始化共享状态、启动引擎、挂接唤醒/心跳任务。
    /// `main.rs` 在 `open_window` 构建闭包中调用，返回值即窗口根视图。
    pub fn new(_window: &mut Window, cx: &mut App, cfg: AppConfig) -> Entity<Self> {
        let dir = config::data_dir();
        let state = Arc::new(SharedState::default());
        *state.cfg.write() = cfg.clone();

        // 唤醒通道：引擎写状态后 send(())，下方任务 recv 后触发重绘。
        // 引擎线程不能触碰 GPUI 执行器（非 Send），通道是标准桥
        let (wake_tx, mut wake_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        crate::engine::spawn(Arc::clone(&state), wake_tx);

        let phase = if vault::vault_exists(dir) {
            Phase::Unlock
        } else {
            Phase::EnterKeys
        };
        let interval = Interval::from_str(&cfg.default_interval);
        theme::set_mode_str(&cfg.theme_mode);
        theme::set_price_dir_str(&cfg.price_dir);
        crate::i18n::set_lang(if cfg.lang == "en" {
            crate::i18n::EN
        } else {
            crate::i18n::ZH
        });

        let default_symbol = cfg.default_symbol.to_uppercase();
        cx.new(|cx| {
            // 唤醒任务：tokio 通道的 waker 唤醒 GPUI 主线程上的 async 任务，
            // 延迟与旧版 request_repaint 相当
            cx.spawn(async move |this, cx| {
                while wake_rx.recv().await.is_some() {
                    let _: Result<(), _> = this.update(cx, |_, cx| cx.notify());
                }
            })
            .detach();
            // 心跳任务：自适应间隔驱动 flash 衰减/toast TTL/倒计时等
            // "无数据到达也要动"的动画
            cx.spawn(async move |this, cx| {
                loop {
                    let ms = this
                        .update(cx, |app: &mut BnApp, _| app.heartbeat_ms())
                        .unwrap_or(theme::REPAINT_IDLE);
                    cx.background_executor()
                        .timer(Duration::from_millis(ms))
                        .await;
                    let _: Result<(), _> = this.update(cx, |_, cx| cx.notify());
                }
            })
            .detach();

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
                tab: match std::env::var("BTP_TAB").as_deref() {
                    Ok("futures") => Tab::Futures,
                    _ => Tab::Spot,
                },
                applied_theme: cfg.theme_mode.clone(),
                applied_lang: cfg.lang.clone(),
                // 留空让首帧应用块设一次档位（空串与任何值都不相等）
                applied_font_size: String::new(),
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
                flash: flash::FlashState::default(),
                book_anim: Default::default(),
                book_tab: crate::ui::market::BookTab::Book,
                flow_auto_done: false,
                hist_kind: crate::ui::orders::HistKind::All,
            }
        })
    }

    /// 下单面板的最小宽度：两市场统一 300（= 默认面板宽）。字号上调后
    /// 272 下合约表单的 TIF 胶囊/杠杆行/价格辅助钮过挤，300 保证完整
    /// 显示；分割线位置在现货/期货间保持一致
    fn min_trade_width(&self) -> f32 {
        300.0
    }

    /// 心跳间隔（毫秒）：按当前可见区域各自需要多快取最小值，而非全局一刀切
    fn heartbeat_ms(&self) -> u64 {
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
                if self.flash.active() {
                    next = next.min(theme::REPAINT_CHART);
                }
            }
            // 解锁/换钥页有轮询动效，也给行情档
            _ => next = next.min(theme::REPAINT_QUOTE),
        }
        next
    }

    // ---------- 价格预警 ----------

    /// 每帧评估预警：最新价来自全市场迷你 ticker（约 1s 一批，覆盖任意
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
        self.flash.reset();
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
        self.flash.reset();
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

    // ---------- 渲染 ----------

    /// 配置即时生效块：主题/语言/字号在渲染帧跟随配置变更应用
    fn apply_cfg_changes(&mut self, cx: &mut Context<Self>) {
        let (theme_mode, lang, font_size) = {
            let cfg = self.state.cfg.read();
            (
                cfg.theme_mode.clone(),
                cfg.lang.clone(),
                cfg.font_size.clone(),
            )
        };
        if self.applied_theme != theme_mode {
            theme::set_mode_str(&theme_mode);
            cx.global_mut::<theme::UiTheme>().refresh();
            self.applied_theme = theme_mode;
        }
        if self.applied_lang != lang {
            crate::i18n::set_lang(if lang == "en" {
                crate::i18n::EN
            } else {
                crate::i18n::ZH
            });
            self.applied_lang = lang;
        }
        // 字号：档位系数改变全部字号取值（font_*() 每帧现算），无需重装字体
        if self.applied_font_size != font_size {
            theme::set_font_size_str(&font_size);
            self.applied_font_size = font_size;
        }
    }

    /// 主界面骨架（里程碑 1）：顶栏 + 占位中央区；完整布局在里程碑 4-8 移植
    fn render_main(&mut self, cx: &mut Context<Self>) -> Div {
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
        let t = *theme::c(cx);
        div()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            // 顶栏（页签/主题语言切换/状态灯在里程碑 4 完整移植）
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(36.0))
                    .px(px(14.0))
                    .bg(t.panel)
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .text_size(theme::font_xl())
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.accent)
                            .child("BnTradingPro"),
                    ),
            )
            .child(
                div().flex_1().flex().items_center().justify_center().child(
                    div()
                        .text_size(theme::font_lg())
                        .text_color(t.text_dim)
                        .child(crate::i18n::tr("主界面骨架（GPUI 迁移进行中）")),
                ),
            )
    }

    /// 解锁/配钥页骨架（里程碑 3 完整移植）
    fn render_auth(&mut self, cx: &mut Context<Self>) -> Div {
        let t = *theme::c(cx);
        let title = match self.phase {
            Phase::Unlock => crate::i18n::tr("解锁"),
            _ => crate::i18n::tr("配置 API 密钥"),
        };
        div().flex_1().flex().items_center().justify_center().child(
            div()
                .w(px(440.0))
                .flex()
                .flex_col()
                .bg(t.card)
                .border_1()
                .border_color(t.border)
                .rounded(px(theme::R_LG))
                .p(px(22.0))
                .child(
                    div()
                        .text_size(theme::font_lg())
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child(title),
                )
                .child(
                    div()
                        .mt(px(8.0))
                        .text_size(theme::font_md())
                        .text_color(t.text_dim)
                        .child(crate::i18n::tr("认证卡片将在里程碑 3 完整移植")),
                ),
        )
    }
}

impl Render for BnApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 本地价格预警逐帧评估（内部空表时即刻返回，无锁开销）
        self.check_alerts();
        // 主题/语言/字号跟随配置（设置页或顶栏修改后即时生效）
        self.apply_cfg_changes(cx);

        // 开发辅助：BTP_DEMO=1 跳过解锁直接进主界面，便于无凭据调试/截图（仅 debug 构建编译）；
        // 同时注入假资产快照，让资产总览/持仓等需要凭据的页面也有完整布局可看
        #[cfg(debug_assertions)]
        self.apply_demo();

        // 离开主界面（解锁页/换钥页）后重新进入时需要再做一次初始加载
        if !matches!(self.phase, Phase::Main) {
            self.initial_load_done = false;
        }

        let t = *theme::c(cx);
        let body = match self.phase {
            Phase::Main => self.render_main(cx),
            _ => self.render_auth(cx),
        };

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::FONT_UI)
            .bg(t.bg)
            .text_color(t.text)
            .child(body)
            // Toast 覆盖层：右下角堆叠
            .child(widgets::draw_toasts(&self.state))
    }
}

#[cfg(debug_assertions)]
impl BnApp {
    fn apply_demo(&mut self) {
        if self.phase == Phase::Main || std::env::var("BTP_DEMO").ok().as_deref() != Some("1") {
            return;
        }
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
        // 市场页签必须与 set_tab 走同一套换出逻辑（page swap + market），
        // 否则 BTP_DEMO_TAB=futures 仍渲染现货页，无法复现期货布局
        match tab {
            Tab::Spot => std::mem::swap(&mut self.spot, &mut self.page),
            Tab::Futures => std::mem::swap(&mut self.fut, &mut self.page),
            _ => {}
        }
        if let Some(m) = tab.market() {
            self.market = m;
        }
        Self::seed_demo_accounts(&self.state);
    }

    /// BTP_DEMO 演示数据：注入假资产/持仓快照，让资产总览/持仓等需要凭据的
    /// 页面在无凭据环境也有完整布局可看
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
