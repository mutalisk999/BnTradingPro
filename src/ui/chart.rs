//! 自绘蜡烛图：缩放/平移、十字光标、最新价与挂单/持仓叠加线

use crate::app::BnApp;
use crate::binance::model::Candle;
use crate::binance::Interval;
use crate::ui::theme;
use crate::ui::widgets;
use chrono::TimeZone;
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use rust_decimal::Decimal;

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

    // ---------- 头部工具行：MA 开关（24h 统计已移至页头） ----------
    let ma_colors = theme::ma_colors();
    ui.horizontal(|ui| {
        for (i, &period) in MA_PERIODS.iter().enumerate() {
            let label = egui::RichText::new(format!("MA{period}"))
                .size(11.5)
                .strong();
            let label = if app.page.chart.ma[i] {
                label.color(ma_colors[i])
            } else {
                label.color(theme::cur().text_dim)
            };
            if ui
                .add(
                    egui::Button::selectable(app.page.chart.ma[i], label)
                        .min_size(egui::vec2(0.0, 24.0)),
                )
                .clicked()
            {
                app.page.chart.ma[i] = !app.page.chart.ma[i];
            }
        }
    });
    ui.separator();

    // 叠加线：当前挂单（含止损触发价）+ 合约持仓开仓价
    let mut overlays: Vec<Overlay> = Vec::new();
    if state.has_creds() {
        for o in state.orders.lock().open(app.market).iter() {
            if o.symbol == app.page.symbol {
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
                        label: format!("{}{} {}", crate::i18n::tr("挂单 "), side_txt, fmt_norm(&p)),
                    });
                } else if let Some(s) = o.stop_price.filter(|s| !s.is_zero()) {
                    overlays.push(Overlay {
                        price: widgets::f64_decimal(&s),
                        color,
                        label: format!("{}{} {}", crate::i18n::tr("触发 "), side_txt, fmt_norm(&s)),
                    });
                }
            }
        }
        if app.market == crate::binance::Market::UmFutures {
            for p in state.acct.lock().fut_positions.iter() {
                if p.symbol == app.page.symbol && p.amt != Decimal::ZERO {
                    overlays.push(Overlay {
                        price: widgets::f64_decimal(&p.entry_price),
                        color: theme::cur().accent,
                        label: format!(
                            "{}{}",
                            crate::i18n::tr("持仓开仓 "),
                            fmt_norm(&p.entry_price)
                        ),
                    });
                }
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
        let key = (app.market, app.page.symbol.clone(), md.interval, md.kline_version);
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
    draw(ui, app, &kls, &prefix, interval, dp, &overlays);
    app.page.chart.cache_klines = kls;
    app.page.chart.cache_closes = closes;
    app.page.chart.cache_prefix = prefix;
}

fn fmt_norm(d: &Decimal) -> String {
    d.normalize().to_string()
}

fn draw(
    ui: &mut egui::Ui,
    app: &mut BnApp,
    candles: &[Candle],
    prefix: &[f64],
    interval: Interval,
    dp: u32,
    overlays: &[Overlay],
) {
    // 取光标之后的剩余矩形：max_rect 会把全高控件分配在工具行之下，
    // 响应矩形向下溢出面板、抢走下方订单区的点击/拖拽
    let rect = ui.available_rect_before_wrap();
    let (resp, painter) = ui.allocate_painter(rect.size(), Sense::click_and_drag());
    let plot = resp.rect.shrink(4.0);
    let t = theme::cur();

    // ---------- 品牌水印（先画，沉在所有元素底下） ----------
    let wm_alpha = if theme::is_light() { 6 } else { 9 };
    let txt = theme::cur().text;
    painter.text(
        plot.center() - Vec2::new(0.0, plot.height() * 0.10),
        Align2::CENTER_CENTER,
        &app.page.symbol,
        FontId::proportional(58.0),
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
    let chart_w = plot.width() - axis_w;

    let y_of = |p: f32| plot.top() + (hi - p) / span * price_h;
    let p_of = |y: f32| hi - (y - plot.top()) / price_h * span;
    let bar_w = chart_w / bars;
    let x_of = |i: usize| plot.left() + (i as f32 + 0.5) * bar_w;

    // ---------- 网格与价格轴 ----------
    let step = nice_step(span as f64, 6);
    let grid_font = FontId::monospace(11.0);
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
            FontId::proportional(10.5),
            theme::cur().text_dim,
        );
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(0.4_f32, t.grid_time),
        );
    }

    // ---------- 成交量 ----------
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
        FontId::monospace(11.0),
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
            FontId::proportional(10.5),
            ov.color,
        );
    }

    // ---------- 十字光标与 tooltip ----------
    if let Some(hp) = resp.hover_pos() {
        if plot.contains(hp) && hp.x < plot.left() + chart_w {
            let cross = Stroke::new(0.7_f32, t.crosshair);
            painter.extend(Shape::dashed_line(
                &[Pos2::new(hp.x, plot.top()), Pos2::new(hp.x, plot.bottom())],
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
            // 悬停价格
            painter.text(
                Pos2::new(plot.left() + chart_w + 6.0, hp.y),
                Align2::LEFT_CENTER,
                widgets::fmt_thousands(p_of(hp.y) as f64, dp as usize),
                grid_font.clone(),
                theme::cur().text,
            );
            // 定位 K 线
            let idx = (((hp.x - plot.left()) / bar_w) as usize).min(visible.len() - 1);
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
                fmt_norm(&c.v),
                if chg >= 0.0 { "+" } else { "" },
                chg
            );
            let font = FontId::monospace(12.0);
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
            let t_galley =
                painter.layout_no_wrap(t_txt.clone(), FontId::monospace(11.0), theme::cur().text);
            let tsz = t_galley.size() + Vec2::new(10.0, 4.0);
            let tx = (hp.x - tsz.x / 2.0).clamp(plot.left(), plot.left() + chart_w - tsz.x);
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(tx, plot.bottom() - tsz.y), tsz),
                2.0,
                t.tag_bg,
            );
            painter.text(
                Pos2::new(tx + tsz.x / 2.0, plot.bottom() - tsz.y / 2.0),
                Align2::CENTER_CENTER,
                t_txt,
                FontId::monospace(11.0),
                theme::cur().text,
            );
        }
    }

    // 操作提示放右上角：底部是时间轴标签区，放左下会与时间刻度重叠
    painter.text(
        Pos2::new(plot.left() + chart_w - 8.0, plot.top() + 8.0),
        Align2::RIGHT_TOP,
        crate::i18n::tr("滚轮缩放 · 拖动平移 · 双击回最新"),
        FontId::proportional(10.5),
        theme::cur().text_dim.gamma_multiply(0.8),
    );

    // ---------- 左上角图例：交易对 · 周期 + 最新 MA 值 ----------
    let last_gi = end as usize - 1;
    let mut legend_y = plot.top() + 8.0;
    painter.text(
        Pos2::new(plot.left() + 8.0, legend_y),
        Align2::LEFT_TOP,
        format!("{} · {}", app.page.symbol, interval.label()),
        FontId::proportional(11.5),
        theme::cur().text_dim,
    );
    legend_y += 16.0;
    for (k, &period) in MA_PERIODS.iter().enumerate() {
        if !app.page.chart.ma[k] {
            continue;
        }
        let val = sma_at(last_gi, period)
            .map(|v| widgets::fmt_thousands(v, dp as usize))
            .unwrap_or_else(|| "—".into());
        painter.text(
            Pos2::new(plot.left() + 8.0, legend_y),
            Align2::LEFT_TOP,
            format!("MA{period} {val}"),
            FontId::monospace(11.0),
            ma_colors[k],
        );
        legend_y += 14.0;
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
