//! 通用小控件：状态灯、数字输入、Toast 提示、格式化工具

use crate::engine::state::{ConnState, SharedState, Toast, ToastKind};
use crate::ui::theme;
use chrono::{Local, TimeZone};
use eframe::egui;
use egui::{Align2, Color32, FontId, Id, Rect, Stroke};
use rust_decimal::Decimal;

/// 顶栏连接状态项：「名称 + 状态灯」紧邻成组，详细状态（连接中/未连接/
/// 错误原因）收进悬停提示。旧版把「已连接」等文字平铺在名称旁，多项
/// 连读时语义粘连成「已连接 账户 已连接 行情」，且行高被文字撑高
pub fn status_light(ui: &mut egui::Ui, name: &str, st: &ConnState) {
    let (c, st_txt) = match st {
        ConnState::Connected => (theme::cur().up, crate::i18n::tr("已连接").to_string()),
        ConnState::Connecting => (theme::cur().accent, crate::i18n::tr("连接中").to_string()),
        ConnState::Disconnected => (theme::cur().text_dim, crate::i18n::tr("未连接").to_string()),
        ConnState::Failed(e) => (
            theme::cur().err,
            if e.is_empty() {
                crate::i18n::tr("错误").to_string()
            } else {
                format!("{}{e}", crate::i18n::tr("错误: "))
            },
        ),
    };
    status_item(ui, name, c, *st == ConnState::Connected, &st_txt);
}

/// 顶栏布尔状态项（REST 接口）：结构与 status_light 一致，仅状态来自布尔
pub fn status_light_ok(ui: &mut egui::Ui, name: &str, ok: bool, ok_txt: &str, err_txt: &str) {
    status_item(
        ui,
        name,
        if ok {
            theme::cur().up
        } else {
            theme::cur().err
        },
        ok,
        if ok { ok_txt } else { err_txt },
    );
}

fn status_item(ui: &mut egui::Ui, name: &str, c: Color32, glow: bool, hover: &str) {
    // 紧凑成组：名称与灯之间 3pt，组与组之间由外层布局的 item_spacing 决定
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(
            egui::RichText::new(name)
                .size(theme::font_sm())
                .color(theme::cur().text_dim),
        );
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        let p = ui.painter();
        p.circle_filled(rect.center(), 4.5, c);
        // 已连接时加一圈微光晕
        if glow {
            p.circle_stroke(
                rect.center(),
                6.5,
                Stroke::new(1.0_f32, c.gamma_multiply(0.35)),
            );
        }
        resp.on_hover_text(format!("{name} · {hover}"));
    });
}

/// 紧凑文字/图标按钮：自绘 glyph 的方形热区，无 Button 的 18pt 最小高与
/// 内边距——混排不同字号的行里与相邻文字做竖直对齐时，Button 的底部
/// 留白会把 glyph 推得明显偏低。glyph 竖直中心显式锚定到 anchor_y
/// （通常取同行参照文字 rect 的 center().y），None 时落在热区自身中心
pub fn glyph_button(
    ui: &mut egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    anchor_y: Option<f32>,
    hover: &str,
) -> egui::Response {
    let font = FontId::proportional(size);
    let galley_w = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), color)
        .size()
        .x;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(galley_w + 8.0, size + 4.0), egui::Sense::click());
    // 热区在行内的位置由布局决定，但 glyph 画在参照文字的中线上；
    // 窄窗口下 wrapped 行会把热区换到下一行，此时锚点已失效，
    // 落回热区自身中心（±4pt 容差吸收行高差）
    let center_y = match anchor_y {
        Some(y) if (rect.min.y - 4.0..=rect.max.y + 4.0).contains(&y) => y,
        _ => rect.center().y,
    };
    ui.painter().text(
        egui::pos2(rect.center().x, center_y),
        Align2::CENTER_CENTER,
        text,
        font,
        color,
    );
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hover);
    // 自绘控件默认对辅助技术不可见，补上可访问名字
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    resp
}

