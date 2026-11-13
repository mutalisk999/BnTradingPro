//! 自绘蜡烛图：缩放/平移、十字光标、最新价与挂单/持仓叠加线、副图指标
//!
//! ChartState 与纯数学辅助函数自里程碑 1 原样迁移（无 egui 依赖）；
//! 里程碑 6：绘制改为 gpui::canvas——render 期算好可见段数据（纯数学、
//! 不依赖布局），prepaint 期按真实 bounds 算几何并 shape 文本，产出一列
//! 有序绘制原语（quad / path / 文本），paint 期原样执行。
//! 交互（滚轮缩放 / 拖拽平移 / 双击复位 / 悬停十字光标）由包裹 canvas 的
//! div 事件处理器承担。

use crate::app::BnApp;
use crate::binance::model::Candle;
use crate::binance::Interval;
use crate::ui::indicators::{self, Role, SubSeries};
use crate::ui::theme::{self, cur, faded, font_md, font_sm, FONT_MONO, FONT_UI};
use crate::ui::widgets;
use gpui::{
    div, point, prelude::FluentBuilder, px, quad, AppContext, ClickEvent, Context, Div,
    DragMoveEvent, Edges, ElementId, Font, Hsla, InteractiveElement, MouseButton,
    ParentElement, PathBuilder, Pixels, Point, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, TextRun, Window,
};
use rust_decimal::Decimal;

// 供 app.rs 以 `chart::SubIndicator` 引用（保持既有导入路径不变）
pub use crate::ui::indicators::SubIndicator;

/// 均线周期（MA5/MA10/MA20/MA60）；配色见 `theme::ma_colors()`（随主题切换）
pub const MA_PERIODS: [usize; 4] = [5, 10, 20, 60];

#[derive(Debug, Clone)]
pub struct ChartState {
    pub bars_visible: f32,
    pub right_off: f32,
    pub ma: [bool; 4],
    /// K 线克隆缓存：按 (市场, 交易对, 周期, kline_version) 失效。
    /// 之前每帧深克隆最多 1500 根 K 线并全量重建 SMA 前缀和（~10MB/s 分配），
    /// 缓存后仅在数据变化时重算一次
    cache_key: Option<(crate::binance::Market, String, Interval, u64)>,
    cache_klines: Vec<Candle>,
    cache_closes: Vec<f64>,
    /// 收盘价前缀和（长度 = closes.len()+1）：所有周期 SMA 共用，
    /// 查询 (prefix[gi+1]-prefix[gi+1-period])/period
    cache_prefix: Vec<f64>,
    /// 副图指标序列缓存：按 (市场, 交易对, 周期, kline_version, 指标) 失效
    sub_key: Option<(crate::binance::Market, String, Interval, u64, SubIndicator)>,
    sub_series: Option<SubSeries>,
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            bars_visible: 120.0,
            right_off: 0.0,
            ma: [true, true, true, false],
            cache_key: None,
            cache_klines: Vec::new(),
            cache_closes: Vec::new(),
            cache_prefix: Vec::new(),
            sub_key: None,
            sub_series: None,
        }
    }
}

/// 图表叠加线（挂单/强平价等）：绘制时取主题色
pub struct Overlay {
    pub price: f64,
    /// 颜色按角色语义存为运行时色板值——GPUI 色板是运行时全局，不能在状态里存死
    pub color: gpui::Rgba,
    pub label: String,
}

/// 图表拖拽平移的载荷标记（on_drag 用；实际平移量在 on_drag_move 里按光标差分）
#[derive(Default)]
pub struct DragChart;

fn time_label(ms: i64, interval: Interval) -> String {
    use chrono::TimeZone;
    let t = match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => t,
        None => return "—".into(),
    };
    match interval {
        Interval::MO1 => t.format("%Y-%m").to_string(),
        Interval::D1 | Interval::D3 | Interval::W1 => t.format("%m-%d").to_string(),
        _ => t.format("%H:%M").to_string(),
    }
}

fn time_label_full(ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        None => "—".into(),
    }
}

/// 选择"好看"的网格步长
fn nice_step(range: f64, target: usize) -> f64 {
    if range <= 0.0 {
        return 1.0;
    }
    let raw = range / target.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if raw <= m * mag {
            return m * mag;
        }
    }
    10.0 * mag
}

// ---------- 绘制原语 ----------

/// 单个绘制原语（prepaint 产出、paint 执行）；顺序即图层顺序
enum Prim {
    /// 圆角矩形（radius/边框可选；radius 取宽高一半即圆点）
    Quad {
        bounds: gpui::Bounds<Pixels>,
        bg: Hsla,
        radius: Pixels,
        border_w: Pixels,
        border_c: Hsla,
    },
    /// 描边路径（折线/虚线；坐标为窗口绝对坐标）。
    /// Path 装箱：paint 层按序消费，堆上转移避免 184B 的大变体拷贝
    Path { path: Box<gpui::Path<Pixels>>, color: Hsla },
    /// 已排版的单行文本（origin 为行盒左上角；ShapedLine 装箱：排版行
    /// 体积大（内含 run 向量元数据），场景里文本原语数量多，按值入枚举
    /// 拉大全体 Prim 的栈占用
    Text {
        line: Box<gpui::ShapedLine>,
        origin: Point<Pixels>,
        line_height: Pixels,
    },
}

/// 文本锚点（对应 egui Align2 的常用子集）
enum Anchor {
    LeftCenter,
    CenterCenter,
    LeftTop,
    RightTop,
    CenterBottom,
}

/// 排版并压入一个文本原语。行盒高取 ascent+descent（自然行高），
/// 各锚点据此换算原点——文字自身已带颜色（TextRun）
#[allow(clippy::too_many_arguments)]
fn push_text(
    out: &mut Vec<Prim>,
    text: String,
    size: f32,
    mono: bool,
    color: impl Into<Hsla>,
    window: &Window,
    anchor: Anchor,
    pos: Point<Pixels>,
) {
    let color: Hsla = color.into();
    let font = Font {
        family: if mono { FONT_MONO } else { FONT_UI }.into(),
        ..gpui::font("")
    };
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text),
        px(size),
        &[run],
        None,
    );
    let lh = line.ascent + line.descent;
    let w = line.width;
    let origin = match anchor {
        Anchor::LeftCenter => point(pos.x, pos.y - lh / 2.0),
        Anchor::CenterCenter => point(pos.x - w / 2.0, pos.y - lh / 2.0),
        Anchor::LeftTop => point(pos.x, pos.y),
        Anchor::RightTop => point(pos.x - w, pos.y),
        Anchor::CenterBottom => point(pos.x - w / 2.0, pos.y - lh),
    };
    out.push(Prim::Text {
        line: Box::new(line),
        origin,
        line_height: lh,
    });
}

