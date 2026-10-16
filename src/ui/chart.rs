//! 自绘蜡烛图：缩放/平移、十字光标、最新价与挂单/持仓叠加线、副图指标

use crate::app::BnApp;
use crate::binance::model::Candle;
use crate::binance::Interval;
use crate::ui::indicators::{self, Role, SubSeries};
use crate::ui::theme;
use crate::ui::widgets;
use chrono::TimeZone;
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
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
            ma: [true, true, true, true],
            cache_key: None,
            cache_klines: Vec::new(),
            cache_closes: Vec::new(),
            cache_prefix: Vec::new(),
            sub_key: None,
            sub_series: None,
        }
    }
}

pub struct Overlay {
    pub price: f64,
    pub color: Color32,
    pub label: String,
}

pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = app.state.clone();
    let dp = state.with_rule(app.market, &app.page.symbol, |r| {
        r.map(|x| x.price_dp()).unwrap_or(2)
    });
    // 数量按 step 小数位显示（与下单/订单簿同精度），同样走纯 Decimal 路径
    let qdp = state.with_rule(app.market, &app.page.symbol, |r| {
        r.map(|x| x.qty_dp()).unwrap_or(3)
    }) as usize;

    // ---------- 头部工具行 ----------
    // 第一行：周期直选（1m…1M）单独成行，不再与 MA 开关挤在一起；
    // 第二行：MA 开关（左）+ 副图指标切换/最大化（右）
    let ma_colors = theme::ma_colors();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        // 周期直选：pill_tab_sm 胶囊，与页头同一样式
        for iv in Interval::all() {
            if theme::pill_tab_sm(ui, iv.label(), app.page.interval == iv).clicked() {
                app.switch_interval(iv);
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, &period) in MA_PERIODS.iter().enumerate() {
            let on = app.page.chart.ma[i];
            let label = format!("MA{period}");
            // 仿 pill_tab 自绘：选中 = MA 专属色淡填充 + 同色文字，
            // 不吃全局 selection 蓝（与金色 accent + ma_colors 体系冲突）；
            // 填充透明度随 animate_bool 淡入淡出
            let galley = ui.painter().layout_no_wrap(
                label.clone(),
                egui::FontId::proportional(theme::FONT_SM),
                if on {
                    ma_colors[i]
                } else {
                    theme::cur().text_dim
                },
            );
            let size = egui::vec2(galley.size().x + 12.0, theme::BTN_H);
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let anim = ui
                .ctx()
                .animate_bool(egui::Id::new("ma_toggle").with(i), on);
            let mut fill = ma_colors[i].gamma_multiply(0.14 * anim);
            if resp.hovered() && !on {
                fill = theme::cur().row_hover;
            }
            ui.painter().rect_filled(rect, f32::from(theme::R_SM), fill);
            ui.painter().galley(
                rect.center() - galley.size() / 2.0,
                galley,
                if on {
                    ma_colors[i]
                } else {
                    theme::cur().text_dim
                },
            );
            let mut resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            // 说明带上当前开关状态：开/关仅靠填充色区分会撞"色即唯一编码"
            // 的红线，tooltip 与可访问名字都要把状态说出来
            resp = resp.on_hover_text(format!(
                "MA{period} · {} · {}",
                if on {
                    crate::i18n::tr("已开")
                } else {
                    crate::i18n::tr("已关")
                },
                crate::i18n::tr("点按切换均线，图上叠加 MA5/10/20/60"),
            ));
            // 自绘热区补可访问名字（含开关状态）：键盘 Tab 聚焦后
            // Enter/Space 亦可触发（Sense::click 的热区可获焦）
            resp.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    true,
                    on,
                    format!("MA{period}"),
                )
            });
            if resp.clicked() {
                app.page.chart.ma[i] = !app.page.chart.ma[i];
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // ComboBox 按钮高度取 spacing.interact_size.y（默认 18 < BTN_H=28），
            // 显式抬到 BTN_H 与全屏按钮等高（行内布局已是 Align::Center 居中对齐）
            ui.spacing_mut().interact_size.y = theme::BTN_H;
            // 最大化/还原：文字按钮（⛶ 字形在等宽字体优先下有缺字风险）
            let max_txt = if app.chart_maximized {
                crate::i18n::tr("还原")
            } else {
                crate::i18n::tr("全屏")
            };
            if theme::btn_default(ui, max_txt, true).clicked() {
                app.chart_maximized = !app.chart_maximized;
            }
            // 副图指标切换（标签中英文一致，无需 i18n）
            egui::ComboBox::from_id_salt("sub_indicator")
                .selected_text(app.page.sub_indicator.label())
                .width(74.0)
                .show_ui(ui, |ui| {
                    for ind in SubIndicator::ALL {
                        ui.selectable_value(&mut app.page.sub_indicator, ind, ind.label());
                    }
                });
        });
    });
    ui.separator();

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
                theme::cur().up
            } else {
                theme::cur().down
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
                        widgets::fmt_dec_fixed(&p, dp as usize)
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
                        widgets::fmt_dec_fixed(&s, dp as usize)
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
                    color: theme::cur().accent,
                    label: format!(
                        "{}{}",
                        crate::i18n::tr("持仓开仓 "),
                        // 开仓价按交易对 tick 精度显示：币安返回的 entryPrice
                        // 原文带双精度尾数（如 86997.7000000001），全精度直出
                        // 会把服务端浮点残差展示给用户
                        widgets::fmt_dec_fixed(&p.entry_price, dp as usize)
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
            widgets::placeholder(ui, ui.max_rect(), crate::i18n::tr("加载行情中…"));
            return;
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
        widgets::placeholder(
            ui,
            ui.max_rect(),
            crate::i18n::tr("暂无K线数据（等待加载或检查网络/代理）"),
        );
        return;
    }
    // draw 需要 &mut app（交互状态）又引用缓存数据：先把缓存整体取出，
    // 用独立于 app 的本地借用绘制，结束后归还
    let (kls, closes, prefix) = (
        std::mem::take(&mut app.page.chart.cache_klines),
        std::mem::take(&mut app.page.chart.cache_closes),
        std::mem::take(&mut app.page.chart.cache_prefix),
    );
    let sub = if app.page.sub_indicator == SubIndicator::Vol {
        None
    } else {
        Some(std::mem::take(&mut app.page.chart.sub_series).unwrap_or_default())
    };
    draw(
        ui,
        app,
        &kls,
        &prefix,
        interval,
        dp,
        qdp as u32,
        &overlays,
        sub.as_ref(),
    );
    app.page.chart.cache_klines = kls;
    app.page.chart.cache_closes = closes;
    app.page.chart.cache_prefix = prefix;
    if let Some(s) = sub {
        app.page.chart.sub_series = Some(s);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    ui: &mut egui::Ui,
    app: &mut BnApp,
    candles: &[Candle],
    prefix: &[f64],
    interval: Interval,
    dp: u32,
    qdp: u32,
    overlays: &[Overlay],
    sub: Option<&SubSeries>,
) {
    // 取光标之后的剩余矩形：max_rect 会把全高控件分配在工具行之下，
    // 响应矩形向下溢出面板、抢走下方订单区的点击/拖拽
    let rect = ui.available_rect_before_wrap();
    let (resp, painter) = ui.allocate_painter(rect.size(), Sense::click_and_drag());
    // 恢复动画等过渡帧里图表高度可能不足 8px：shrink(4.0) 会使 rect 上下颠倒
    // （min>max），price_h 随之为负，最新价虚线的 clamp 触发 "min > max" panic
    // （实测崩溃点）。这里钳到非负尺寸
    let shrunk = resp.rect.shrink(4.0);
    let plot = egui::Rect::from_min_size(
        shrunk.min,
        egui::vec2(shrunk.width().max(0.0), shrunk.height().max(0.0)),
    );
    let t = theme::cur();

    // ---------- 品牌水印（先画，沉在所有元素底下） ----------
    let wm_alpha = if theme::is_light() { 6 } else { 9 };
    let txt = theme::cur().text;
    painter.text(
        plot.center() - Vec2::new(0.0, plot.height() * 0.10),
        Align2::CENTER_CENTER,
        &app.page.symbol,
        FontId::proportional(theme::FONT_WATERMARK),
        Color32::from_rgba_unmultiplied(txt.r(), txt.g(), txt.b(), wm_alpha),
    );

    // ---------- 交互 ----------
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta).y;
        if scroll.abs() > 0.5 {
            let f = (1.0 + scroll * 0.0015).clamp(0.6, 1.6);
            app.page.chart.bars_visible = (app.page.chart.bars_visible * f).clamp(20.0, 480.0);
        }
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        let bar_w = plot.width() / app.page.chart.bars_visible.max(1.0);
        // 上界取 max(0)，K 线不足 10 根时 clamp 不会 panic
        let max_off = (candles.len() as f32 - 10.0).max(0.0);
        app.page.chart.right_off = (app.page.chart.right_off + d.x / bar_w).clamp(0.0, max_off);
    }
    if resp.double_clicked() {
        app.page.chart.right_off = 0.0;
    }

    let n = candles.len() as i64;
    // 空数据保护：clamp(1, n) 在 n==0 时是 lo>hi 直接 panic。
    // 当前唯一调用方在缓存为空时已提前 return，但 draw 自身不能
    // 依赖这一约定（未来新增调用方不能再埋同一个崩溃点）
    if n == 0 {
        return;
    }
    let bars = app.page.chart.bars_visible.min(n as f32).max(10.0);
    let end = (n - app.page.chart.right_off as i64).clamp(1, n);
    let start = (end - bars as i64).max(0);
    let visible = &candles[start as usize..end as usize];
    if visible.is_empty() {
        return;
    }

    // ---------- 价格范围 ----------
    // closes/prefix 由调用方按 kline_version 缓存传入（见 show），
    // SMA 查询 O(1)：(prefix[gi+1]-prefix[gi+1-period])/period
    let sma_at = |gi: usize, period: usize| -> Option<f64> {
        if gi + 1 < period || gi + 1 >= prefix.len() {
            return None;
        }
        Some((prefix[gi + 1] - prefix[gi + 1 - period]) / period as f64)
    };
    let ma_colors = theme::ma_colors();
    let vis_ohlc: Vec<[f64; 4]> = visible
        .iter()
        .map(|c| {
            [
                widgets::f64_decimal(&c.o),
                widgets::f64_decimal(&c.h),
                widgets::f64_decimal(&c.l),
                widgets::f64_decimal(&c.c),
            ]
        })
        .collect();
    let vis_vol: Vec<f64> = visible.iter().map(|c| widgets::f64_decimal(&c.v)).collect();

    let pad_ratio = 0.06;
    let price_h = plot.height() * 0.82;
    let vol_h = plot.height() * 0.14;

    let mut hi = f64::MIN;
    let mut lo = f64::MAX;
    let mut max_vol = f64::MIN;
    for c in &vis_ohlc {
        hi = hi.max(c[1]);
        lo = lo.min(c[2]);
    }
    for v in &vis_vol {
        max_vol = max_vol.max(*v);
    }
    // 可见范围的均线也纳入范围
    for gi in start as usize..end as usize {
        for (k, &period) in MA_PERIODS.iter().enumerate() {
            if !app.page.chart.ma[k] {
                continue;
            }
            if let Some(v) = sma_at(gi, period) {
                if v > 0.0 {
                    hi = hi.max(v);
                    lo = lo.min(v);
                }
            }
        }
    }
    // 叠加线也纳入范围
    for ov in overlays {
        if ov.price > 0.0 {
            hi = hi.max(ov.price);
            lo = lo.min(ov.price);
        }
    }
    let span = (hi - lo).max(0.0001);
    let hi = (hi + span * pad_ratio) as f32;
    let lo = (lo - span * pad_ratio) as f32;
    // 高价位（~1e9）下 f32 分辨率可能把填充后的 hi/lo 收敛到同一个值：
    // 钳一个非零 span，防止 y_of 除零产生 NaN
    let span = (hi - lo).max(hi.abs() * 1e-6).max(1e-6);

    // 右侧价格轴宽度自适应数字位数（千分位），避免大数值被截断
    let axis_w = ((widgets::fmt_thousands(hi as f64, dp as usize)
        .chars()
        .count() as f32)
        * 8.2
        + 16.0)
        .clamp(58.0, 120.0);
    // 窄窗口下轴宽可能超过 plot 宽：钳非负，防后续 clamp 的 min>max panic
    let chart_w = (plot.width() - axis_w).max(0.0);

    let y_of = |p: f32| plot.top() + (hi - p) / span * price_h;
    let p_of = |y: f32| hi - (y - plot.top()) / price_h * span;
    let bar_w = chart_w / bars;
    let x_of = |i: usize| plot.left() + (i as f32 + 0.5) * bar_w;
    // 副图区域（成交量或技术指标共用同一块空间）
    let sub_rect = Rect::from_min_max(
        Pos2::new(plot.left(), plot.top() + price_h),
        Pos2::new(plot.left() + chart_w, plot.top() + price_h + vol_h),
    );

    // ---------- 网格与价格轴 ----------
    let step = nice_step(span as f64, 6);
    let grid_font = FontId::monospace(theme::FONT_SM);
    // 网格点用整数索引计算（gp = idx * step）：f32 累加在 step 极小（横盘品种
    // span 收缩到 ULP 量级）时会低于 ULP 不再前进，循环永不终止冻结 UI
    let mut idx = (lo as f64 / step).floor() as i64;
    loop {
        let gp64 = idx as f64 * step;
        if gp64 > hi as f64 {
            break;
        }
        let gp = gp64 as f32;
        if gp >= lo {
            let y = y_of(gp);
            painter.line_segment(
                [
                    Pos2::new(plot.left(), y),
                    Pos2::new(plot.left() + chart_w, y),
                ],
                Stroke::new(0.5_f32, t.grid),
            );
            painter.text(
                Pos2::new(plot.left() + chart_w + 6.0, y),
                Align2::LEFT_CENTER,
                widgets::fmt_thousands(gp64, dp as usize),
                grid_font.clone(),
                theme::cur().text_dim,
            );
        }
        idx += 1;
    }

    // ---------- 时间轴 ----------
    let stride = ((visible.len() as f32) / 7.0).ceil().max(1.0) as usize;
    for (idx, c) in visible.iter().enumerate() {
        if idx % stride != 0 {
            continue;
        }
        let x = x_of(idx);
        let label = time_label(c.t, interval);
        painter.text(
            Pos2::new(x, plot.bottom() - 2.0),
            Align2::CENTER_BOTTOM,
            label,
            FontId::proportional(theme::FONT_SM),
            theme::cur().text_dim,
        );
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(0.4_f32, t.grid_time),
        );
    }

    // ---------- 成交量 / 副图指标 ----------
    // VOL 走原生成交量路径（保持既有渲染不变）；其余指标在副图区域绘制
    match sub {
        None => {
            for (idx, _) in visible.iter().enumerate() {
                let v = vis_vol[idx];
                if max_vol <= 0.0 {
                    continue;
                }
                let vh = (v / max_vol * vol_h as f64) as f32;
                let o = vis_ohlc[idx][0];
                let close = vis_ohlc[idx][3];
                let col = if close >= o {
                    theme::cur().up.gamma_multiply(0.45)
                } else {
                    theme::cur().down.gamma_multiply(0.45)
                };
                let x = x_of(idx);
                let w = (bar_w * 0.7).max(1.0);
                painter.rect_filled(
                    Rect::from_center_size(
                        Pos2::new(x, plot.top() + price_h + vol_h - vh / 2.0),
                        Vec2::new(w, vh),
                    ),
                    0.0,
                    col,
                );
            }
        }
        Some(s) => {
            let start_us = start.max(0) as usize;
            let end_us = end.max(0) as usize;
            draw_sub_panel(&painter, sub_rect, s, start_us, end_us, bar_w, &x_of);
        }
    }

    // ---------- MA 均线 ----------
    for (k, &period) in MA_PERIODS.iter().enumerate() {
        if !app.page.chart.ma[k] {
            continue;
        }
        let mut pts: Vec<Pos2> = Vec::new();
        for (local, gi) in (start as usize..end as usize).enumerate() {
            if let Some(v) = sma_at(gi, period) {
                pts.push(Pos2::new(x_of(local), y_of(v as f32)));
            }
        }
        if pts.len() > 1 {
            painter.add(Shape::line(pts, Stroke::new(1.3_f32, ma_colors[k])));
        }
    }

    // ---------- 蜡烛 ----------
    for (idx, _) in visible.iter().enumerate() {
        let [o, h, l, close] = vis_ohlc[idx];
        let col = if close >= o {
            theme::cur().up
        } else {
            theme::cur().down
        };
        let x = x_of(idx);
        let w = (bar_w * 0.7).max(1.0);
        // 影线
        painter.line_segment(
            [Pos2::new(x, y_of(h as f32)), Pos2::new(x, y_of(l as f32))],
            Stroke::new(1.0_f32, col),
        );
        // 实体（最低 1px，保证十字星可见）
        let y_o = y_of(o as f32);
        let y_c = y_of(close as f32);
        let top = y_o.min(y_c);
        let bottom = (y_o.max(y_c)).max(top + 1.0);
        let body = Rect::from_min_max(Pos2::new(x - w / 2.0, top), Pos2::new(x + w / 2.0, bottom));
        painter.rect_filled(body, 0.0, col);
    }

    // ---------- 最新价 ----------
    let last = vis_ohlc[vis_ohlc.len() - 1];
    let last_close = last[3];
    let prev_close = if vis_ohlc.len() > 1 {
        vis_ohlc[vis_ohlc.len() - 2][3]
    } else {
        last[0]
    };
    let yl = y_of(last_close as f32).clamp(plot.top(), plot.top() + price_h);
    let col = if last_close >= prev_close {
        theme::cur().up
    } else {
        theme::cur().down
    };
    painter.extend(Shape::dashed_line(
        &[
            Pos2::new(plot.left(), yl),
            Pos2::new(plot.left() + chart_w, yl),
        ],
        Stroke::new(1.0_f32, col),
        5.0,
        4.0,
    ));
    let tag = widgets::fmt_thousands(last_close, dp as usize);
    let tag_size = egui::vec2(axis_w - 8.0, 16.0);
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(plot.left() + chart_w + 4.0, yl - tag_size.y / 2.0),
            tag_size,
        ),
        3.0,
        col,
    );
    painter.text(
        Pos2::new(plot.left() + chart_w + 4.0 + tag_size.x / 2.0, yl),
        Align2::CENTER_CENTER,
        tag,
        FontId::monospace(theme::FONT_SM),
        theme::cur().on_fill,
    );

    // ---------- 叠加线（挂单/持仓） ----------
    for ov in overlays {
        if ov.price <= 0.0 {
            continue;
        }
        let y = y_of(ov.price as f32);
        if y < plot.top() || y > plot.top() + price_h {
            continue;
        }
        painter.extend(Shape::dashed_line(
            &[
                Pos2::new(plot.left(), y),
                Pos2::new(plot.left() + chart_w, y),
            ],
            Stroke::new(1.0_f32, ov.color.gamma_multiply(0.8)),
            6.0,
            4.0,
        ));
        painter.text(
            Pos2::new(plot.left() + 6.0, y - 8.0),
            Align2::LEFT_CENTER,
            &ov.label,
            FontId::proportional(theme::FONT_SM),
            ov.color,
        );
    }

    // ---------- 十字光标与 tooltip ----------
    if let Some(hp) = resp.hover_pos() {
        if plot.contains(hp) && hp.x < plot.left() + chart_w {
            let cross = Stroke::new(0.7_f32, t.crosshair);
            // 定位 K 线：x 吸附到最近蜡烛中心（竖线与底部时间标签随之对齐）
            let idx =
                ((((hp.x - plot.left()) / bar_w).round().max(0.0)) as usize).min(visible.len() - 1);
            let cx = x_of(idx);
            painter.extend(Shape::dashed_line(
                &[Pos2::new(cx, plot.top()), Pos2::new(cx, plot.bottom())],
                cross,
                4.0,
                3.0,
            ));
            painter.extend(Shape::dashed_line(
                &[
                    Pos2::new(plot.left(), hp.y),
                    Pos2::new(plot.left() + chart_w, hp.y),
                ],
                cross,
                4.0,
                3.0,
            ));
            // 悬停价格：tag_bg 圆角小贴片（与底部时间标签同款，比裸文字醒目）
            let ptxt = widgets::fmt_thousands(p_of(hp.y) as f64, dp as usize);
            let pgal = painter.layout_no_wrap(ptxt, grid_font.clone(), theme::cur().text);
            let psz = pgal.size() + Vec2::new(8.0, 4.0);
            let prect = Rect::from_min_size(
                Pos2::new(plot.left() + chart_w + 4.0, hp.y - psz.y / 2.0),
                psz,
            );
            painter.rect_filled(prect, 3.0, t.tag_bg);
            painter.galley(prect.min + Vec2::new(4.0, 2.0), pgal, theme::cur().text);
            let c = &visible[idx];
            let [o, h, l, close] = vis_ohlc[idx];
            let chg = if o != 0.0 {
                (close - o) / o * 100.0
            } else {
                0.0
            };
            let lines = format!(
                "{}\n{} {}  {} {}\n{} {}  {} {}\n{} {}   {}{:.2}%",
                time_label_full(c.t),
                crate::i18n::tr("开"),
                widgets::fmt_thousands(o, dp as usize),
                crate::i18n::tr("高"),
                widgets::fmt_thousands(h, dp as usize),
                crate::i18n::tr("低"),
                widgets::fmt_thousands(l, dp as usize),
                crate::i18n::tr("收"),
                widgets::fmt_thousands(close, dp as usize),
                crate::i18n::tr("量"),
                widgets::fmt_dec_plain(&c.v, qdp as usize),
                if chg >= 0.0 { "+" } else { "" },
                chg
            );
            let font = FontId::monospace(theme::FONT_MD);
            let galley = painter.layout_no_wrap(lines.clone(), font.clone(), theme::cur().text);
            let mut pos = hp + Vec2::new(14.0, 14.0);
            let sz = galley.size() + Vec2::new(16.0, 12.0);
            if pos.x + sz.x > plot.right() {
                pos.x = hp.x - sz.x - 10.0;
            }
            if pos.y + sz.y > plot.bottom() {
                pos.y = hp.y - sz.y - 10.0;
            }
            painter.rect_filled(
                Rect::from_min_size(pos, sz),
                3.0,
                t.tooltip_bg.gamma_multiply(0.97),
            );
            painter.rect_stroke(
                Rect::from_min_size(pos, sz),
                3.0,
                Stroke::new(1.0_f32, theme::cur().border),
                egui::StrokeKind::Inside,
            );
            painter.text(
                pos + Vec2::new(8.0, 6.0),
                Align2::LEFT_TOP,
                lines,
                font,
                theme::cur().text,
            );
            // 底部时间标签
            let t_txt = time_label_full(c.t);
            let t_galley = painter.layout_no_wrap(
                t_txt.clone(),
                FontId::monospace(theme::FONT_SM),
                theme::cur().text,
            );
            let tsz = t_galley.size() + Vec2::new(10.0, 4.0);
            // 标签比图表宽时上界会小于下界：钳非负防 clamp panic
            let tx =
                (cx - tsz.x / 2.0).clamp(plot.left(), plot.left() + (chart_w - tsz.x).max(0.0));
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(tx, plot.bottom() - tsz.y), tsz),
                2.0,
                t.tag_bg,
            );
            painter.text(
                Pos2::new(tx + tsz.x / 2.0, plot.bottom() - tsz.y / 2.0),
                Align2::CENTER_CENTER,
                t_txt,
                FontId::monospace(theme::FONT_SM),
                theme::cur().text,
            );
        }
    }

    // 操作提示放右上角：底部是时间轴标签区，放左下会与时间刻度重叠
    painter.text(
        Pos2::new(plot.left() + chart_w - 8.0, plot.top() + 8.0),
        Align2::RIGHT_TOP,
        crate::i18n::tr("滚轮缩放 · 拖动平移 · 双击回最新"),
        FontId::proportional(theme::FONT_SM),
        theme::cur().text_dim.gamma_multiply(0.8),
    );

    // ---------- 左上角图例：交易对 · 周期 + 最新 MA 值 ----------
    // 先铺 tag_bg 圆角贴片再画文字：图例直接叠在 K 线上时穿越蜡烛可读性差
    let last_gi = end as usize - 1;
    let mut legend_y = plot.top() + 8.0;
    let mut legend_rows: Vec<(String, FontId, Color32)> = vec![(
        format!("{} · {}", app.page.symbol, interval.label()),
        FontId::proportional(theme::FONT_SM),
        theme::cur().text_dim,
    )];
    for (k, &period) in MA_PERIODS.iter().enumerate() {
        if !app.page.chart.ma[k] {
            continue;
        }
        let val = sma_at(last_gi, period)
            .map(|v| widgets::fmt_thousands(v, dp as usize))
            .unwrap_or_else(|| "—".into());
        legend_rows.push((
            format!("MA{period} {val}"),
            FontId::monospace(theme::FONT_SM),
            ma_colors[k],
        ));
    }
    let legend_w = legend_rows
        .iter()
        .map(|(s, f, _)| {
            painter
                .layout_no_wrap(s.clone(), f.clone(), Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0_f32, |a, b| a.max(b));
    let legend_h = legend_rows.len() as f32 * 14.0 + 4.0;
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(plot.left() + 4.0, plot.top() + 4.0),
            egui::vec2(legend_w + 12.0, legend_h),
        ),
        4.0,
        theme::cur().tag_bg.gamma_multiply(0.75),
    );
    for (ri, (txt, font, color)) in legend_rows.iter().enumerate() {
        painter.text(
            Pos2::new(plot.left() + 8.0, legend_y),
            Align2::LEFT_TOP,
            txt,
            font.clone(),
            *color,
        );
        legend_y += if ri == 0 { 16.0 } else { 14.0 };
    }
}

