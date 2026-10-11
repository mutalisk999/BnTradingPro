//! 自绘蜡烛图：缩放/平移、十字光标、最新价与挂单/持仓叠加线

use crate::app::BnApp;
use crate::binance::model::Candle;
use crate::binance::Interval;
use crate::ui::theme;
use chrono::TimeZone;
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use rust_decimal::Decimal;

#[derive(Debug, Clone)]
pub struct ChartState {
    pub bars_visible: f32,
    pub right_off: f32,
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            bars_visible: 120.0,
            right_off: 0.0,
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
    let dp = state
        .rule_of(app.market, &app.symbol)
        .map(|r| r.price_dp())
        .unwrap_or(2);

    // 叠加线：当前挂单 + 合约持仓开仓价
    let mut overlays: Vec<Overlay> = Vec::new();
    if state.has_creds() {
        for o in state.orders.lock().open(app.market).iter() {
            if o.symbol == app.symbol {
                if let Some(p) = o.price.filter(|p| !p.is_zero()) {
                    let color = if o.side == "BUY" {
                        theme::UP
                    } else {
                        theme::DOWN
                    };
                    let side_txt = if o.side == "BUY" { "买" } else { "卖" };
                    overlays.push(Overlay {
                        price: to_f64(&p),
                        color,
                        label: format!("挂单 {side_txt} {}", fmt_norm(&p)),
                    });
                }
            }
        }
        if app.market == crate::binance::Market::UmFutures {
            for p in state.acct.lock().fut_positions.iter() {
                if p.symbol == app.symbol && p.amt != Decimal::ZERO {
                    overlays.push(Overlay {
                        price: to_f64(&p.entry_price),
                        color: theme::ACCENT,
                        label: format!("持仓开仓 {}", fmt_norm(&p.entry_price)),
                    });
                }
            }
        }
    }

    let md_guard = state.md.lock();
    let Some(md) = md_guard.get(app.market, &app.symbol) else {
        widgets_placeholder(ui, ui.max_rect(), "加载行情中…");
        return;
    };
    let interval = md.interval;
    let klines = &md.klines;
    if klines.is_empty() {
        widgets_placeholder(ui, ui.max_rect(), "暂无K线数据（等待加载或检查网络/代理）");
        return;
    }
    draw(ui, app, klines, interval, dp, &overlays);
}

fn widgets_placeholder(ui: &mut egui::Ui, rect: Rect, text: &str) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(15.0),
        theme::TEXT_DIM,
    );
}

fn to_f64(d: &Decimal) -> f64 {
    d.to_string().parse().unwrap_or(0.0)
}

fn fmt_norm(d: &Decimal) -> String {
    d.normalize().to_string()
}