fn push_quad(out: &mut Vec<Prim>, bounds: gpui::Bounds<Pixels>, bg: impl Into<Hsla>, radius: Pixels) {
    out.push(Prim::Quad {
        bounds,
        bg: bg.into(),
        radius,
        border_w: px(0.0),
        border_c: Hsla::default(),
    });
}


/// f32 坐标快捷构造（场景几何全部以 f32 运算，最后统一转 Pixels）
fn pt(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

/// 圆点（egui circle_filled 对应物：方形 quad + 全圆角）
fn push_dot(out: &mut Vec<Prim>, center: Point<Pixels>, radius: Pixels, color: impl Into<Hsla>) {
    let size = radius * 2.0;
    out.push(Prim::Quad {
        bounds: gpui::Bounds::from_corner_and_size(
            gpui::Corner::TopLeft,
            point(center.x - radius, center.y - radius),
            gpui::size(size, size),
        ),
        bg: color.into(),
        radius,
        border_w: px(0.0),
        border_c: Hsla::default(),
    });
}

/// 折线路径（升序点位直连；egui Shape::line 对应物）
fn push_line(out: &mut Vec<Prim>, pts: &[Point<Pixels>], width: f32, color: impl Into<Hsla>) {
    let Some(&first) = pts.first() else { return };
    let mut b = PathBuilder::stroke(px(width));
    b.move_to(first);
    for p in &pts[1..] {
        b.line_to(*p);
    }
    if let Ok(path) = b.build() {
        out.push(Prim::Path {
            path: Box::new(path),
            color: color.into(),
        });
    }
}

/// 虚线路径（egui Shape::dashed_line 对应物；dash=实段/空段长度）
fn push_dashed(
    out: &mut Vec<Prim>,
    a: Point<Pixels>,
    b_: Point<Pixels>,
    width: f32,
    color: impl Into<Hsla>,
    dash: f32,
    gap: f32,
) {
    let mut b = PathBuilder::stroke(px(width)).dash_array(&[px(dash), px(gap)]);
    b.move_to(a);
    b.line_to(b_);
    if let Ok(path) = b.build() {
        out.push(Prim::Path {
            path: Box::new(path),
            color: color.into(),
        });
    }
}

// ---------- 可见段数据（render 期产出，纯数值不依赖布局） ----------

/// 可见段单根 K 线（Decimal 保原文供 tooltip 精度直出）
struct VisBar {
    t: i64,
    o: Decimal,
    h: Decimal,
    l: Decimal,
    c: Decimal,
    v: Decimal,
}

/// 副图面板绘制数据：可见段切片（NaN → None），范围已扫描
struct SubPanel {
    lines: Vec<(String, Hsla, Vec<Option<f64>>)>,
    bars: Option<Vec<Option<f64>>>,
    dots_up: Option<Vec<Option<f64>>>,
    dots_down: Option<Vec<Option<f64>>>,
    guides: Vec<f64>,
    lo: f64,
    hi: f64,
}

/// canvas prepaint 的全部输入
struct ChartData {
    bars: Vec<VisBar>,
    ma_vals: Vec<[Option<f64>; 4]>,
    ma_on: [bool; 4],
    overlays: Vec<Overlay>,
    sub_panel: Option<SubPanel>,
    max_vol: f64,
    hi: f32,
    span: f32,
    axis_w: f32,
    bars_f: f32,
    interval: Interval,
    symbol: String,
    dp: usize,
    qdp: usize,
    mouse: Option<Point<Pixels>>,
}

fn role_color(r: Role) -> gpui::Rgba {
    let t = cur();
    match r {
        Role::Accent => t.accent,
        Role::Warn => t.warn,
        Role::Up => t.up,
        Role::Down => t.down,
        Role::Dim => t.text_dim,
    }
}

// ---------- 主入口 ----------

/// 图表区：工具行（周期直选 / MA 开关 / 副图指标 / 最大化）+ canvas
pub fn show(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let state = app.state.clone();
    let dp = state.with_rule(app.market, &app.page.symbol, |r| {
        r.map(|x| x.price_dp() as usize).unwrap_or(2)
    });
    // 数量按 step 小数位显示（与下单/订单簿同精度），同样走纯 Decimal 路径
    let qdp = state.with_rule(app.market, &app.page.symbol, |r| {
        r.map(|x| x.qty_dp() as usize).unwrap_or(3)
    });

    // ---------- 头部工具行 ----------
    // 第一行：周期直选（1m…1M）单独成行；第二行：MA 开关（左）+ 副图指标/最大化（右）
    let tools = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .pb(px(theme::SPACE_SM))
        .child(tools_intervals(app, cx))
        .child(tools_ma(app, cx));

    // 叠加线：当前挂单（含止损触发价）+ 合约持仓开仓价
    let mut overlays: Vec<Overlay> = Vec::new();
    if state.has_creds() {
        // 匹配项先在短锁内筛出克隆，再在锁外格式化绘制：orders/acct 是引擎
        // 每个订单/账户事件都要写的热锁，跨整个收集循环持有（lifetime
        // extension 会让 guard 活到循环结束）会阻塞用户数据流写入。
        // 与同文件 md 锁的纪律一致（"绝不在绘制期间持有"）
        let sym = app.page.symbol.clone();
        let my_orders: Vec<_> = state
            .orders
            .lock()
            .open(app.market)
            .iter()
            .filter(|o| o.symbol == sym)
            .cloned()
            .collect();
        for o in &my_orders {
            let color = if o.side == "BUY" {
                cur().up
            } else {
                cur().down
            };
            let side_txt = if o.side == "BUY" {
                crate::i18n::tr("买")
            } else {
                crate::i18n::tr("卖")
            };
            if let Some(p) = o.price.filter(|p| !p.is_zero()) {
                overlays.push(Overlay {
                    price: widgets::f64_decimal(&p),
                    color,
                    label: format!(
                        "{}{} {}",
                        crate::i18n::tr("挂单 "),
                        side_txt,
                        widgets::fmt_dec_fixed(&p, dp)
                    ),
                });
            } else if let Some(s) = o.stop_price.filter(|s| !s.is_zero()) {
                overlays.push(Overlay {
                    price: widgets::f64_decimal(&s),
                    color,
                    label: format!(
                        "{}{} {}",
                        crate::i18n::tr("触发 "),
                        side_txt,
                        widgets::fmt_dec_fixed(&s, dp)
                    ),
                });
            }
        }
        if app.market == crate::binance::Market::UmFutures {
            let my_positions: Vec<_> = state
                .acct
                .lock()
                .fut_positions
                .iter()
                .filter(|p| p.symbol == sym && p.amt != Decimal::ZERO)
                .cloned()
                .collect();
            for p in &my_positions {
                overlays.push(Overlay {
                    price: widgets::f64_decimal(&p.entry_price),
                    color: cur().accent,
                    label: format!(
                        "{}{}",
                        crate::i18n::tr("持仓开仓 "),
                        // 开仓价按交易对 tick 精度显示：币安返回的 entryPrice
                        // 原文带双精度尾数（如 86997.7000000001），全精度直出
                        // 会把服务端浮点残差展示给用户
                        widgets::fmt_dec_fixed(&p.entry_price, dp)
                    ),
                });
            }
        }
    }

    // 短暂持锁：仅当缓存键（市场/交易对/周期/kline版本）变化时才克隆一次 K 线
    // 并重建 SMA 前缀和；md 是全局行情锁，绝不在绘制期间持有
    let interval;
    {
        let md_guard = state.md.lock();
        let Some(md) = md_guard.get(app.market, &app.page.symbol) else {
            return tools.child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(widgets::loading_state(crate::i18n::tr("加载行情中…"))),
            );
        };
        let key = (
            app.market,
            app.page.symbol.clone(),
            md.interval,
            md.kline_version,
        );
        if app.page.chart.cache_key.as_ref() != Some(&key) {
            app.page.chart.cache_klines = md.klines.clone();
            app.page.chart.cache_closes = md
                .klines
                .iter()
                .map(|c| widgets::f64_decimal(&c.c))
                .collect();
            let mut prefix = Vec::with_capacity(app.page.chart.cache_closes.len() + 1);
            prefix.push(0.0f64);
            for v in &app.page.chart.cache_closes {
                prefix.push(prefix.last().unwrap() + v);
            }
            app.page.chart.cache_prefix = prefix;
            app.page.chart.cache_key = Some(key);
        }
        // 副图指标序列：仅在 (数据版本, 指标) 变化时重算一次（与 K 线缓存同粒度）
        let ind = app.page.sub_indicator;
        let skey = (
            app.market,
            app.page.symbol.clone(),
            md.interval,
            md.kline_version,
            ind,
        );
        if app.page.chart.sub_key.as_ref() != Some(&skey) {
            app.page.chart.sub_series = if ind == SubIndicator::Vol {
                None
            } else {
                Some(indicators::compute(ind, &md.klines))
            };
            app.page.chart.sub_key = Some(skey);
        }
        interval = md.interval;
    }
    if app.page.chart.cache_klines.is_empty() {
        return tools.child(
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .child(widgets::loading_state(crate::i18n::tr(
                    "暂无K线数据（等待加载或检查网络/代理）",
                ))),
        );
    }

    // ---------- 可见段与映射参数（render 期算好，prepaint 只按 bounds 定位） ----------
    let n = app.page.chart.cache_klines.len() as i64;
    let bars_f = app.page.chart.bars_visible.min(n as f32).max(10.0);
    let end = (n - app.page.chart.right_off as i64).clamp(1, n);
    let start = (end - bars_f as i64).max(0);
    let visible = &app.page.chart.cache_klines[start as usize..end as usize];
    if visible.is_empty() {
        return tools.child(div().flex_1());
    }
    let bars: Vec<VisBar> = visible
        .iter()
        .map(|c| VisBar {
            t: c.t,
            o: c.o,
            h: c.h,
            l: c.l,
            c: c.c,
            v: c.v,
        })
        .collect();

    // closes/prefix 由调用方按 kline_version 缓存传入，
    // SMA 查询 O(1)：(prefix[gi+1]-prefix[gi+1-period])/period
    let prefix = app.page.chart.cache_prefix.clone();
    let sma_at = |gi: usize, period: usize| -> Option<f64> {
        if gi + 1 < period || gi + 1 >= prefix.len() {
            return None;
        }
        Some((prefix[gi + 1] - prefix[gi + 1 - period]) / period as f64)
    };
    let ma_on = app.page.chart.ma;
    let mut ma_vals: Vec<[Option<f64>; 4]> = Vec::with_capacity(bars.len());
    for gi in start as usize..end as usize {
        let mut row = [None; 4];
        for (k, &period) in MA_PERIODS.iter().enumerate() {
            row[k] = sma_at(gi, period);
        }
        ma_vals.push(row);
    }

    // 价格范围：可见 OHLC + 开启的均线 + 叠加线
    let mut hi = f64::MIN;
    let mut lo = f64::MAX;
    let mut max_vol = f64::MIN;
    for b in &bars {
        hi = hi.max(widgets::f64_decimal(&b.h));
        lo = lo.min(widgets::f64_decimal(&b.l));
        max_vol = max_vol.max(widgets::f64_decimal(&b.v));
    }
    for row in &ma_vals {
        for (k, v) in row.iter().enumerate() {
            if !ma_on[k] {
                continue;
            }
            if let Some(v) = v {
                if *v > 0.0 {
                    hi = hi.max(*v);
                    lo = lo.min(*v);
                }
            }
        }
    }
    for ov in &overlays {
        if ov.price > 0.0 {
            hi = hi.max(ov.price);
            lo = lo.min(ov.price);
        }
    }
    if hi <= lo {
        return tools.child(div().flex_1());
    }
    let span = (hi - lo).max(0.0001);
    let pad = span * 0.06;
    let hi = hi + pad;
    let lo = lo - pad;
    // 高价位（~1e9）下 f32 分辨率可能把填充后的 hi/lo 收敛到同一个值：
    // 钳一个非零 span，防止 y_of 除零产生 NaN
    let hi32 = hi as f32;
    let lo32 = lo as f32;
    let span32 = (hi32 - lo32).max(hi32.abs() * 1e-6).max(1e-6);

    // 右侧价格轴宽度自适应数字位数（千分位），避免大数值被截断。
    // 以 dp=2 的宽度保底：同一交易对现货/期货 tick 小数位可能不同
    // （如 BTCUSDT 现货 2 位、期货 1 位），只按当前市场位数取宽会让
    // K 线区右缘（烛区/价格轴分界）切市场时横移几像素；按较宽者
    // 预留后分割线在两市场位置一致
    let axis_chars = |d: f64| {
        (widgets::fmt_thousands(hi, d as usize).chars().count() as f32) * 8.2 + 16.0
    };
    let axis_w = axis_chars(2.0).max(axis_chars(dp as f64)).clamp(58.0, 120.0);

    // ---------- 副图数据：可见段切片 + 范围扫描 ----------
    let sub_panel = if app.page.sub_indicator == SubIndicator::Vol {
        None
    } else {
        let sub = app.page.chart.sub_series.clone().unwrap_or_default();
        Some(build_sub_panel(&sub, start as usize, end as usize, &ma_on))
    };

    let mouse = app
        .chart_mouse
        .filter(|_| app.chart_hover)
        .map(|(x, y)| gpui::point(px(x), px(y)));
    let data = ChartData {
        bars,
        ma_vals,
        ma_on,
        overlays,
        sub_panel,
        max_vol: max_vol.max(0.0),
        hi: hi32,
        span: span32,
        axis_w,
        bars_f,
        interval,
        symbol: app.page.symbol.clone(),
        dp,
        qdp,
        mouse,
    };
    app.chart_n = n as usize;

    // ---------- canvas：prepaint 产场景，paint 执行 ----------
    let canvas = gpui::canvas(
        move |bounds, window, _cx| build_scene(bounds, &data, window),
        move |_bounds, scene, window, _cx| {
            for p in scene {
                paint_prim(p, window, _cx);
            }
        },
    )
    .size_full();

    // ---------- 交互层：滚轮缩放 / 拖拽平移 / 双击复位 / 悬停十字光标 ----------
    let area = div()
        .id("chart_area")
        .relative()
        .flex_1()
        .w_full()
        .overflow_hidden()
        .on_mouse_move(cx.listener(|app: &mut BnApp, e: &gpui::MouseMoveEvent, _, cx| {
            app.chart_mouse = Some((f32::from(e.position.x), f32::from(e.position.y)));
            cx.notify();
        }))
        .on_hover(cx.listener(|app: &mut BnApp, hovered: &bool, _, cx| {
            app.chart_hover = *hovered;
            if !*hovered {
                app.chart_mouse = None;
            }
            cx.notify();
        }))
        .on_scroll_wheel(cx.listener(|app: &mut BnApp, e: &ScrollWheelEvent, _, cx| {
            let dy = f32::from(e.delta.pixel_delta(theme::font_body()).y);
            if dy.abs() > 0.5 {
                let f = (1.0 + dy * 0.0015).clamp(0.6, 1.6);
                app.page.chart.bars_visible =
                    (app.page.chart.bars_visible * f).clamp(20.0, 480.0);
                cx.notify();
            }
        }))
        // 平移会话基线：按下时清零差分基准，避免跨会话跳变
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|app: &mut BnApp, _, _, _| {
                app.pan_last = None;
            }),
        )
        .on_drag(DragChart, |_, _, _, cx| cx.new(|_| widgets::DragGhost))
        .on_drag_move(cx.listener(|app: &mut BnApp, e: &DragMoveEvent<DragChart>, _, cx| {
            let x = f32::from(e.event.position.x);
            let last = app.pan_last.replace(x);
            if let Some(d) = last.map(|l| x - l) {
                if d != 0.0 {
                    // bar_w 用面板宽估算即可：只影响拖拽手感，不影响精度
                    let bar_w = (f32::from(e.bounds.size.width)
                        / app.page.chart.bars_visible.max(1.0))
                    .max(0.5);
                    let max_off = (app.chart_n as f32 - 10.0).max(0.0);
                    app.page.chart.right_off =
                        (app.page.chart.right_off + d / bar_w).clamp(0.0, max_off);
                    cx.notify();
                }
            }
        }))
        .on_click(cx.listener(|app: &mut BnApp, e: &ClickEvent, _, cx| {
            if e.click_count() == 2 {
                app.page.chart.right_off = 0.0;
                cx.notify();
            }
        }))
        // 点图表任意处收起副图指标下拉（与选择/按钮互斥的兜底）
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|app: &mut BnApp, _, _, cx| {
                if app.sub_menu_open {
                    app.sub_menu_open = false;
                    cx.notify();
                }
            }),
        )
        .child(canvas);

    let mut root = div().flex_1().w_full().flex().flex_col().overflow_hidden();
    if app.sub_menu_open {
        // 副图指标下拉菜单：铺在工具行之下的覆盖层（root 相对定位锚点）
        root = root.child(sub_menu(app, cx));
    }
    root.child(tools).child(area)
}