fn role_color(r: Role) -> Color32 {
    let t = theme::cur();
    match r {
        Role::Accent => t.accent,
        Role::Warn => t.warn,
        Role::Up => t.up,
        Role::Down => t.down,
        Role::Dim => t.text_dim,
    }
}

/// 副图指标面板：在 rect 上绘制 [start, end) 范围内的线/柱/点/参考线。
/// 序列可能比 K 线短（预热段 NaN / unwrap_or_default 空值），统一按最小长度钳制防越界。
/// 注意：start/end 是全局 K 线下标（序列对齐用），x_of 只接收可见区局部下标，
/// 绘制前必须做 `i - start` 转换，否则画到裁剪区外（表现为副图一片空白）。
fn draw_sub_panel(
    painter: &egui::Painter,
    rect: Rect,
    sub: &SubSeries,
    start: usize,
    end: usize,
    bar_w: f32,
    x_of: &impl Fn(usize) -> f32,
) {
    // 全部序列的公共有效长度
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
    if len == usize::MAX {
        return; // 没有任何序列，无可绘制内容
    }
    let start = start.min(len);
    let end = end.min(len);
    if start >= end {
        return;
    }

    let t = theme::cur();

    // 面板上下边界线（与主图分隔）
    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.top()),
            Pos2::new(rect.right(), rect.top()),
        ],
        Stroke::new(0.5_f32, t.grid),
    );
    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.bottom()),
            Pos2::new(rect.right(), rect.bottom()),
        ],
        Stroke::new(0.5_f32, t.grid),
    );

    // 范围：扫描可见段（跳过 NaN），合并参考线与零轴
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    let mut scan = |arr: &[f64]| {
        for &v in &arr[start..end] {
            if v.is_nan() {
                continue;
            }
            lo = lo.min(v);
            hi = hi.max(v);
        }
    };
    for (_, _, arr) in &sub.lines {
        scan(arr);
    }
    if let Some(b) = &sub.bars {
        scan(b);
    }
    for d in [&sub.dots_up, &sub.dots_down].into_iter().flatten() {
        scan(d);
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
        return; // 可见段全为 NaN（预热中）
    }
    if hi - lo < 1e-12 {
        hi += 1e-12;
        lo -= 1e-12;
    }
    let span = (hi - lo) * 1.0 / 0.84; // 8% 上下留白
    let mid = (hi + lo) / 2.0;
    let lo = mid - span / 2.0;
    let y_of = |v: f64| rect.bottom() - ((v - lo) / span) as f32 * rect.height();
    let y0 = y_of(0.0);

    // 参考线：虚线 + 右侧数值
    for &g in &sub.guides {
        let y = y_of(g);
        if y < rect.top() || y > rect.bottom() {
            continue;
        }
        painter.extend(Shape::dashed_line(
            &[Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(0.5_f32, t.grid),
            4.0,
            3.0,
        ));
        painter.text(
            Pos2::new(rect.right() + 4.0, y),
            Align2::LEFT_CENTER,
            format!("{g:.0}"),
            FontId::monospace(theme::FONT_SM),
            t.text_dim,
        );
    }

    // 柱状序列（MACD 快慢差），正负分色，自零轴起画
    if let Some(b) = &sub.bars {
        for (i, v) in b.iter().enumerate().take(end).skip(start) {
            let v = *v;
            if v.is_nan() {
                continue;
            }
            let col = if v >= 0.0 {
                t.up.gamma_multiply(0.45)
            } else {
                t.down.gamma_multiply(0.45)
            };
            let y = y_of(v);
            let top = y.min(y0);
            let h = (y.max(y0) - top).max(1.0);
            let x = x_of(i - start);
            let w = (bar_w * 0.7).max(1.0);
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x - w / 2.0, top), Pos2::new(x + w / 2.0, top + h)),
                0.0,
                col,
            );
        }
    }

    // 折线序列（NaN 断点分段），1.2px
    for (_name, role, arr) in &sub.lines {
        let col = role_color(*role);
        let mut pts: Vec<Pos2> = Vec::new();
        for (i, v) in arr.iter().enumerate().take(end).skip(start) {
            let v = *v;
            if v.is_nan() {
                if pts.len() > 1 {
                    painter.add(Shape::line(
                        std::mem::take(&mut pts),
                        Stroke::new(1.2_f32, col),
                    ));
                } else {
                    pts.clear();
                }
                continue;
            }
            pts.push(Pos2::new(x_of(i - start), y_of(v)));
        }
        if pts.len() > 1 {
            painter.add(Shape::line(pts, Stroke::new(1.2_f32, col)));
        }
    }

    // 点序列（SAR 趋势点），按多空分色
    for (dots, col) in [(&sub.dots_up, t.up), (&sub.dots_down, t.down)] {
        let Some(d) = dots else { continue };
        for (i, v) in d.iter().enumerate().take(end).skip(start) {
            let v = *v;
            if v.is_nan() {
                continue;
            }
            painter.circle_filled(Pos2::new(x_of(i - start), y_of(v)), 1.6, col);
        }
    }

    // 图例：线名横排于面板左上角
    let mut lx = rect.left() + 8.0;
    for (name, role, _) in &sub.lines {
        let col = role_color(*role);
        let gal =
            painter.layout_no_wrap((*name).to_string(), FontId::monospace(theme::FONT_SM), col);
        painter.galley(Pos2::new(lx, rect.top() + 2.0), gal.clone(), col);
        lx += gal.size().x + 10.0;
    }
}

fn time_label(ms: i64, interval: Interval) -> String {
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