/// 钳制容器：内容限定在当前可用矩形内布局，横向超出部分按遮挡裁剪——
/// 绘制与点击热区都裁（egui 对 interact_rect 取 clip_rect 交集，挡住绘制
/// 即同时挡住点击），父级只按「可用宽 × 内容自然高」推进，溢出宽度
/// 不回流进父级 min_rect。
///
/// 为何需要：egui 的 allocate_ui / scope / with_layout 把子级 min_rect
/// （含溢出）回报给父级，而 ScrollArea 会沿非滚动轴"跟随内容"扩展。
/// 大字号下某表单行超出下单面板宽时，溢出经 ScrollArea → Frame 层层
/// 放大进面板外框，egui 右面板又按外框右缘锚定 300pt 宽——整块面板被
/// 顶出窗口右缘，表现为面板左缘整体右移、右缘被裁（现货切期货时
/// 有效期多一枚 GTX 胶囊，溢出 46pt 尤为明显）。遮挡是可接受的降级，
/// 面板错位不是。顶栏右侧簇同理：超宽时裁掉状态指示，不压到页签文字。
pub fn clip_zone<R>(
    ui: &mut egui::Ui,
    layout: egui::Layout,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let avail = ui.available_rect_before_wrap();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(avail).layout(layout));
    child.set_clip_rect(child.clip_rect().intersect(avail));
    let ret = add_contents(&mut child);
    // 父级按钳制后的矩形推进；child 自身 min_rect 含溢出，只取其高度
    let rect = egui::Rect::from_min_size(
        avail.min,
        egui::vec2(avail.width().max(0.0), child.min_rect().height().max(0.0)),
    );
    let response = ui.interact(
        rect,
        ui.next_auto_id().with("clip_zone"),
        egui::Sense::hover(),
    );
    ui.advance_cursor_after_rect(rect);
    egui::InnerResponse::new(ret, response)
}

/// 弹性填宽数字输入框：desired_width 取本行剩余可用宽减去 tail（行尾
/// 伴随控件的总宽），输入框总是占满行内空隙、右缘与伴随控件对齐。
/// fixed 120 宽在任意面板宽下右缘参差不齐；flex 后面板宽变化亦成列。
/// 返回 Response 供调用方按需响应 .changed() / .lost_focus()
/// （现有调用方仍每帧重解析输入串，返回值是为接线的场景留的——
/// 此前 show 返回 ()，编辑信号被结构性丢弃，接线也静默失效）
pub fn num_field_flex(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
    h: f32,
    tail: f32,
) -> egui::Response {
    let w = (ui.available_width() - tail).max(56.0);
    egui::TextEdit::singleline(text)
        .id_salt(id)
        .desired_width(w)
        .min_size(egui::vec2(0.0, h))
        .font(egui::TextStyle::Monospace)
        .text_color(theme::cur().text)
        .hint_text(hint)
        .show(ui)
        .response
        .response
}

pub fn parse_dec(s: &str) -> Option<Decimal> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    Decimal::from_str_exact(t).ok()
}

/// Decimal → f64 数值转换（替代 to_string().parse()，避免每帧大量字符串堆分配）
pub fn f64_decimal(d: &Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    d.to_f64().unwrap_or(0.0)
}

/// 展示层千分位分组：绝对值 ≥1000 时按 Decimal 自身小数位分组（不丢失精度语境），
/// 其余保持 Decimal 全精度原文。价格不被四舍五入，仅加千分位分隔。
pub fn fmt_dec_grouped(d: &Decimal) -> String {
    if d.abs() >= Decimal::from(1000) {
        let dp = (d.scale() as usize).min(8);
        fmt_thousands_dec(d, dp)
    } else {
        d.normalize().to_string()
    }
}

/// 按交易对规则精度定长显示：小数位固定 dp，≥1000 附千分位分组。
/// 价格用 quote 资产侧精度（tickSize 小数位）、数量用 base 资产侧精度
/// （stepSize 小数位），同一列表格纵向小数点对齐
pub fn fmt_dec_fixed(d: &Decimal, dp: usize) -> String {
    if d.abs() >= Decimal::from(1000) {
        fmt_thousands_dec(d, dp)
    } else {
        fmt_dec_plain(d, dp)
    }
}

/// 按精度定长显示、不附千分位分组：侧栏等紧凑场景需要纯粹的价格数字，
/// 逗号既占宽度又打断纵向扫读（68,432.15 → 68432.15）
pub fn fmt_dec_plain(d: &Decimal, dp: usize) -> String {
    // 纯 Decimal 路径：round_dp 负责按精度舍入，Display 的 {:.*} 负责末位补零。
    // 全程不经过 f64——f64 只有 15~17 位有效数字，高精度币种（8 位小数）
    // 或币昂服务端返回的双精度残差（如 entryPrice 86997.7000000001）
    // 经 f64 中转后末位会露出脏数字
    // 舍入策略显式取 MidpointAwayFromZero：rust_decimal 的 round_dp 默认是
    // 银行家舍入（half-to-even，0.125 → 0.12），金融展示惯例是四舍五入（→ 0.13）
    format!("{:.*}", dp, round_money(d, dp))
}