/// 工具行第一行：周期直选（pill_tab_sm，与页头同一样式）
fn tools_intervals(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let mut row = div().w_full().flex().flex_row().items_center().gap(px(theme::SPACE_XS));
    for (i, iv) in Interval::all().into_iter().enumerate() {
        row = row.child(
            widgets::pill_tab_sm(("civ", i), iv.label(), app.page.interval == iv).on_click(
                cx.listener(move |app: &mut BnApp, _, _, cx| {
                    app.switch_interval(iv);
                    cx.notify();
                }),
            ),
        );
    }
    row
}

/// 工具行第二行：MA 开关（左，MA 专属色淡填充）+ 副图指标下拉/最大化（右）
fn tools_ma(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let t = cur();
    let ma_colors = theme::ma_colors();
    let mut row = div()
        .w_full()
        .h(px(theme::BTN_H))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_XS));
    for (i, &period) in MA_PERIODS.iter().enumerate() {
        let on = app.page.chart.ma[i];
        let color = ma_colors[i];
        row = row.child(
            div()
                .id(ElementId::from(("ma", i)))
                .px(px(theme::SPACE_MD))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::R_SM))
                .text_size(font_sm())
                .whitespace_nowrap()
                .cursor_pointer()
                .text_color(if on { color } else { t.text_dim })
                .when(on, |d| d.bg(faded(color, 0.14)))
                .when(!on, |d| d.hover(|s| s.bg(t.row_hover)))
                .child(format!("MA{period}"))
                .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                    app.page.chart.ma[i] = !app.page.chart.ma[i];
                    cx.notify();
                })),
        );
    }
    row = row.child(div().flex_1());
    // 最大化/还原（文字按钮；⛶ 字形在等宽字体优先下有缺字风险）
    let max_txt = if app.chart_maximized {
        crate::i18n::tr("还原")
    } else {
        crate::i18n::tr("全屏")
    };
    row = row.child(
        widgets::btn_default("chart_max", max_txt, true).on_click(cx.listener(
            |app: &mut BnApp, _, _, cx| {
                app.chart_maximized = !app.chart_maximized;
                cx.notify();
            },
        )),
    );
    // 副图指标选择按钮（下拉菜单见 sub_menu；标签中英文一致无需 i18n）
    let sel = app.page.sub_indicator.label();
    row = row.child(
        widgets::btn_default("sub_indicator", format!("{sel} ▾"), true).on_click(cx.listener(
            |app: &mut BnApp, _, _, cx| {
                app.sub_menu_open = !app.sub_menu_open;
                cx.notify();
            },
        )),
    );
    row
}

