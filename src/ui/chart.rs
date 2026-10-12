//! 自绘蜡烛图：缩放/平移、十字光标、最新价与挂单/持仓叠加线

use crate::app::BnApp;
use crate::binance::model::Candle;
use crate::binance::Interval;
use crate::ui::theme;
use crate::ui::widgets;
use chrono::TimeZone;
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

/// 均线周期与配色（MA5/MA10/MA20/MA60）
pub const MA_PERIODS: [usize; 4] = [5, 10, 20, 60];
pub const MA_COLORS: [Color32; 4] = [
    Color32::from_rgb(255, 193, 61),  // 琥珀
    Color32::from_rgb(240, 100, 181), // 粉
    Color32::from_rgb(80, 200, 255),  // 青
    Color32::from_rgb(170, 130, 255), // 紫
];

#[derive(Debug, Clone)]
pub struct ChartState {
    pub bars_visible: f32,
    pub right_off: f32,
    pub ma: [bool; 4],
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            bars_visible: 120.0,
            right_off: 0.0,
            ma: [true, true, true, false],
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
    let dp = state.with_rule(app.market, &app.symbol, |r| {
        r.map(|x| x.price_dp()).unwrap_or(2)
    });

    // 头部信息：24h 统计（合约附标记价/资金费率）
    let (tick, mark) = {
        let md = state.md.lock();
        let t = md
            .get(app.market, &app.symbol)
            .and_then(|m| m.ticker.clone());
        let m = md.get(app.market, &app.symbol).and_then(|m| m.mark.clone());
        (t, m)
    };

