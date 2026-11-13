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
use crate::ui::widgets::DragGhost;
use crate::ui::{assets, chart, flash, help, market, orders, settings, symbols, textfield::TextField, theme, trade, unlock, widgets};
use gpui::{
    div, prelude::FluentBuilder, px, App, AppContext, Context, Div, Entity, FontWeight,
    DragMoveEvent, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Window,
};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

// ---------- 快捷键 ----------
// B/S 切换买卖侧并聚焦数量框（输入框聚焦时 TextField 消费按键，不触发）；
// Ctrl+Shift+C 连按两次撤销当前交易对全部挂单：单次和弦键误按风险高，
// 与按钮路径同一两步纪律——第一击武装（4 秒窗口内）并提示，第二击才执行
gpui::actions!(bn, [Buy, Sell, CancelAllShortcut]);

// ---------- 交易页骨架尺寸 ----------
// 默认值与拖拽 clamp 同源：BnApp::new 的初始宽高与 on_drag_move 的
// clamp 边界都引用这里，改布局只动一处
const SIDE_W: f32 = 190.0;
const SIDE_W_MIN: f32 = 190.0;
const SIDE_W_MAX: f32 = 240.0;
const BOOK_W: f32 = 230.0;
const BOOK_W_MIN: f32 = 180.0;
const BOOK_W_MAX: f32 = 340.0;
const TRADE_W: f32 = 300.0;
const TRADE_W_MIN: f32 = 300.0;
const TRADE_W_MAX: f32 = 360.0;
const ORDERS_H: f32 = 240.0;
const ORDERS_H_MIN: f32 = 120.0;
const ORDERS_H_MAX: f32 = 500.0;
/// 收起态侧栏窄条宽
const RAIL_W: f32 = 36.0;

/// 面板尺寸拖拽把手的载荷标记：类型即身份，on_drag_move 按载荷类型分发到
/// 各自的宽度/高度调整逻辑（同一根容器上注册四个互不干扰的处理器）
#[derive(Default)]
pub struct DragSidebar;
#[derive(Default)]
pub struct DragBook;
#[derive(Default)]
pub struct DragTrade;
#[derive(Default)]
pub struct DragOrdersH;

/// 面板边缘的拖拽把手：8px 竖条/横条发起拖拽；宽度更新统一在根容器的
/// on_drag_move 里做——把手窄，指针很快划出把手范围，事件必须由更大的
/// 祖先容器持续接收（egui 版把手注册在外层 ui 上，同理）
fn h_resize_strip<T: Default + 'static>(id: &'static str, right_edge: bool) -> Stateful<Div> {
    let mut base = div()
        .id(id)
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(8.0))
        .cursor(gpui::CursorStyle::ResizeLeftRight);
    base = if right_edge {
        base.right_0()
    } else {
        base.left_0()
    };
    base.on_drag(
        T::default(),
        |_, _, _, cx| cx.new(|_| DragGhost),
    )
}

fn v_resize_strip<T: Default + 'static>(id: &'static str, top_edge: bool) -> Stateful<Div> {
    let mut base = div()
        .id(id)
        .absolute()
        .left_0()
        .right_0()
        .h(px(8.0))
        .cursor(gpui::CursorStyle::ResizeUpDown);
    base = if top_edge {
        base.top_0()
    } else {
        base.bottom_0()
    };
    base.on_drag(
        T::default(),
        |_, _, _, cx| cx.new(|_| DragGhost),
    )
}