fn draw(
    ui: &mut egui::Ui,
    app: &mut BnApp,
    candles: &[Candle],
    interval: Interval,
    dp: u32,
    overlays: &[Overlay],
) {
    let rect = ui.max_rect();
    let (resp, painter) = ui.allocate_painter(rect.size(), Sense::click_and_drag());
    let plot = resp.rect.shrink(4.0);

    // ---------- 交互 ----------
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta).y;
        if scroll.abs() > 0.5 {
            let f = (1.0 + scroll * 0.0015).clamp(0.6, 1.6);
            app.chart.bars_visible = (app.chart.bars_visible * f).clamp(20.0, 480.0);
        }
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        let bar_w = plot.width() / app.chart.bars_visible.max(1.0);
        app.chart.right_off =
            (app.chart.right_off + d.x / bar_w).clamp(0.0, candles.len() as f32 - 10.0);
    }
    if resp.double_clicked() {
        app.chart.right_off = 0.0;
    }

    let n = candles.len() as i64;
    let bars = app.chart.bars_visible.min(n as f32).max(10.0);
    let end = (n - app.chart.right_off as i64).clamp(1, n);
    let start = (end - bars as i64).max(0);
    let visible = &candles[start as usize..end as usize];
    if visible.is_empty() {
        return;
    }

    // ---------- 价格范围 ----------
    let pad_ratio = 0.06;
    let price_h = plot.height() * 0.82;
    let vol_h = plot.height() * 0.14;
    let axis_w = 62.0; // 右侧价格轴
    let chart_w = plot.width() - axis_w;

    let mut hi = f64::MIN;
    let mut lo = f64::MAX;
    let mut max_vol = f64::MIN;
    for c in visible {
        let [_, h, l, _] = c.f();
        hi = hi.max(h);
        lo = lo.min(l);
        max_vol = max_vol.max(to_f64(&c.v));
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
    let span = hi - lo;

    let y_of = |p: f32| plot.top() + (hi - p) / span * price_h;
    let p_of = |y: f32| hi - (y - plot.top()) / price_h * span;
    let bar_w = chart_w / bars;
    let x_of = |i: usize| plot.left() + (i as f32 + 0.5) * bar_w;

    // ---------- 网格与价格轴 ----------
    let step = nice_step(span as f64, 6);
    let grid_font = FontId::monospace(11.0);
    let mut gp = step as f32;
    while gp <= hi {
        if gp >= lo {
            let y = y_of(gp);
            painter.line_segment(
                [
                    Pos2::new(plot.left(), y),
                    Pos2::new(plot.left() + chart_w, y),
                ],
                Stroke::new(0.5_f32, Color32::from_rgb(34, 41, 54)),
            );
            painter.text(
                Pos2::new(plot.left() + chart_w + 6.0, y),
                Align2::LEFT_CENTER,
                fmt_f64(gp as f64, dp as usize),
                grid_font.clone(),
                theme::TEXT_DIM,
            );
        }
        gp += step as f32;
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
            theme::TEXT_DIM,
        );
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(0.4_f32, Color32::from_rgb(30, 36, 47)),
        );
    }

    // ---------- 成交量 ----------
    for (idx, c) in visible.iter().enumerate() {
        let v = to_f64(&c.v);
        if max_vol <= 0.0 {
            continue;
        }
        let vh = (v / max_vol * vol_h as f64) as f32;
        let [o, _, _, close] = c.f();
        let col = if close >= o {
            theme::UP.gamma_multiply(0.45)
        } else {
            theme::DOWN.gamma_multiply(0.45)
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

    // ---------- 蜡烛 ----------
    for (idx, c) in visible.iter().enumerate() {
        let [o, h, l, close] = c.f();
        let col = if close >= o { theme::UP } else { theme::DOWN };
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
    let last = visible[visible.len() - 1].f();
    let last_close = last[3];
    let prev_close = if visible.len() > 1 {
        visible[visible.len() - 2].f()[3]
    } else {
        last[0]
    };
    let yl = y_of(last_close as f32).clamp(plot.top(), plot.top() + price_h);
    let col = if last_close >= prev_close {
        theme::UP
    } else {
        theme::DOWN
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
    let tag = fmt_f64(last_close, dp as usize);
    let tag_size = egui::vec2(axis_w - 8.0, 16.0);
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(plot.left() + chart_w + 4.0, yl - tag_size.y / 2.0),
            tag_size,
        ),
        2.0,
        col,
    );
    painter.text(
        Pos2::new(plot.left() + chart_w + 4.0 + tag_size.x / 2.0, yl),
        Align2::CENTER_CENTER,
        tag,
        FontId::monospace(11.0),
        Color32::WHITE,
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
            painter.line_segment(
                [Pos2::new(hp.x, plot.top()), Pos2::new(hp.x, plot.bottom())],
                Stroke::new(0.6_f32, Color32::from_rgb(110, 120, 135)),
            );
            painter.line_segment(
                [
                    Pos2::new(plot.left(), hp.y),
                    Pos2::new(plot.left() + chart_w, hp.y),
                ],
                Stroke::new(0.6_f32, Color32::from_rgb(110, 120, 135)),
            );
            // 悬停价格
            painter.text(
                Pos2::new(plot.left() + chart_w + 6.0, hp.y),
                Align2::LEFT_CENTER,
                fmt_f64(p_of(hp.y) as f64, dp as usize),
                grid_font.clone(),
                theme::TEXT,
            );
            // 定位 K 线
            let idx = (((hp.x - plot.left()) / bar_w) as usize).min(visible.len() - 1);
            let c = &visible[idx];
            let [o, h, l, close] = c.f();
            let chg = if o != 0.0 {
                (close - o) / o * 100.0
            } else {
                0.0
            };
            let lines = format!(
                "{}\n开 {}  高 {}\n低 {}  收 {}\n量 {}   {}{:.2}%",
                time_label_full(c.t),
                fmt_f64(o, dp as usize),
                fmt_f64(h, dp as usize),
                fmt_f64(l, dp as usize),
                fmt_f64(close, dp as usize),
                fmt_norm(&c.v),
                if chg >= 0.0 { "+" } else { "" },
                chg
            );
            let font = FontId::monospace(12.0);
            let galley = painter.layout_no_wrap(lines.clone(), font.clone(), theme::TEXT);
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
                Color32::from_rgb(24, 30, 41).gamma_multiply(0.97),
            );
            painter.rect_stroke(
                Rect::from_min_size(pos, sz),
                3.0,
                Stroke::new(1.0_f32, theme::BORDER),
            );
            painter.text(
                pos + Vec2::new(8.0, 6.0),
                Align2::LEFT_TOP,
                lines,
                font,
                theme::TEXT,
            );
        }
    }

    // 提示角标
    painter.text(
        Pos2::new(plot.left() + 8.0, plot.top() + 8.0),
        Align2::LEFT_TOP,
        format!(
            "{} · {} · 滚轮缩放/拖动平移/双击回最新",
            app.symbol,
            interval.label()
        ),
        FontId::proportional(11.5),
        theme::TEXT_DIM,
    );
}

fn time_label(ms: i64, interval: Interval) -> String {
    let t = match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => t,
        None => return "—".into(),
    };
    match interval {
        Interval::D1 => t.format("%m-%d").to_string(),
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

fn fmt_f64(v: f64, dp: usize) -> String {
    if dp > 0 {
        format!("{:.*}", dp, v)
    } else {
        format!("{:.0}", v)
    }
}
