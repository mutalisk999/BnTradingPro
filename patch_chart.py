# -*- coding: utf-8 -*-
p = 'src/ui/chart.rs'
s = open(p, encoding='utf-8').read()

# 1. 价格范围计算后，自适应价格轴宽度（根据最大标签字符数估算）
old = """    let pad_ratio = 0.06;
    let price_h = plot.height() * 0.82;
    let vol_h = plot.height() * 0.14;
    let axis_w = 62.0; // 右侧价格轴
    let chart_w = plot.width() - axis_w;

    let mut hi = f64::MIN;"""
new = """    let pad_ratio = 0.06;
    let price_h = plot.height() * 0.82;
    let vol_h = plot.height() * 0.14;

    let mut hi = f64::MIN;"""
assert old in s, "axis head"
s = s.replace(old, new)

old = """    let span = (hi - lo).max(0.0001);
    let hi = (hi + span * pad_ratio) as f32;
    let lo = (lo - span * pad_ratio) as f32;
    let span = hi - lo;

    let y_of = |p: f32| plot.top() + (hi - p) / span * price_h;"""
new = """    let span = (hi - lo).max(0.0001);
    let hi = (hi + span * pad_ratio) as f32;
    let lo = (lo - span * pad_ratio) as f32;
    let span = hi - lo;

    // 右侧价格轴宽度自适应数字位数（千分位），避免大数值被截断
    let axis_w = ((widgets::fmt_thousands(hi as f64, dp as usize).chars().count() as f32)
        * 8.2
        + 16.0)
        .clamp(58.0, 120.0);
    let chart_w = plot.width() - axis_w;

    let y_of = |p: f32| plot.top() + (hi - p) / span * price_h;"""
assert old in s, "axis width"
s = s.replace(old, new)

# 2. 所有价格展示统一千分位
s = s.replace("fn fmt_f64(v: f64, dp: usize) -> String {\n    if dp > 0 {\n        format!(\"{:.*}\", dp, v)\n    } else {\n        format!(\"{:.0}\", v)\n    }\n}",
"""fn fmt_f64(v: f64, dp: usize) -> String {
    widgets::fmt_thousands(v, dp)
}""")

# 3. 网格线更柔和
s = s.replace("Stroke::new(0.5_f32, Color32::from_rgb(34, 41, 54)),",
              "Stroke::new(0.5_f32, Color32::from_rgb(28, 34, 44)),")
s = s.replace("Stroke::new(0.4_f32, Color32::from_rgb(30, 36, 47)),",
              "Stroke::new(0.4_f32, Color32::from_rgb(26, 32, 42)),")

# 4. 最新价标签圆角
old = """    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(plot.left() + chart_w + 4.0, yl - tag_size.y / 2.0),
            tag_size,
        ),
        2.0,
        col,
    );"""
new = """    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(plot.left() + chart_w + 4.0, yl - tag_size.y / 2.0),
            tag_size,
        ),
        3.0,
        col,
    );"""
assert old in s, "tag"
s = s.replace(old, new)

# 5. 十字光标改虚线
old = """    if let Some(hp) = resp.hover_pos() {
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
            );"""
new = """    if let Some(hp) = resp.hover_pos() {
        if plot.contains(hp) && hp.x < plot.left() + chart_w {
            let cross = Stroke::new(0.7_f32, Color32::from_rgb(120, 130, 148));
            painter.extend(Shape::dashed_line(
                &[
                    Pos2::new(hp.x, plot.top()),
                    Pos2::new(hp.x, plot.bottom()),
                ],
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
            ));"""
assert old in s, "crosshair"
s = s.replace(old, new)

# 6. tooltip 底框圆角 3 → 保持；MA 线宽 1.3
s = s.replace("Stroke::new(1.2_f32, MA_COLORS[k])", "Stroke::new(1.3_f32, MA_COLORS[k])")

open(p, 'w', encoding='utf-8').write(s)
print("chart done")