/// 副图指标下拉菜单：覆盖层（root 相对定位，铺在工具行之下右缘）
fn sub_menu(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let t = cur();
    let mut menu = div()
        .absolute()
        .top(px(58.0))
        .right(px(4.0))
        .w(px(110.0))
        .py(px(theme::SPACE_SM))
        .rounded(px(theme::R_MD))
        .bg(t.card)
        .border_1()
        .border_color(faded(t.border, 0.7))
        .shadow_sm();
    for (i, ind) in SubIndicator::ALL.into_iter().enumerate() {
        let selected = app.page.sub_indicator == ind;
        menu = menu.child(
            div()
                .id(ElementId::from(("subind", i)))
                .px(px(10.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .text_size(font_sm())
                .whitespace_nowrap()
                .cursor_pointer()
                .text_color(if selected { t.accent } else { t.text })
                .when(selected, |d| d.bg(faded(t.accent, 0.12)))
                .hover(|s| s.bg(t.row_hover))
                .child(ind.label())
                .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
                    app.page.sub_indicator = ind;
                    app.sub_menu_open = false;
                    cx.notify();
                })),
        );
    }
    menu
}

/// 副图可见段切片与范围扫描：NaN → None；范围含参考线与零轴
fn build_sub_panel(sub: &SubSeries, start: usize, end: usize, _ma: &[bool; 4]) -> SubPanel {
    // 全部序列的公共有效长度（序列可能比 K 线短：预热段）
    let mut len = usize::MAX;
    for (_, _, arr) in &sub.lines {
        len = len.min(arr.len());
    }
    if let Some(b) = &sub.bars {
        len = len.min(b.len());
    }
    for d in [&sub.dots_up, &sub.dots_down].into_iter().flatten() {
        len = len.min(d.len());
    }
    let start = start.min(len.saturating_sub(1));
    let end = end.min(len);

    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    let mut scan = |arr: &[f64]| {
        for &v in arr.iter().take(end).skip(start) {
            if v.is_nan() {
                continue;
            }
            lo = lo.min(v);
            hi = hi.max(v);
        }
    };
    let mut lines = Vec::new();
    for (name, role, arr) in &sub.lines {
        scan(arr);
        lines.push((
            (*name).to_string(),
            Hsla::from(role_color(*role)),
            arr.iter()
                .take(end)
                .skip(start)
                .map(|v| (!v.is_nan()).then_some(*v))
                .collect(),
        ));
    }
    let mut bars = None;
    if let Some(b) = &sub.bars {
        scan(b);
        bars = Some(
            b.iter()
                .take(end)
                .skip(start)
                .map(|v| (!v.is_nan()).then_some(*v))
                .collect(),
        );
    }
    let mut dots_up = None;
    if let Some(d) = &sub.dots_up {
        scan(d);
        dots_up = Some(
            d.iter()
                .take(end)
                .skip(start)
                .map(|v| (!v.is_nan()).then_some(*v))
                .collect(),
        );
    }
    let mut dots_down = None;
    if let Some(d) = &sub.dots_down {
        scan(d);
        dots_down = Some(
            d.iter()
                .take(end)
                .skip(start)
                .map(|v| (!v.is_nan()).then_some(*v))
                .collect(),
        );
    }
    if sub.zero_based {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    for &g in &sub.guides {
        lo = lo.min(g);
        hi = hi.max(g);
    }
    if lo > hi {
        // 可见段全为 NaN（预热中）——交给 prepaint 的空面板兜底
        lo = 0.0;
        hi = 0.0;
    }
    if hi - lo < 1e-12 {
        hi += 1e-12;
        lo -= 1e-12;
    }
    SubPanel {
        lines,
        bars,
        dots_up,
        dots_down,
        guides: sub.guides.clone(),
        lo,
        hi,
    }
}

// ---------- 场景构建（prepaint） ----------

/// 按 bounds 算几何并 shape 文本，产出有序绘制原语。
/// 坐标一律窗口绝对（canvas bounds 即窗口坐标，paint_quad/paint_path 同域）
fn build_scene(bounds: gpui::Bounds<Pixels>, d: &ChartData, window: &Window) -> Vec<Prim> {
    let mut s: Vec<Prim> = Vec::with_capacity(1024);
    let t = cur();

    // 恢复动画等过渡帧里图表高度可能不足 8px：shrink 会使 rect 上下颠倒
    // （min>max），price_h 随之为负，最新价虚线的 clamp 会触发 panic。钳非负
    let plot = gpui::Bounds::from_corner_and_size(
        gpui::Corner::TopLeft,
        point(bounds.origin.x + px(4.0), bounds.origin.y + px(4.0)),
        gpui::size(
            (bounds.size.width - px(8.0)).max(px(0.0)),
            (bounds.size.height - px(8.0)).max(px(0.0)),
        ),
    );
    if plot.size.width <= px(0.0) || plot.size.height <= px(0.0) {
        return s;
    }

    // ---------- 品牌水印（先画，沉在所有元素底下） ----------
    let wm_alpha = if theme::is_light() { 6.0 / 255.0 } else { 9.0 / 255.0 };
    push_text(
        &mut s,
        d.symbol.clone(),
        f32::from(theme::font_watermark()),
        false,
        faded(t.text, wm_alpha),
        window,
        Anchor::CenterCenter,
        point(
            plot.origin.x + plot.size.width / 2.0,
            plot.origin.y + plot.size.height / 2.0 - plot.size.height * 0.10,
        ),
    );

    let price_h = f32::from(plot.size.height) * 0.82;
    let vol_h = f32::from(plot.size.height) * 0.14;
    let hi = d.hi;
    let span = d.span;
    let chart_w = (f32::from(plot.size.width) - d.axis_w).max(0.0);
    let y_of = |p: f32| f32::from(plot.origin.y) + (hi - p) / span * price_h;
    let p_of = |y: f32| hi - (y - f32::from(plot.origin.y)) / price_h * span;
    let bar_w = chart_w / d.bars_f;
    let x_of = |i: usize| f32::from(plot.origin.x) + (i as f32 + 0.5) * bar_w;
    let plot_left = f32::from(plot.origin.x);
    let plot_top = f32::from(plot.origin.y);
    let plot_bottom = plot_top + f32::from(plot.size.height);

    // ---------- 网格与价格轴 ----------
    let step = nice_step(span as f64, 6);
    // 网格点用整数索引计算（gp = idx * step）：f32 累加在 step 极小（横盘品种
    // span 收缩到 ULP 量级）时会低于 ULP 不再前进，循环永不终止冻结 UI
    let lo64 = (hi - span) as f64;
    let mut idx = (lo64 / step).floor() as i64;
    loop {
        let gp64 = idx as f64 * step;
        if gp64 > hi as f64 {
            break;
        }
        let gp = gp64 as f32;
        if gp >= lo64 as f32 {
            let y = y_of(gp);
            push_quad(
                &mut s,
                gpui::Bounds::from_corner_and_size(
                    gpui::Corner::TopLeft,
                    pt(plot_left, y),
                    gpui::size(px(chart_w), px(0.5)),
                ),
                t.grid,
                px(0.0),
            );
            push_text(
                &mut s,
                widgets::fmt_thousands(gp64, d.dp),
                f32::from(font_sm()),
                true,
                t.text_dim,
                window,
                Anchor::LeftCenter,
                pt(plot_left + chart_w + 6.0, y),
            );
        }
        idx += 1;
    }

    // ---------- 时间轴 ----------
    let stride = ((d.bars.len() as f32) / 7.0).ceil().max(1.0) as usize;
    for (i, b) in d.bars.iter().enumerate() {
        if i % stride != 0 {
            continue;
        }
        let x = x_of(i);
        push_text(
            &mut s,
            time_label(b.t, d.interval),
            f32::from(font_sm()),
            false,
            t.text_dim,
            window,
            Anchor::CenterBottom,
            pt(x, plot_bottom - 2.0),
        );
        push_quad(
            &mut s,
            gpui::Bounds::from_corner_and_size(
                gpui::Corner::TopLeft,
                pt(x, plot_top),
                gpui::size(px(0.4), px(f32::from(plot.size.height))),
            ),
            t.grid_time,
            px(0.0),
        );
    }

    // ---------- 成交量 / 副图指标 ----------
    // VOL 走原生成交量路径；其余指标在副图区域绘制
    let sub_top = plot_top + price_h;
    match &d.sub_panel {
        None => {
            for (i, b) in d.bars.iter().enumerate() {
                if d.max_vol <= 0.0 {
                    continue;
                }
                let v = widgets::f64_decimal(&b.v);
                if v <= 0.0 {
                    continue;
                }
                let vh = (v / d.max_vol * vol_h as f64) as f32;
                let o = widgets::f64_decimal(&b.o);
                let close = widgets::f64_decimal(&b.c);
                let col = if close >= o { t.up } else { t.down };
                let x = x_of(i);
                let w = (bar_w * 0.7).max(1.0);
                push_quad(
                    &mut s,
                    gpui::Bounds::from_corner_and_size(
                        gpui::Corner::TopLeft,
                        pt(x - w / 2.0, sub_top + vol_h - vh),
                        gpui::size(px(w), px(vh)),
                    ),
                    faded(col, 0.45),
                    px(0.0),
                );
            }
        }
        Some(p) => draw_sub_panel(&mut s, p, plot_left, sub_top, chart_w, vol_h, bar_w, window),
    }

    // ---------- MA 均线 ----------
    let ma_colors = theme::ma_colors();
    for (k, _) in MA_PERIODS.iter().enumerate() {
        if !d.ma_on[k] {
            continue;
        }
        let pts: Vec<Point<Pixels>> = d
            .ma_vals
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                row[k].map(|v| pt(x_of(i), y_of(v as f32)))
            })
            .collect();
        push_line(&mut s, &pts, 1.3, ma_colors[k]);
    }

    // ---------- 蜡烛 ----------
    for (i, b) in d.bars.iter().enumerate() {
        let o = widgets::f64_decimal(&b.o);
        let h = widgets::f64_decimal(&b.h);
        let l = widgets::f64_decimal(&b.l);
        let close = widgets::f64_decimal(&b.c);
        let col = if close >= o { t.up } else { t.down };
        let x = x_of(i);
        let w = (bar_w * 0.7).max(1.0);
        // 影线（1px 竖条）
        let yh = y_of(h as f32);
        let yl = y_of(l as f32);
        push_quad(
            &mut s,
            gpui::Bounds::from_corner_and_size(
                gpui::Corner::TopLeft,
                pt(x - 0.5, yh.min(yl)),
                gpui::size(px(1.0), px((yl - yh).max(1.0))),
            ),
            col,
            px(0.0),
        );
        // 实体（最低 1px，保证十字星可见）
        let y_o = y_of(o as f32);
        let y_c = y_of(close as f32);
        let top = y_o.min(y_c);
        let bottom = (y_o.max(y_c)).max(top + 1.0);
        push_quad(
            &mut s,
            gpui::Bounds::from_corner_and_size(
                gpui::Corner::TopLeft,
                pt(x - w / 2.0, top),
                gpui::size(px(w), px(bottom - top)),
            ),
            col,
            px(0.0),
        );
    }

    // ---------- 最新价 ----------
    let last_i = d.bars.len() - 1;
    let last_close = widgets::f64_decimal(&d.bars[last_i].c);
    let prev_close = if last_i > 0 {
        widgets::f64_decimal(&d.bars[last_i - 1].c)
    } else {
        widgets::f64_decimal(&d.bars[last_i].o)
    };
    let yl = y_of(last_close as f32).clamp(plot_top, plot_top + price_h);
    let col = if last_close >= prev_close { t.up } else { t.down };
    push_dashed(
        &mut s,
        pt(plot_left, yl),
        pt(plot_left + chart_w, yl),
        1.0,
        col,
        5.0,
        4.0,
    );
    let tag = widgets::fmt_thousands(last_close, d.dp);
    let tag_w = (d.axis_w - 8.0).max(20.0);
    push_quad(
        &mut s,
        gpui::Bounds::from_corner_and_size(
            gpui::Corner::TopLeft,
            pt(plot_left + chart_w + 4.0, yl - 8.0),
            gpui::size(px(tag_w), px(16.0)),
        ),
        col,
        px(3.0),
    );
    push_text(
        &mut s,
        tag,
        f32::from(font_sm()),
        true,
        t.on_fill,
        window,
        Anchor::CenterCenter,
        pt(plot_left + chart_w + 4.0 + tag_w / 2.0, yl),
    );

    // ---------- 叠加线（挂单/持仓） ----------
    for ov in &d.overlays {
        if ov.price <= 0.0 {
            continue;
        }
        let y = y_of(ov.price as f32);
        if y < plot_top || y > plot_top + price_h {
            continue;
        }
        push_dashed(
            &mut s,
            pt(plot_left, y),
            pt(plot_left + chart_w, y),
            1.0,
            faded(ov.color, 0.8),
            6.0,
            4.0,
        );
        push_text(
            &mut s,
            ov.label.clone(),
            f32::from(font_sm()),
            false,
            ov.color,
            window,
            Anchor::LeftCenter,
            pt(plot_left + 6.0, y - 8.0),
        );
    }

    // ---------- 十字光标与 tooltip ----------
    if let Some(m) = d.mouse {
        let mx = f32::from(m.x) - f32::from(bounds.origin.x);
        let my = f32::from(m.y) - f32::from(bounds.origin.y);
        if mx >= plot_left
            && mx <= plot_left + chart_w
            && my >= plot_top
            && my <= plot_bottom
        {
            let cross = t.crosshair;
            // 定位 K 线：x 吸附到最近蜡烛中心（竖线与底部时间标签随之对齐）
            let idx = (((mx - plot_left) / bar_w).round().max(0.0) as usize)
                .min(d.bars.len() - 1);
            let cx_ = x_of(idx);
            push_dashed(
                &mut s,
                pt(cx_, plot_top),
                pt(cx_, plot_bottom),
                0.7,
                cross,
                4.0,
                3.0,
            );
            push_dashed(
                &mut s,
                pt(plot_left, my),
                pt(plot_left + chart_w, my),
                0.7,
                cross,
                4.0,
                3.0,
            );
            // 悬停价格：tag_bg 圆角小贴片（与底部时间标签同款，比裸文字醒目）
            let ptxt = widgets::fmt_thousands(p_of(my) as f64, d.dp);
            let pw = measure_w(&ptxt, true, font_sm().into(), window) + 8.0;
            let ph = f32::from(font_sm()) * 1.4 + 4.0;
            push_quad(
                &mut s,
                gpui::Bounds::from_corner_and_size(
                    gpui::Corner::TopLeft,
                    pt(plot_left + chart_w + 4.0, my - ph / 2.0),
                    gpui::size(px(pw), px(ph)),
                ),
                t.tag_bg,
                px(3.0),
            );
            push_text(
                &mut s,
                ptxt,
                f32::from(font_sm()),
                true,
                t.text,
                window,
                Anchor::CenterCenter,
                pt(plot_left + chart_w + 4.0 + pw / 2.0, my),
            );
            // OHLC tooltip（四行文本 + 边框贴片；贴近右/下缘时向内翻折）
            let b = &d.bars[idx];
            let o = widgets::f64_decimal(&b.o);
            let h = widgets::f64_decimal(&b.h);
            let l = widgets::f64_decimal(&b.l);
            let close = widgets::f64_decimal(&b.c);
            let chg = if o != 0.0 { (close - o) / o * 100.0 } else { 0.0 };
            let lines = [
                time_label_full(b.t),
                format!(
                    "{} {}  {} {}",
                    crate::i18n::tr("开"),
                    widgets::fmt_thousands(o, d.dp),
                    crate::i18n::tr("高"),
                    widgets::fmt_thousands(h, d.dp)
                ),
                format!(
                    "{} {}  {} {}",
                    crate::i18n::tr("低"),
                    widgets::fmt_thousands(l, d.dp),
                    crate::i18n::tr("收"),
                    widgets::fmt_thousands(close, d.dp)
                ),
                format!(
                    "{} {}   {}{:.2}%",
                    crate::i18n::tr("量"),
                    widgets::fmt_dec_plain(&b.v, d.qdp),
                    if chg >= 0.0 { "+" } else { "" },
                    chg
                ),
            ];
            let md_sz = f32::from(font_md());
            let widths: Vec<f32> = lines
                .iter()
                .map(|l| measure_w(l, true, md_sz, window))
                .collect();
            let tw = widths.iter().fold(0.0f32, |a, &b| a.max(b));
            let th = lines.len() as f32 * md_sz * 1.45;
            let mut tx = mx + 14.0;
            let mut ty = my + 14.0;
            if tx + tw + 16.0 > plot_left + chart_w {
                tx = mx - tw - 26.0;
            }
            if ty + th + 12.0 > plot_bottom {
                ty = my - th - 24.0;
            }
            push_quad(
                &mut s,
                gpui::Bounds::from_corner_and_size(
                    gpui::Corner::TopLeft,
                    pt(tx, ty),
                    gpui::size(px(tw + 16.0), px(th + 12.0)),
                ),
                faded(t.tooltip_bg, 0.97),
                px(3.0),
            );
            for (li, txt) in lines.iter().enumerate() {
                push_text(
                    &mut s,
                    txt.clone(),
                    md_sz,
                    true,
                    t.text,
                    window,
                    Anchor::LeftTop,
                    pt(tx + 8.0, ty + 6.0 + li as f32 * md_sz * 1.45),
                );
            }
            // 底部时间标签
            let t_txt = time_label_full(b.t);
            let t_w = measure_w(&t_txt, true, font_sm().into(), window) + 10.0;
            let t_h = f32::from(font_sm()) * 1.4 + 4.0;
            let tx = (cx_ - t_w / 2.0).clamp(plot_left, plot_left + (chart_w - t_w).max(0.0));
            push_quad(
                &mut s,
                gpui::Bounds::from_corner_and_size(
                    gpui::Corner::TopLeft,
                    pt(tx, plot_bottom - t_h),
                    gpui::size(px(t_w), px(t_h)),
                ),
                t.tag_bg,
                px(2.0),
            );
            push_text(
                &mut s,
                t_txt,
                f32::from(font_sm()),
                true,
                t.text,
                window,
                Anchor::CenterCenter,
                pt(tx + t_w / 2.0, plot_bottom - t_h / 2.0),
            );
        }
    }

    // 操作提示放右上角：底部是时间轴标签区，放左下会与时间刻度重叠
    push_text(
        &mut s,
        crate::i18n::tr("滚轮缩放 · 拖动平移 · 双击回最新").to_string(),
        f32::from(font_sm()),
        false,
        faded(t.text_dim, 0.8),
        window,
        Anchor::RightTop,
        pt(plot_left + chart_w - 8.0, plot_top + 8.0),
    );

    // ---------- 左上角图例：交易对 · 周期 + 最新 MA 值 ----------
    // 先铺 tag_bg 圆角贴片再画文字：图例直接叠在 K 线上时穿越蜡烛可读性差
    let last_gi = d.bars.len() - 1;
    let ma_colors = theme::ma_colors();
    let mut rows: Vec<(String, bool, Hsla)> = vec![(
        format!("{} · {}", d.symbol, d.interval.label()),
        false,
        Hsla::from(t.text_dim),
    )];
    for (k, &period) in MA_PERIODS.iter().enumerate() {
        if !d.ma_on[k] {
            continue;
        }
        let val = d
            .ma_vals
            .get(last_gi)
            .and_then(|r| r[k])
            .map(|v| widgets::fmt_thousands(v, d.dp))
            .unwrap_or_else(|| "—".into());
        rows.push((format!("MA{period} {val}"), true, Hsla::from(ma_colors[k])));
    }
    let sm_sz = f32::from(font_sm());
    let legend_w = rows
        .iter()
        .map(|(txt, mono, _)| measure_w(txt, *mono, sm_sz, window))
        .fold(0.0f32, f32::max);
    let legend_h = rows.len() as f32 * 14.0 + 4.0;
    push_quad(
        &mut s,
        gpui::Bounds::from_corner_and_size(
            gpui::Corner::TopLeft,
            pt(plot_left + 4.0, plot_top + 4.0),
            gpui::size(px(legend_w + 12.0), px(legend_h)),
        ),
        faded(t.tag_bg, 0.75),
        px(4.0),
    );
    let mut ly = plot_top + 4.0 + 2.0;
    for (ri, (txt, mono, color)) in rows.iter().enumerate() {
        push_text(
            &mut s,
            txt.clone(),
            sm_sz,
            *mono,
            *color,
            window,
            Anchor::LeftTop,
            pt(plot_left + 8.0, ly),
        );
        ly += if ri == 0 { 16.0 } else { 14.0 };
    }

    s
}