    // ---------- 头部工具行：MA 开关 + 24h 统计 ----------
    ui.horizontal(|ui| {
        for (i, &period) in MA_PERIODS.iter().enumerate() {
            let label = egui::RichText::new(format!("MA{period}"))
                .size(11.5)
                .strong();
            let label = if app.chart.ma[i] {
                label.color(MA_COLORS[i])
            } else {
                label.color(theme::cur().text_dim)
            };
            if ui.selectable_label(app.chart.ma[i], label).clicked() {
                app.chart.ma[i] = !app.chart.ma[i];
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(t) = &tick {
                let vol_txt = widgets::fmt_compact(f64_of(&t.quote_volume));
                ui.label(
                    egui::RichText::new(format!(
                        "{} {} · {} {} · {} {}",
                        crate::i18n::tr("24h 高"),
                        widgets::fmt_dec(&t.high_price),
                        crate::i18n::tr("24h 低"),
                        widgets::fmt_dec(&t.low_price),
                        crate::i18n::tr("· 额"),
                        vol_txt
                    ))
                    .size(11.0)
                    .color(theme::cur().text_dim),
                );
            }
            if app.market == crate::binance::Market::UmFutures {
                if let Some(m) = &mark {
                    if let Some(r) = &m.funding_rate {
                        let rf = f64_of(r);
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {rf:+.4}%",
                                crate::i18n::tr("资金费率")
                            ))
                            .size(11.0)
                            .color(theme::cur().accent),
                        );
                    }
                }
            }
        });
    });
    ui.separator();

    // 叠加线：当前挂单（含止损触发价）+ 合约持仓开仓价
    let mut overlays: Vec<Overlay> = Vec::new();
    if state.has_creds() {
        for o in state.orders.lock().open(app.market).iter() {
            if o.symbol == app.symbol {
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
                        price: f64_of(&p),
                        color,
                        label: format!("{}{} {}", crate::i18n::tr("挂单 "), side_txt, fmt_norm(&p)),
                    });
                } else if let Some(s) = o.stop_price.filter(|s| !s.is_zero()) {
                    overlays.push(Overlay {
                        price: f64_of(&s),
                        color,
                        label: format!("{}{} {}", crate::i18n::tr("触发 "), side_txt, fmt_norm(&s)),
                    });
                }
            }
        }
        if app.market == crate::binance::Market::UmFutures {
            for p in state.acct.lock().fut_positions.iter() {
                if p.symbol == app.symbol && p.amt != Decimal::ZERO {
                    overlays.push(Overlay {
                        price: f64_of(&p.entry_price),
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

    // 短暂持锁克隆所需数据后立即释放：md 是全局行情锁（depth/ticker/kline 每条
    // WS 消息都要写），不能在整段图表绘制期间持有，否则拖动/悬停重绘会饿死行情写入
    let (klines, interval) = {
        let md = state.md.lock();
        let Some(md) = md.get(app.market, &app.symbol) else {
            widgets::placeholder(ui, ui.max_rect(), crate::i18n::tr("加载行情中…"));
            return;
        };
        (md.klines.clone(), md.interval)
    };
    if klines.is_empty() {
        widgets::placeholder(
            ui,
            ui.max_rect(),
            crate::i18n::tr("暂无K线数据（等待加载或检查网络/代理）"),
        );
        return;
    }
    draw(ui, app, &klines, interval, dp, &overlays);
}

fn f64_of(d: &Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
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
    let t = theme::cur();

    // ---------- 交互 ----------
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta).y;
        if scroll.abs() > 0.5 {
            let f = (1.0 + scroll * 0.0015).clamp(0.6, 1.6);
            app.chart.bars_visible = (app.chart.bars_visible * f).clamp(20.0, 480.0);
        }
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        let bar_w = plot.width() / app.chart.bars_visible.max(1.0);
        // 上界取 max(0)，K 线不足 10 根时 clamp 不会 panic
        let max_off = (candles.len() as f32 - 10.0).max(0.0);
        app.chart.right_off = (app.chart.right_off + d.x / bar_w).clamp(0.0, max_off);
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
    // 每帧一次把 Decimal 批量转 f64：旧实现在多条绘制通道里反复 to_string().parse()，
    // 480 根可见 K 线时每帧上万次字符串堆分配。SMA 改前缀和，O(1) 查询替代 O(period) 重算。
    let closes: Vec<f64> = candles.iter().map(|c| f64_of(&c.c)).collect();
    let mut prefix = Vec::with_capacity(closes.len() + 1);
    prefix.push(0.0f64);
    for v in &closes {
        prefix.push(prefix.last().unwrap() + v);
    }
    let sma_at = |gi: usize, period: usize| -> Option<f64> {
        if gi + 1 < period {
            return None;
        }
        Some((prefix[gi + 1] - prefix[gi + 1 - period]) / period as f64)
    };
    let vis_ohlc: Vec<[f64; 4]> = visible
        .iter()
        .map(|c| [f64_of(&c.o), f64_of(&c.h), f64_of(&c.l), f64_of(&c.c)])
        .collect();
    let vis_vol: Vec<f64> = visible.iter().map(|c| f64_of(&c.v)).collect();

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
            if !app.chart.ma[k] {
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
    let span = hi - lo;

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
    let mut gp = step as f32;
    while gp <= hi {
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
                fmt_f64(gp as f64, dp as usize),
                grid_font.clone(),
                theme::cur().text_dim,
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
        if !app.chart.ma[k] {
            continue;
        }
        let mut pts: Vec<Pos2> = Vec::new();
        for (local, gi) in (start as usize..end as usize).enumerate() {
            if let Some(v) = sma_at(gi, period) {
                pts.push(Pos2::new(x_of(local), y_of(v as f32)));
            }
        }
        if pts.len() > 1 {
            painter.add(Shape::line(pts, Stroke::new(1.3_f32, MA_COLORS[k])));
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
    let tag = fmt_f64(last_close, dp as usize);
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
                fmt_f64(p_of(hp.y) as f64, dp as usize),
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

    // 底部操作提示
    painter.text(
        Pos2::new(plot.left() + 8.0, plot.bottom() - 6.0),
        Align2::LEFT_BOTTOM,
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
        format!("{} · {}", app.symbol, interval.label()),
        FontId::proportional(11.5),
        theme::cur().text_dim,
    );
    legend_y += 16.0;
    for (k, &period) in MA_PERIODS.iter().enumerate() {
        if !app.chart.ma[k] {
            continue;
        }
        let val = sma_at(last_gi, period)
            .map(|v| fmt_f64(v, dp as usize))
            .unwrap_or_else(|| "—".into());
        painter.text(
            Pos2::new(plot.left() + 8.0, legend_y),
            Align2::LEFT_TOP,
            format!("MA{period} {val}"),
            FontId::monospace(11.0),
            MA_COLORS[k],
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

fn fmt_f64(v: f64, dp: usize) -> String {
    widgets::fmt_thousands(v, dp)
}