/// 金额/价格的展示舍入：half away from zero（四舍五入）
fn round_money(d: &Decimal, dp: usize) -> Decimal {
    d.round_dp_with_strategy(
        dp as u32,
        rust_decimal::RoundingStrategy::MidpointAwayFromZero,
    )
}

/// 千分位分组格式化（用于展示层；坐标轴等本身就是 f64 的场景）
pub fn fmt_thousands(v: f64, dp: usize) -> String {
    group_thousands(&format!("{:.*}", dp, v))
}

/// 千分位分组（Decimal 源）：舍入与定长由 Decimal 完成，仅分组逻辑与
/// `fmt_thousands` 共用
fn fmt_thousands_dec(d: &Decimal, dp: usize) -> String {
    group_thousands(&format!("{:.*}", dp, round_money(d, dp)))
}

/// 已格式化数字串的 3 位分组：整数部分从右起每 3 位插一个逗号
fn group_thousands(s: &str) -> String {
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let neg = int_part.starts_with('-');
    let digits = int_part.trim_start_matches('-');
    let bytes = digits.as_bytes();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(*b as char);
    }
    let sign = if neg { "-" } else { "" };
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// 表格数值单元格：右对齐 + 等宽字体（价格/数量/金额/盈亏列通用）
pub fn num_cell(ui: &mut egui::Ui, txt: String, color: Option<Color32>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let mut rt = egui::RichText::new(txt).monospace().size(theme::font_md());
        if let Some(c) = color {
            rt = rt.color(c);
        }
        ui.label(rt);
    });
}

/// 大数缩写（成交额）：中文 1.2亿 / 34.5万；英文 1.2B / 34.5K
pub fn fmt_compact(v: f64) -> String {
    fmt_compact_lang(v, crate::i18n::lang())
}

/// 按指定语言缩写：参数化版供测试并行使用——测试不得改全局 LANG
/// （i18n LANG 是进程级 AtomicU8，并行测试一个 set_lang 会把其它
/// 读全局的测试带偏，实测偶发 "34.5K" != "3.5万"）
fn fmt_compact_lang(v: f64, lang: u8) -> String {
    let a = v.abs();
    if lang == crate::i18n::EN {
        if a >= 1e9 {
            format!("{:.2}B", v / 1e9)
        } else if a >= 1e6 {
            format!("{:.2}M", v / 1e6)
        } else if a >= 1e3 {
            format!("{:.1}K", v / 1e3)
        } else if a >= 1.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.4}")
        }
    } else if a >= 1e8 {
        format!("{:.2}亿", v / 1e8)
    } else if a >= 1e4 {
        format!("{:.1}万", v / 1e4)
    } else if a >= 1.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.4}")
    }
}

/// 右下角 Toast 提示（未过期的持续显示，过期自动移除）
pub fn draw_toasts(ctx: &egui::Context, state: &SharedState) {
    let toasts: Vec<Toast> = {
        let mut t = state.toasts.lock();
        // 错误类保留更久：下单被拒/断线等提示 5 秒即消失极易错过
        t.retain(|x| {
            let ttl = match x.kind {
                ToastKind::Err => 15.0,
                _ => 5.0,
            };
            x.at.elapsed().as_secs_f64() < ttl
        });
        t.clone()
    };
    if toasts.is_empty() {
        return;
    }
    egui::Area::new(Id::new("toasts_area"))
        .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::RIGHT), |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for (i, t) in toasts.iter().enumerate() {
                    draw_toast(ui, ctx, state, i, t);
                }
            });
        });
}