/// 预量文本宽（tooltip/图例贴片尺寸用；窗口坐标域，不需要精确到像素级）
fn measure_w(text: &str, mono: bool, size: f32, window: &Window) -> f32 {
    let font = Font {
        family: if mono { FONT_MONO } else { FONT_UI }.into(),
        ..gpui::font("")
    };
    let run = TextRun {
        len: text.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(SharedString::from(text.to_string()), px(size), &[run], None)
            .width,
    )
}

/// 副图指标面板：在 [plot_left, plot_left+chart_w] × [top, top+vol_h] 绘制
/// 线/柱/点/参考线。切片可能为空（预热段全 NaN）：范围无效时只画分隔线
#[allow(clippy::too_many_arguments)]
fn draw_sub_panel(
    s: &mut Vec<Prim>,
    p: &SubPanel,
    plot_left: f32,
    top: f32,
    chart_w: f32,
    vol_h: f32,
    bar_w: f32,
    window: &Window,
) {
    let t = cur();
    let bottom = top + vol_h;

    // 面板上下边界线（与主图分隔）
    for y in [top, bottom] {
        push_quad(
            s,
            gpui::Bounds::from_corner_and_size(
                gpui::Corner::TopLeft,
                pt(plot_left, y),
                gpui::size(px(chart_w), px(0.5)),
            ),
            t.grid,
            px(0.0),
        );
    }

    let valid = p.hi > p.lo;
    if !valid {
        return;
    }
    let span = (p.hi - p.lo) * 1.0 / 0.84; // 8% 上下留白
    let mid = (p.hi + p.lo) / 2.0;
    let lo = mid - span / 2.0;
    let y_of = |v: f64| bottom - ((v - lo) / span) as f32 * vol_h;
    let y0 = y_of(0.0);
    let x_of = |i: usize| plot_left + (i as f32 + 0.5) * bar_w;

    // 参考线：虚线 + 右侧数值
    for &g in &p.guides {
        let y = y_of(g);
        if y < top || y > bottom {
            continue;
        }
        push_dashed(
            s,
            pt(plot_left, y),
            pt(plot_left + chart_w, y),
            0.5,
            t.grid,
            4.0,
            3.0,
        );
        push_text(
            s,
            format!("{g:.0}"),
            f32::from(font_sm()),
            true,
            t.text_dim,
            window,
            Anchor::LeftCenter,
            pt(plot_left + chart_w + 4.0, y),
        );
    }

    // 柱状序列（MACD 快慢差），正负分色，自零轴起画
    if let Some(b) = &p.bars {
        for (i, v) in b.iter().enumerate() {
            let Some(v) = v else { continue };
            let col = if *v >= 0.0 { t.up } else { t.down };
            let y = y_of(*v);
            let ytop = y.min(y0);
            let h = (y.max(y0) - ytop).max(1.0);
            let x = x_of(i);
            let w = (bar_w * 0.7).max(1.0);
            push_quad(
                s,
                gpui::Bounds::from_corner_and_size(
                    gpui::Corner::TopLeft,
                    pt(x - w / 2.0, ytop),
                    gpui::size(px(w), px(h)),
                ),
                faded(col, 0.45),
                px(0.0),
            );
        }
    }

    // 折线序列（NaN 断点分段），1.2px
    for (_name, col, arr) in &p.lines {
        let mut pts: Vec<Point<Pixels>> = Vec::new();
        let flush = |pts: &mut Vec<Point<Pixels>>, s: &mut Vec<Prim>| {
            if pts.len() > 1 {
                let seg = std::mem::take(pts);
                push_line(s, &seg, 1.2, *col);
            } else {
                pts.clear();
            }
        };
        for (i, v) in arr.iter().enumerate() {
            match v {
                None => flush(&mut pts, s),
                Some(v) => pts.push(pt(x_of(i), y_of(*v) as f64 as f32)),
            }
        }
        flush(&mut pts, s);
    }

    // 点序列（SAR 趋势点），按多空分色
    for (dots, col) in [(&p.dots_up, t.up), (&p.dots_down, t.down)] {
        let Some(d) = dots else { continue };
        for (i, v) in d.iter().enumerate() {
            let Some(v) = v else { continue };
            push_dot(s, pt(x_of(i), y_of(*v) as f64 as f32), px(1.6), col);
        }
    }

    // 图例：线名横排于面板左上角
    let mut lx = plot_left + 8.0;
    for (name, col, _) in &p.lines {
        push_text(
            s,
            name.clone(),
            f32::from(font_sm()),
            true,
            *col,
            window,
            Anchor::LeftTop,
            pt(lx, top + 2.0),
        );
        lx += measure_w(name, true, font_sm().into(), window) + 10.0;
    }
}

/// 执行一个绘制原语
fn paint_prim(p: Prim, window: &mut Window, _cx: &mut gpui::App) {
    match p {
        Prim::Quad {
            bounds,
            bg,
            radius,
            border_w,
            border_c,
        } => {
            window.paint_quad(quad(
                bounds,
                radius,
                bg,
                Edges::all(border_w),
                border_c,
                gpui::BorderStyle::Solid,
            ));
        }
        Prim::Path { path, color } => window.paint_path(*path, color),
        Prim::Text {
            line,
            origin,
            line_height,
        } => {
            let _ = line.paint(origin, line_height, window, _cx);
        }
    }
}