/// 把手拖拽的位移提取：同一会话内相邻两次 drag_move 的差值。
/// 新会话的第一帧会拿到上一会话的残留坐标——差值会出现 40px 以上的跳变
/// （跨会话指针位移远大于单帧拖速），此时按新会话起点处理、不应用位移
fn drag_delta(last: Option<f32>, pos: f32) -> Option<f32> {
    let last = last?;
    let d = pos - last;
    (d.abs() < 40.0).then_some(d)
}

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
    /// 设置页文本输入框（GPUI TextField 独立 Entity，文本状态在框内；
    /// 保存时经草稿写回配置）
    pub settings_fields: settings::SettingsFields,
    /// 价格预警目标价输入框（文本经 on_change 同步进 alert_input）
    pub alert_field: Entity<TextField>,

    /// 现货/期货页各自的持久状态
    pub spot: TradingPage,
    pub fut: TradingPage,
    /// 侧栏符号列表缓存（市场，过滤词大写，规则代次，结果）
    pub symbols_cache: Option<(Market, String, u64, std::sync::Arc<Vec<SharedString>>)>,
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
    /// 持仓行内 TP/SL 弹窗：(交易对, 多头?, 持仓方向)；触发价文本在
    /// tpsl_sl_field/tpsl_tp_field 输入框内，提交时读出
    pub tpsl_edit: Option<(String, bool, FutPosSide)>,
    /// 价格预警弹窗开关（页头铃铛）
    pub alerts_open: bool,
    /// 新预警目标价输入缓冲
    pub alert_input: String,
    /// 认证表单输入框（GPUI 版 TextField 是独立 Entity，文本状态在框内；
    /// pw1/pw2/api_key/api_secret 字符串字段在提交时从框内同步）
    pub pw1_field: Entity<TextField>,
    pub pw2_field: Entity<TextField>,
    pub api_key_field: Entity<TextField>,
    pub api_secret_field: Entity<TextField>,
    /// 侧栏交易对搜索框（过滤词同步进 page.symbol_filter 供缓存键使用）
    pub search_field: Entity<TextField>,
    /// 下单表单输入框集合（现货/期货页共用一套 TextField Entity；文本仍是
    /// page.form 的 String 字段为数据源，渲染帧回灌外部变更）
    pub trade_fields: trade::TradeFields,
    /// 订单类型下拉菜单展开态
    pub kind_menu_open: bool,
    /// 持仓 TP/SL 弹窗的止损/止盈触发价输入框
    pub tpsl_sl_field: Entity<TextField>,
    pub tpsl_tp_field: Entity<TextField>,
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
    /// 底部订单区高度（pt），上缘把手拖动调节，范围 120~500
    pub orders_h: f32,
    /// 把手拖拽会话的上一帧指针坐标（x/y）：None = 当前没有进行中的拖拽。
    /// 四个把手共用（同一时刻至多一个拖拽会话），40px 跳变守卫见 drag_delta
    pub resize_drag_x: Option<f32>,
    pub resize_drag_y: Option<f32>,
    /// 价格变动高亮位点集合（盘口/成交/页头）
    pub flash: flash::FlashState,
    /// 图表鼠标位置（窗口绝对坐标）；None = 不在图表上（十字光标隐藏）
    pub chart_mouse: Option<(f32, f32)>,
    /// 图表悬停态（on_hover 维护；false 时清 chart_mouse）
    pub chart_hover: bool,
    /// 图表拖拽平移会话的上一帧指针 x（None = 会话未开始）
    pub pan_last: Option<f32>,
    /// 图表 K 线总数（每次渲染由 chart::show 写入，拖拽平移钳位用）
    pub chart_n: usize,
    /// 副图指标下拉菜单展开态
    pub sub_menu_open: bool,
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
        // 快捷键：B/S 买卖侧、Ctrl+Shift+C 两步全撤（输入框聚焦时由
        // TextField 消费按键，不会误触）
        cx.bind_keys([
            KeyBinding::new("b", Buy, None),
            KeyBinding::new("s", Sell, None),
            KeyBinding::new("ctrl-shift-c", CancelAllShortcut, None),
        ]);

        let default_symbol = cfg.default_symbol.to_uppercase();
        let entity = cx.new(|cx| {
            // 认证表单输入框：全部遮蔽显示（egui 版 password(true)——API Key
            // 明文也足以创建 listenKey 读取推送，录屏/共享屏幕时同样不能暴露）
            let mk_field = |cx: &mut Context<Self>, id: &'static str| {
                cx.new(|cx_tf| {
                    let mut f = TextField::new(cx_tf);
                    f.set_id(id).set_masked(true).set_font_size(theme::font_body());
                    f
                })
            };
            let pw1_field = mk_field(cx, "pw1");
            let pw2_field = mk_field(cx, "pw2");
            let api_key_field = mk_field(cx, "api_key");
            let api_secret_field = mk_field(cx, "api_secret");
            // 侧栏搜索框：明文 + 占位文案
            let search_field = cx.new(|cx_tf| {
                let mut f = TextField::new(cx_tf);
                f.set_id("symbol_search")
                    .set_placeholder(crate::i18n::tr("搜索交易对…"))
                    .set_font_size(theme::font_body());
                f
            });
            // 下单表单输入框：数字过滤 + 占位（8 个框统一在 TradeFields 里构建）
            let trade_fields = trade::TradeFields::new(cx);
            // 持仓 TP/SL 弹窗输入框：数字过滤（开窗时清空）
            let mk_tpsl_field = |cx: &mut Context<Self>, id: &'static str| {
                cx.new(|cx_tf| {
                    let mut f = TextField::new(cx_tf);
                    f.set_id(id).set_numeric(true).set_font_size(theme::font_md());
                    f
                })
            };
            let tpsl_sl_field = mk_tpsl_field(cx, "tpsl_sl");
            let tpsl_tp_field = mk_tpsl_field(cx, "tpsl_tp");
            // 设置页输入框组 + 预警目标价输入框
            let settings_fields = settings::SettingsFields::new(cx);
            let alert_field = cx.new(|cx_tf| {
                let mut f = TextField::new(cx_tf);
                f.set_id("alert_price")
                    .set_numeric(true)
                    .set_placeholder(crate::i18n::tr("目标价"))
                    .set_font_size(theme::font_md());
                f
            });

            // 唤醒任务：tokio 通道的 waker 唤醒 GPUI 主线程上的 async 任务。
            // 必须合并：行情活跃时 WS 消息可达数百条/秒，逐条 notify 会让整树
            // 按 vsync 频率全量重建（egui 版 request_repaint_after(100ms) 的
            // 合并语义）——首条唤醒后先排空通道再统一 notify 一次，最小间隔
            // 对齐 REPAINT_QUOTE；尾随帧由心跳任务兜底
            cx.spawn(async move |this, cx| {
                loop {
                    if wake_rx.recv().await.is_none() {
                        break;
                    }
                    // 排空积压唤醒 + 限流到 10fps：期间新到的唤醒一并消化
                    // （GPUI 主线程 async 无 tokio reactor，必须用背景执行器计时）
                    cx.background_executor()
                        .timer(Duration::from_millis(theme::REPAINT_QUOTE))
                        .await;
                    while wake_rx.try_recv().is_ok() {}
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
                pw1_field,
                pw2_field,
                api_key_field,
                api_secret_field,
                search_field,
                trade_fields,
                kind_menu_open: false,
                tpsl_sl_field,
                tpsl_tp_field,
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
                settings_fields,
                alert_field,
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
                sidebar_expanded: true,
                sidebar_width: SIDE_W,
                book_width: BOOK_W,
                chart_maximized: false,
                trade_expanded: true,
                trade_width: TRADE_W,
                orders_h: ORDERS_H,
                resize_drag_x: None,
                resize_drag_y: None,
                flash: flash::FlashState::default(),
                chart_mouse: None,
                chart_hover: false,
                pan_last: None,
                chart_n: 0,
                sub_menu_open: false,
                book_anim: Default::default(),
                book_tab: crate::ui::market::BookTab::Book,
                flow_auto_done: false,
                hist_kind: crate::ui::orders::HistKind::All,
            }
        });

        // 回车提交：TextField 的回调拿不到视图句柄，经 WeakEntity 找回。
        // pw 框回车 = 解锁；Secret 框回车 = 保存（与 egui 版 lost_focus+Enter 一致）
        let weak = entity.downgrade();
        let w = weak.clone();
        entity.update(cx, |app, cx| {
            app.pw1_field.update(cx, |f, _| {
                f.set_on_submit(Box::new(move |window, cx| {
                    let unlocked = w
                        .update(cx, |app: &mut BnApp, cx| {
                            app.pw1 =
                                Zeroizing::new(app.pw1_field.read(cx).text().to_string());
                            unlock::try_unlock(app, cx);
                            app.phase == Phase::Main
                        })
                        .unwrap_or(false);
                    // 解锁成功后失焦：字段已不渲染，残留焦点会挡住主界面快捷键
                    if unlocked {
                        window.blur();
                    }
                }));
            });
        });
        let w = weak.clone();
        entity.update(cx, |app, cx| {
            app.api_secret_field.update(cx, |f, _| {
                f.set_on_submit(Box::new(move |window, cx| {
                    let saved = w
                        .update(cx, |app: &mut BnApp, cx| {
                            app.sync_auth_fields(cx);
                            let can_save = app.can_save_keys(cx);
                            if can_save {
                                unlock::save_keys(
                                    app,
                                    cx,
                                    config::data_dir(),
                                    vault::vault_exists(config::data_dir()),
                                );
                            }
                            app.phase == Phase::Main
                        })
                        .unwrap_or(false);
                    if saved {
                        window.blur();
                    }
                }));
            });
        });
        let w = weak.clone();
        entity.update(cx, |app, cx| {
            app.search_field.update(cx, |f, _| {
                f.set_on_change(Box::new(move |text, _, cx| {
                    let _ = w.update(cx, |app: &mut BnApp, cx| {
                        app.page.symbol_filter = text.to_string();
                        cx.notify();
                    });
                }));
            });
        });
        // 下单表单输入框写回：框内文本变更写回 page.form.*（回调需要视图句柄，
        // 实体在闭包里创建后才能拿到 WeakEntity，所以挂接放在 new 末尾）
        entity.update(cx, |app, cx| {
            let weak = cx.entity().downgrade();
            app.trade_fields.attach(&weak, cx);
            // 设置页输入框写回 settings 草稿
            app.settings_fields.attach(&weak, cx);
            // 预警输入：输入即存草稿；回车 = 添加（添加成功清空、失败保留待改）
            app.alert_field.update(cx, |f, _| {
                let w = weak.clone();
                f.set_on_change(Box::new(move |s, _, cx| {
                    let s = s.to_string();
                    let _ = w.update(cx, |app: &mut BnApp, _| app.alert_input = s);
                }));
                let w = weak.clone();
                f.set_on_submit(Box::new(move |_, cx| {
                    let _ = w.update(cx, |app: &mut BnApp, cx| {
                        app.alert_input = app.alert_field.read(cx).text().to_string();
                        app.add_alert();
                        let cur = app.alert_input.clone();
                        app.alert_field.update(cx, |f, _| f.set_text(cur));
                    });
                }));
            });
        });
        entity
    }

    /// 提交时把 TextField 文本同步到逻辑字段（try_unlock/save_keys 读字符串字段）
    pub(crate) fn sync_auth_fields(&mut self, cx: &Context<Self>) {
        self.pw1 = Zeroizing::new(self.pw1_field.read(cx).text().to_string());
        self.pw2 = Zeroizing::new(self.pw2_field.read(cx).text().to_string());
        self.api_key = Zeroizing::new(self.api_key_field.read(cx).text().to_string());
        self.api_secret = Zeroizing::new(self.api_secret_field.read(cx).text().to_string());
    }

    /// 保存按钮可用性（egui 版 can_save）：密钥两项非空，且需要主密码时已填
    pub(crate) fn can_save_keys(&self, cx: &App) -> bool {
        let vault_exists = vault::vault_exists(config::data_dir());
        let need_master = vault_exists || self.master.is_none();
        !self.api_key_field.read(cx).text().trim().is_empty()
            && !self.api_secret_field.read(cx).text().trim().is_empty()
            && (!need_master || !self.pw1_field.read(cx).text().trim().is_empty())
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

    /// 主界面：顶栏 + 按顶层页签路由（交易页 / 资产 / 设置 / 帮助）
    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
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
        let topbar = self.topbar(cx);
        let body = match self.tab {
            Tab::Spot | Tab::Futures => self.trading_ui(window, cx),
            Tab::Portfolio => assets::show(self, cx),
            Tab::Settings => settings::show(self, cx),
            Tab::Help => help::show(self, cx),
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(topbar)
            .child(body)
    }

    /// 顶部导航：品牌 + 五大页签 + 右侧簇（状态灯 / 环境徽标 / 语言主题切换）。
    /// 右侧簇内容超宽时由顶栏 overflow 遮挡裁剪——大字号 + 窄窗口下宁可
    /// 裁掉右侧指示，也不让它们压到左侧页签文字
    fn topbar(&mut self, cx: &mut Context<Self>) -> Div {
        let t = theme::c(cx);
        let mut bar = div()
            .flex()
            .items_center()
            .h(px(theme::TOPBAR_H))
            .px(px(theme::SPACE_XL))
            .gap(px(theme::SPACE_XL))
            .bg(t.panel)
            .border_b_1()
            .border_color(t.border)
            .overflow_hidden()
            // 品牌：与页签同一行内垂直居中
            .child(
                div()
                    .text_size(theme::font_xl())
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.accent)
                    .whitespace_nowrap()
                    .child("BnTradingPro"),
            );
        for (i, tab) in [
            Tab::Spot,
            Tab::Futures,
            Tab::Portfolio,
            Tab::Settings,
            Tab::Help,
        ]
        .into_iter()
        .enumerate()
        {
            bar = bar.child(
                widgets::pill_tab(("navtab", i), tab.label(), self.tab == tab).on_click(
                    cx.listener(move |app: &mut BnApp, _, _, cx| {
                        app.set_tab(tab);
                        cx.notify();
                    }),
                ),
            );
        }
        bar = bar.child(div().flex_1());

        // 右侧簇（视觉自左向右）：账户灯 → 行情灯 → REST → 分隔 →
        // 环境徽标 → 分隔 → 语言 → 主题（egui RTL 容器的视觉顺序）
        let mut right = div().flex().items_center().gap(px(theme::SPACE_MD));
        if let Some(m) = self.tab.market() {
            if self.state.has_creds() {
                let uws = self.state.md.lock().user_ws(m).clone();
                right = right.child(widgets::status_light(crate::i18n::tr("账户"), &uws));
            } else {
                // 未配置密钥：保持「名称 + 状态」结构，用警示文字代替状态灯
                right = right.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE_SM))
                        .child(
                            div()
                                .text_size(theme::font_sm())
                                .text_color(t.text_dim)
                                .child(crate::i18n::tr("账户").to_string()),
                        )
                        .child(
                            div()
                                .text_size(theme::font_sm())
                                .text_color(t.warn)
                                .child(crate::i18n::tr("未配置密钥").to_string()),
                        ),
                );
            }
            let mws = self.state.md.lock().market_ws(m).clone();
            right = right.child(widgets::status_light(crate::i18n::tr("行情"), &mws));
        }
        let rest_ok = self
            .state
            .rest_ok
            .load(std::sync::atomic::Ordering::Relaxed);
        let testnet = self.state.cfg.read().testnet;
        let lang_txt: &str = if self.state.cfg.read().lang == "en" {
            "中"
        } else {
            "EN"
        };
        let theme_txt: &str = if theme::is_light() {
            crate::i18n::tr("深色")
        } else {
            crate::i18n::tr("浅色")
        };
        let vsep = || {
            div()
                .w(px(1.0))
                .h(px(16.0))
                .bg(theme::faded(t.border, 0.7))
        };
        right = right
            .child(widgets::status_light_ok(
                crate::i18n::tr("REST"),
                rest_ok,
                crate::i18n::tr("REST 接口正常"),
                crate::i18n::tr("REST 接口异常"),
            ))
            .child(vsep())
            .child(widgets::badge(
                if testnet {
                    "TESTNET"
                } else {
                    crate::i18n::tr("实盘 · 真实资金")
                },
                if testnet { t.accent } else { t.err },
            ))
            .child(vsep())
            .child(
                widgets::btn_default("lang_toggle", lang_txt, true)
                    .tooltip({
                        let tip = crate::i18n::tr("切换语言").to_string();
                        move |_, cx| widgets::tooltip_view(tip.clone(), cx)
                    })
                    .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                        app.toggle_lang(cx);
                    })),
            )
            .child(
                widgets::btn_default("theme_toggle", theme_txt, true)
                    .tooltip({
                        let tip = crate::i18n::tr("切换主题").to_string();
                        move |_, cx| widgets::tooltip_view(tip.clone(), cx)
                    })
                    .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                        app.toggle_theme(cx);
                    })),
            );
        bar.child(right)
    }

    /// 主题快捷切换：改配置 + 刷新全局主题 + 同步设置草稿（否则设置页
    /// 「保存并重连」会用旧草稿把切换回滚）+ 落盘走引擎命令
    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        let mut cfg = self.state.cfg.read().clone();
        cfg.theme_mode = if cfg.theme_mode == "light" {
            "dark".into()
        } else {
            "light".into()
        };
        theme::set_mode_str(&cfg.theme_mode);
        self.applied_theme = cfg.theme_mode.clone();
        self.settings.theme_mode = cfg.theme_mode.clone();
        *self.state.cfg.write() = cfg;
        self.state.send_cmd(Command::PersistConfig);
        cx.notify();
    }

    /// 语言快捷切换（按钮显示目标语言）：同主题的同步纪律
    fn toggle_lang(&mut self, cx: &mut Context<Self>) {
        let mut cfg = self.state.cfg.read().clone();
        cfg.lang = if cfg.lang == "en" {
            "zh".into()
        } else {
            "en".into()
        };
        crate::i18n::set_lang(if cfg.lang == "en" {
            crate::i18n::EN
        } else {
            crate::i18n::ZH
        });
        self.applied_lang = cfg.lang.clone();
        self.settings.lang = cfg.lang.clone();
        *self.state.cfg.write() = cfg;
        self.state.send_cmd(Command::PersistConfig);
        cx.notify();
    }

    // ---------- 交易页布局 ----------
    //
    // 侧栏 | 盘口 | ┌ 图表 ┬ 下单面板 ┐
    //           | └ 订单区（从盘口右缘铺到窗口右缘）┘
    // 面板宽度用固定宽 + 自绘把手锁死：不用容器自适应（egui 版可调面板会把
    // 内容宽度持久化，页签来回切会越撑越宽）
    fn trading_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = theme::c(cx);
        let mut row = div().flex_1().flex().flex_row().overflow_hidden();
        row = row.child(self.sidebar(cx));
        // 图表最大化：盘口/下单/订单区全部隐藏，图表占满中央区
        if !self.chart_maximized {
            row = row.child(self.book_panel(window, cx));
        }
        // 中央：页头（交易对/最新价/24h 统计）+ 图表
        let central = div()
            .flex_1()
            .flex()
            .flex_col()
            .p(px(theme::SPACE_LG))
            .bg(t.bg)
            .child(market::header(self, cx))
            .child(chart::show(self, cx));
        let mut chart_row = div().flex_1().flex().flex_row().overflow_hidden();
        chart_row = chart_row.child(central);
        if !self.chart_maximized {
            chart_row = chart_row.child(self.trade_panel(cx));
        }
        let mut mid = div().flex_1().flex().flex_col().child(chart_row);
        if !self.chart_maximized {
            mid = mid.child(self.orders_panel(cx));
        }
        row.child(mid)
    }

    /// 左侧交易对列表：展开态（收起钮 + 搜索框 + 列表 + 右缘把手）/
    /// 折叠态（36px 窄条只放展开钮）。过滤词在搜索框内，折叠不丢状态
    fn sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let t = theme::c(cx);
        if self.sidebar_expanded {
            let has_filter = !self.search_field.read(cx).text().trim().is_empty();
            let top_row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_SM))
                .h(px(theme::CONTROL_H))
                .child(
                    widgets::glyph_button(
                        "sidebar_collapse",
                        "«",
                        12.0,
                        t.text_dim,
                        crate::i18n::tr("收起侧边栏"),
                    )
                    .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                        app.sidebar_expanded = false;
                        cx.notify();
                    })),
                )
                .child(div().flex_1().child(self.search_field.clone()))
                .when(has_filter, |d| {
                    d.child(
                        widgets::glyph_button(
                            "search_clear",
                            "×",
                            12.0,
                            t.text_dim,
                            crate::i18n::tr("清空搜索"),
                        )
                        .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                            app.search_field.update(cx, |f, _| f.clear());
                            app.page.symbol_filter.clear();
                            cx.notify();
                        })),
                    )
                });
            div()
                .relative()
                .w(px(self.sidebar_width))
                .h_full()
                .flex()
                .flex_col()
                .p(px(theme::SPACE_LG))
                .pb(px(0.0))
                .bg(t.panel)
                .child(top_row)
                .child(symbols::list(self, cx))
                // 右缘拖宽把手：范围 190~240
                .child(h_resize_strip::<DragSidebar>("sidebar_resize", true))
        } else {
            div()
                .relative()
                .w(px(RAIL_W))
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .p(px(theme::SPACE_SM))
                .bg(t.panel)
                .child(
                    widgets::glyph_button(
                        "sidebar_expand",
                        "»",
                        12.0,
                        t.text_dim,
                        crate::i18n::tr("展开侧边栏"),
                    )
                    .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                        app.sidebar_expanded = true;
                        cx.notify();
                    })),
                )
        }
    }

    /// 盘口面板（左，全高）：一直显示到 UI 最下面，不被底部订单区挤上去。
    /// 内容（页签/盘口行/价差/档位胶囊、成交明细）在 market.rs 渲染
    fn book_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = theme::c(cx);
        // 盘口内容高度预算：视口高 − 顶栏（36 行高 + 1 下边框）− 面板上内边距 8
        // − 页签行 BTN_H − 页签下分隔线（1 + 上下 my 4）。档位数/行高在
        // market.rs 内据此现算（GPUI 布局前无法询问剩余空间）
        let vh = f32::from(window.viewport_size().height);
        let body_h = (vh - theme::TOPBAR_H - 1.0 - theme::SPACE_LG - theme::BTN_H
            - (1.0 + theme::SPACE_SM * 2.0))
            .max(0.0);
        let avail_w = (self.book_width - theme::SPACE_XXL * 2.0).max(0.0);
        div()
            .relative()
            .w(px(self.book_width))
            .h_full()
            .flex()
            .flex_col()
            .p(px(theme::SPACE_LG))
            .pb(px(0.0))
            .bg(t.panel)
            .child(market::book_panel(self, window, cx, body_h, avail_w))
            // 右缘拖宽把手：范围 180~340
            .child(h_resize_strip::<DragBook>("book_resize", true))
    }

    /// 右侧下单面板：展开态顶部「»」收起 + 左缘拖宽把手；收起态退为
    /// 右缘 36px 窄条「«」。表单状态都在 page 上，折叠不丢输入；
    /// 表单本体在里程碑 7 移植
    fn trade_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let t = theme::c(cx);
        if self.trade_expanded {
            let symbol = self.page.symbol.clone();
            div()
                .relative()
                .w(px(self.trade_width))
                .h_full()
                .flex()
                .flex_col()
                .p(px(theme::SPACE_LG))
                .bg(t.panel)
                .child(
                    // 顶行：收起按钮居左，标题居右——标题右缘对齐下方
                    // 卖出按钮右缘（买卖按钮行总宽=面板内容宽）
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(theme::CONTROL_H))
                        .child(
                            widgets::glyph_button(
                                "trade_collapse",
                                "»",
                                12.0,
                                t.text_dim,
                                crate::i18n::tr("收起下单面板"),
                            )
                            .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                                app.trade_expanded = false;
                                cx.notify();
                            })),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(theme::font_body())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .whitespace_nowrap()
                                .child(format!("{}{symbol}", crate::i18n::tr("下单 · "))),
                        ),
                )
                // 下单表单：窄窗时纵向滚动（表单整体高度随类型/止盈止损行数变化）
                .child(
                    div()
                        .id("trade_scroll")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .child(trade::panel(self, cx)),
                )
                // 左缘拖宽把手：范围 300~360（指针左移 = 变宽）
                .child(h_resize_strip::<DragTrade>("trade_resize", false))
        } else {
            div()
                .relative()
                .w(px(RAIL_W))
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .p(px(theme::SPACE_SM))
                .bg(t.panel)
                .child(
                    widgets::glyph_button(
                        "trade_expand",
                        "«",
                        12.0,
                        t.text_dim,
                        crate::i18n::tr("展开下单面板"),
                    )
                    .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                        app.trade_expanded = true;
                        cx.notify();
                    })),
                )
        }
    }

    /// 底部订单区：子页签 + 表格区（M8）。从盘口右缘一直铺到窗口右缘，
    /// 持仓/挂单表格的列才放得下
    fn orders_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let t = theme::c(cx);
        div()
            .relative()
            .h(px(self.orders_h))
            .flex()
            .flex_col()
            .p(px(theme::SPACE_LG))
            .bg(t.panel)
            .child(self.sub_tabs(cx))
            .child(widgets::separator())
            // 订单子页表格：当前子页签的表格或空态
            .child(orders::show(self, cx))
            // 上缘拖高把手：范围 120~500（指针上移 = 变高）
            .child(v_resize_strip::<DragOrdersH>("orders_resize", true))
    }

    /// 交易页底部子页签：挂单 / 持仓+资金流水(合约) / 历史 / 成交
    fn sub_tabs(&mut self, cx: &mut Context<Self>) -> Div {
        let t = theme::c(cx);
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
        let open_n = self.state.orders.lock().open(self.market).len();
        let mut row = div().flex().flex_row().items_center().gap(px(theme::SPACE_XS));
        for (i, (tab, label)) in tabs.into_iter().enumerate() {
            row = row.child(
                widgets::pill_tab(("subtab", i), label, self.page.sub_tab == tab).on_click(
                    cx.listener(move |app: &mut BnApp, _, _, cx| {
                        app.page.sub_tab = tab;
                        cx.notify();
                    }),
                ),
            );
        }
        row.child(div().flex_1()).child(
            div()
                .text_size(theme::font_sm())
                .text_color(t.text_dim)
                .child(format!("{}{}", open_n, crate::i18n::tr("笔挂单"))),
        )
    }

    // ---------- 快捷键 ----------

    /// 是否有输入框聚焦：GPUI 0.2.2 的 action 分发先于 key listener（bubble
    /// 阶段的 action 监听器运行前就置 propagate_event=false），TextField 的
    /// stop_propagation 拦不住已分发的 action，因此 B/S/Ctrl+Shift+C 必须
    /// 在 handler 里自行让位于输入框——egui 版 focused() 门控的对应物
    fn text_field_focused(&self, window: &Window, cx: &App) -> bool {
        // window.focused() 为 None 说明焦点在非输入元素上，直接判否
        if window.focused(cx).is_none() {
            return false;
        }
        let owned = [
            &self.pw1_field,
            &self.pw2_field,
            &self.api_key_field,
            &self.api_secret_field,
            &self.search_field,
            &self.tpsl_sl_field,
            &self.tpsl_tp_field,
            &self.alert_field,
            &self.trade_fields.price,
            &self.trade_fields.qty,
            &self.trade_fields.trigger,
            &self.trade_fields.stop_limit,
            &self.trade_fields.callback_rate,
            &self.trade_fields.tp,
            &self.trade_fields.sl,
            &self.trade_fields.leverage,
            &self.settings_fields.proxy_url,
            &self.settings_fields.default_symbol,
            &self.settings_fields.recv_window,
            &self.settings_fields.http_timeout,
            &self.settings_fields.ov_spot_rest,
            &self.settings_fields.ov_spot_ws,
            &self.settings_fields.ov_fut_rest,
            &self.settings_fields.ov_fut_ws,
        ];
        owned.iter().any(|f| f.read(cx).is_focused(window))
    }

    /// B/S：切换买卖侧并聚焦数量框（输入框聚焦时让位，见 text_field_focused）
    fn side_shortcut(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        if self.phase == Phase::Main
            && self.tab.market().is_some()
            && self.confirm.is_none()
            && !self.text_field_focused(window, cx)
        {
            self.page.form.side = side;
            self.trade_fields.qty.update(cx, |f, _| f.focus(window));
            cx.notify();
        }
    }

    /// Ctrl+Shift+C 两步全撤当前交易对：武装态按 (交易对, 4 秒窗口) 匹配，
    /// 换交易对或超时即视为首次按下重新武装，杜绝「多久之前误按一次后被
    /// 补键撤销」。输入框聚焦时让位（见 text_field_focused）
    fn cancel_all_shortcut(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.phase != Phase::Main
            || self.tab.market().is_none()
            || self.confirm.is_some()
            || self.text_field_focused(window, cx)
            || !self.state.has_creds()
        {
            return;
        }
        let sym = self.page.symbol.clone();
        let now = std::time::Instant::now();
        let primed = matches!(
            &self.shortcut_cancel_armed,
            Some((s, t)) if *s == sym && now.duration_since(*t) < Duration::from_secs(4)
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

    /// 解锁/配钥页（渲染实现在 unlock.rs；本方法只做进入主界面后的
    /// 初始加载兜底等纯逻辑）
    fn render_auth(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        unlock::page(self, window, cx)
    }
}

impl Render for BnApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let t = theme::c(cx);
        let body = match self.phase {
            Phase::Main => self.render_main(window, cx),
            _ => self.render_auth(window, cx),
        };

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::FONT_UI)
            .bg(t.bg)
            .text_color(t.text)
            // 快捷键：B/S 切换买卖侧、Ctrl+Shift+C 两步全撤。GPUI 的 action
            // 分发先于 TextField 的 key listener（无法靠 stop_propagation 拦截），
            // 输入框聚焦时的让位在各 handler 内部判定（text_field_focused）
            .on_action(cx.listener(|app: &mut BnApp, _: &Buy, window, cx| {
                app.side_shortcut(Side::Buy, window, cx);
            }))
            .on_action(cx.listener(|app: &mut BnApp, _: &Sell, window, cx| {
                app.side_shortcut(Side::Sell, window, cx);
            }))
            .on_action(cx.listener(|app: &mut BnApp, _: &CancelAllShortcut, window, cx| {
                app.cancel_all_shortcut(window, cx);
            }))
            // Esc：确认下单弹窗 / 持仓 TP/SL 弹窗 / 订单类型下拉 / 副图指标
            // 下拉 / 两步撤销武装态，逐层关闭解除。输入框聚焦时 TextField
            // 对 Escape 放行（不 stop_propagation），事件仍会冒泡到这里
            .on_key_down(cx.listener(|app: &mut BnApp, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key != "escape" {
                    return;
                }
                let mut hit = false;
                if app.confirm.take().is_some() {
                    hit = true;
                }
                if app.tpsl_edit.take().is_some() {
                    hit = true;
                }
                if app.alerts_open {
                    app.alerts_open = false;
                    hit = true;
                }
                if app.kind_menu_open {
                    app.kind_menu_open = false;
                    hit = true;
                }
                if app.sub_menu_open {
                    app.sub_menu_open = false;
                    hit = true;
                }
                if app.market_cancel_armed
                    || app.sym_cancel_armed.is_some()
                    || app.row_cancel_armed.is_some()
                {
                    app.market_cancel_armed = false;
                    app.sym_cancel_armed = None;
                    app.row_cancel_armed = None;
                    hit = true;
                }
                if hit {
                    cx.notify();
                }
            }))
            // 点击面板外任意位置解除两步撤销武装态（与 egui any_click 同义；
            // 各武装按钮 on_mouse_down 内 stop_propagation，自身点击不会
            // 触发这里的解除）
            .on_mouse_down(MouseButton::Left, cx.listener(|app: &mut BnApp, _, _, cx| {
                if app.market_cancel_armed
                    || app.sym_cancel_armed.is_some()
                    || app.row_cancel_armed.is_some()
                {
                    app.market_cancel_armed = false;
                    app.sym_cancel_armed = None;
                    app.row_cancel_armed = None;
                    cx.notify();
                }
            }))
            // 面板尺寸拖拽：把手只负责发起（8px 竖条），指针很快划出把手
            // 范围，位移事件必须由覆盖全窗的根容器持续接收；按载荷类型分发
            .on_drag_move(cx.listener(
                |app: &mut BnApp, e: &DragMoveEvent<DragSidebar>, _, cx| {
                    let x = f32::from(e.event.position.x);
                    let last = app.resize_drag_x.replace(x);
                    if let Some(d) = drag_delta(last, x) {
                        // 指针右移 = 面板变宽（最小宽度 190 按用户要求）
                        app.sidebar_width = (app.sidebar_width + d).clamp(SIDE_W_MIN, SIDE_W_MAX);
                    }
                    cx.notify();
                },
            ))
            .on_drag_move(cx.listener(
                |app: &mut BnApp, e: &DragMoveEvent<DragBook>, _, cx| {
                    let x = f32::from(e.event.position.x);
                    let last = app.resize_drag_x.replace(x);
                    if let Some(d) = drag_delta(last, x) {
                        app.book_width = (app.book_width + d).clamp(BOOK_W_MIN, BOOK_W_MAX);
                    }
                    cx.notify();
                },
            ))
            .on_drag_move(cx.listener(
                |app: &mut BnApp, e: &DragMoveEvent<DragTrade>, _, cx| {
                    let x = f32::from(e.event.position.x);
                    let last = app.resize_drag_x.replace(x);
                    if let Some(d) = drag_delta(last, x) {
                        // 指针左移 = 面板变宽；下限两市场统一 300
                        app.trade_width = (app.trade_width - d).clamp(TRADE_W_MIN, TRADE_W_MAX);
                    }
                    cx.notify();
                },
            ))
            .on_drag_move(cx.listener(
                |app: &mut BnApp, e: &DragMoveEvent<DragOrdersH>, _, cx| {
                    let y = f32::from(e.event.position.y);
                    let last = app.resize_drag_y.replace(y);
                    if let Some(d) = drag_delta(last, y) {
                        // 指针上移 = 订单区变高
                        app.orders_h = (app.orders_h - d).clamp(ORDERS_H_MIN, ORDERS_H_MAX);
                    }
                    cx.notify();
                },
            ))
            .child(body)
            // 价格预警面板（None 时不渲染）：顶部右侧无遮罩悬挂，不挡下方 UI
            .children(market::alerts_dialog(self, cx))
            // 持仓 TP/SL 弹窗（None 时不渲染）：无遮罩悬挂，不挡下方 UI
            .children(orders::tpsl_dialog(self, cx))
            // 确认下单弹窗（None 时不渲染）：遮罩 + 居中卡片，
            // 在 Toast 之前渲染让 Toast 盖在其上（与 egui 版层级一致）
            .children(trade::confirm_dialog(self, cx))
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