/// 单条 Toast：投影 + 圆角卡片 + 左侧 kind 色条 + 圆形图标 + TTL 进度条，
/// 入场 250ms 滑入淡入
fn draw_toast(ui: &mut egui::Ui, ctx: &egui::Context, state: &SharedState, i: usize, t: &Toast) {
    let kind_color = match t.kind {
        ToastKind::Ok => theme::cur().up,
        ToastKind::Err => theme::cur().err,
        ToastKind::Warn => theme::cur().warn,
        ToastKind::Info => theme::cur().accent,
    };
    // TTL 与 draw_toasts 的 retain 规则保持一致：错误 15s，其余 5s
    let ttl = match t.kind {
        ToastKind::Err => 15.0,
        _ => 5.0,
    };
    // 入场动画：仅出现后的 250ms 内推进动画值，之后恒为 1（不再每帧计算）
    let age_ms = t.at.elapsed().as_secs_f32() * 1000.0;
    let p = if age_ms < 250.0 {
        ctx.animate_value_with_time(
            Id::new("toast_in").with(i as u64).with(t.msg.as_str()),
            1.0,
            0.25,
        )
        .clamp(0.0, 1.0)
    } else {
        1.0
    };
    let fade =
        |c: Color32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * p) as u8);
    let fg = fade(kind_color);
    let txt = fade(theme::cur().text);
    let bg = fade(theme::cur().panel2);
    let border = fade(theme::cur().border);
    let shadow_color = if theme::is_light() {
        Color32::from_black_alpha((40.0 * p) as u8)
    } else {
        Color32::from_black_alpha((90.0 * p) as u8)
    };
    let frame = egui::Frame::default()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 14,
            spread: 0,
            color: shadow_color,
        })
        .corner_radius(f32::from(theme::R_LG))
        .inner_margin(egui::Margin::symmetric(12, 9));
    let inner = frame.show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            // 圆形图标底 + 内嵌 8px 实心圆点：不用 ✓/✗/! 字形，
            // 规避等宽字体优先后字形缺字回退成豆腐块的风险
            let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            let pr = ui.painter();
            pr.circle_filled(rect.center(), 9.0, fade(kind_color.gamma_multiply(0.2)));
            pr.circle_filled(rect.center(), 4.0, fg);
            ui.label(
                egui::RichText::new(t.msg.as_str())
                    .size(theme::font_md())
                    .color(txt),
            );
        });
    });
    // 左侧 kind 色条（3px）
    let r = inner.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_size(
            egui::Pos2::new(r.left(), r.top() + 4.0),
            egui::vec2(3.0, r.height() - 8.0),
        ),
        2.0,
        fg,
    );
    // TTL 进度条（底部 2px，kind 色，宽度 = 剩余时间占比；两端内缩避开圆角）
    let age = t.at.elapsed().as_secs_f64();
    let remain = ((ttl - age) / ttl).clamp(0.0, 1.0) as f32;
    if remain > 0.0 {
        let bar_w = (r.width() - 16.0).max(0.0) * remain;
        ui.painter().rect_filled(
            Rect::from_min_size(
                egui::Pos2::new(r.left() + 8.0, r.bottom() - 2.0),
                egui::vec2(bar_w, 2.0),
            ),
            1.0,
            fg,
        );
    }
    // 点击即关闭（不等 TTL 自然过期）：按 (内容, 入队时刻) 精确匹配
    let clicked = inner
        .response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(crate::i18n::tr("点击关闭"))
        .clicked();
    if clicked {
        state
            .toasts
            .lock()
            .retain(|x| !(x.msg == t.msg && x.at == t.at));
    }
}

pub fn fmt_time(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%m-%d %H:%M:%S").to_string(),
        None => "—".into(),
    }
}

pub fn fmt_time_short(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%H:%M:%S").to_string(),
        None => "—".into(),
    }
}

/// 空态/加载占位文本
pub fn placeholder(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(theme::font_lg()),
        theme::cur().text_dim,
    );
}

/// 空状态：居中的暗色图标 + 标题 + 副标题（subtitle 为空则不渲染）
pub fn empty_state(ui: &mut egui::Ui, icon: &str, title: &str, subtitle: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(16.0);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(60.0), 26.0),
            egui::Sense::hover(),
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon,
            FontId::proportional(theme::font_hero()),
            theme::cur().text_dim,
        );
        ui.add_space(2.0);
        // 标题用正文色加粗压过副标题：空态也要有信息层级
        ui.label(
            egui::RichText::new(title)
                .size(theme::font_body())
                .strong()
                .color(theme::cur().text),
        );
        if !subtitle.is_empty() {
            ui.label(
                egui::RichText::new(subtitle)
                    .size(theme::font_md())
                    .color(theme::cur().text_dim.gamma_multiply(0.85)),
            );
        }
        ui.add_space(16.0);
    });
}

/// 加载态：spinner + 暗字说明，并预留最小高度——数据到达后不从一行字
/// 跳成整表（盘口/成交/资产等首帧加载共用）
pub fn loading_state(ui: &mut egui::Ui, msg: &str) {
    ui.allocate_ui(egui::vec2(ui.available_width(), 120.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(28.0);
            theme::spinner(ui);
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(msg)
                    .size(theme::font_md())
                    .color(theme::cur().text_dim),
            );
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_grouping() {
        assert_eq!(fmt_thousands(96000.5, 2), "96,000.50");
        assert_eq!(fmt_thousands(1234567.891, 2), "1,234,567.89");
        assert_eq!(fmt_thousands(-1234.5, 1), "-1,234.5");
        assert_eq!(fmt_thousands(42.0, 0), "42");
        assert_eq!(fmt_thousands(0.15, 4), "0.1500");
    }

    #[test]
    fn plain_formatting_no_grouping() {
        let d = rust_decimal::Decimal::from_str_exact("68432.15").unwrap();
        assert_eq!(fmt_dec_plain(&d, 2), "68432.15");
        let big = rust_decimal::Decimal::from_str_exact("1234567.891").unwrap();
        assert_eq!(fmt_dec_plain(&big, 2), "1234567.89");
        let small = rust_decimal::Decimal::from_str_exact("0.1").unwrap();
        assert_eq!(fmt_dec_plain(&small, 4), "0.1000");
    }

    #[test]
    fn decimal_formatters_never_leak_float_residue() {
        // 币昂返回的 entryPrice 原文带双精度尾数（服务端浮点残差），
        // Decimal 全精度保留但在展示层必须按 tick 精度收敛
        let d = rust_decimal::Decimal::from_str_exact("86997.7000000001").unwrap();
        assert_eq!(fmt_dec_plain(&d, 2), "86997.70");
        assert_eq!(fmt_dec_fixed(&d, 2), "86,997.70");
        // 高精度币按其 tick 保留（SHIB 类 8 位小数）：f64 中转会让
        // 第 8 位露出脏数字，纯 Decimal 路径下精确
        let shib = rust_decimal::Decimal::from_str_exact("0.000012345678901234").unwrap();
        assert_eq!(fmt_dec_plain(&shib, 8), "0.00001235");
        // 舍入而非截断
        assert_eq!(
            fmt_dec_plain(&rust_decimal::Decimal::from_str_exact("0.125").unwrap(), 2),
            "0.13"
        );
        // 大整数金额：f64 在 10^16 以上丢失精度，Decimal 不丢
        let huge = rust_decimal::Decimal::from_str_exact("12345678901234567.89").unwrap();
        assert_eq!(fmt_dec_fixed(&huge, 2), "12,345,678,901,234,567.89");
    }

    #[test]
    fn grouped_formatting_keeps_decimal_text() {
        // group_thousands 只分组不改数字：负数符号、零、纯整数均稳定
        assert_eq!(
            fmt_dec_grouped(&rust_decimal::Decimal::from_str_exact("1234567.891").unwrap()),
            "1,234,567.891"
        );
        assert_eq!(
            fmt_dec_grouped(
                &rust_decimal::Decimal::from_str_exact("123456789012345678.1").unwrap()
            ),
            "123,456,789,012,345,678.1"
        );
    }

    #[test]
    fn compact_formatting() {
        // 参数化 ZH：不读全局 LANG（任何测试都不再 set_lang，并行无竞争）
        assert_eq!(fmt_compact_lang(123_456_789.0, crate::i18n::ZH), "1.23亿");
        assert_eq!(fmt_compact_lang(34_500.0, crate::i18n::ZH), "3.5万");
        assert_eq!(fmt_compact_lang(123.0, crate::i18n::ZH), "123");
    }

    #[test]
    fn compact_formatting_english() {
        // 参数化 EN：不改全局（曾因 set_lang 与并行测试竞争导致
        // compact_formatting 偶发读到 EN 而失败）
        assert_eq!(fmt_compact_lang(1_234_567_890.0, crate::i18n::EN), "1.23B");
        assert_eq!(fmt_compact_lang(34_500.0, crate::i18n::EN), "34.5K");
        assert_eq!(fmt_compact_lang(123.0, crate::i18n::EN), "123");
    }

    #[test]
    fn compact_formatting_reads_global_default() {
        // 默认全局为 ZH 且无测试再改动它：fmt_compact 包装层稳走 ZH 分支
        assert_eq!(fmt_compact(34_500.0), "3.5万");
    }
}
