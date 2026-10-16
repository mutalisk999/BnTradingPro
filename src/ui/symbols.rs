//! 左侧交易对列表：搜索 + 自选置顶 + 全量虚拟化列表（自选/全部同构渲染）

use crate::app::BnApp;
use crate::ui::{theme, widgets};
use eframe::egui;

/// 星标按钮固定宽度：不依赖字形自然宽度（等宽字体优先下 "☆" 可能回落到
/// 全角字形），确保星标永远落在行槽裁剪区内、热区完整可点击
const STAR_W: f32 = 24.0;
/// 小节标题（theme::section）固定占位高度
const SECTION_H: f32 = 22.0;
/// 卡片右缘内边距：第二行涨跌幅靠最右，与左缘 6pt 缩进对称，
/// 避免数字完全贴卡片右缘
const CARD_RIGHT_PAD: f32 = 6.0;
/// 行内第一行（名称）与第二行（价格/涨跌）之间的行隙
const LINE_GAP: f32 = 2.0;
/// 卡片左缘到名称文字的缩进：行首内边距 + 星标槽宽 + 星标/名称间距。
/// 第二行价格按此缩进，与名称文字严格左对齐
const NAME_X: f32 = 6.0 + STAR_W + 4.0;
/// 相邻两个交易对行之间的留白：悬停/选中色带不覆盖这部分，
/// 视觉上每行各自成为独立卡片、不再「粘」在一起
const ROW_GAP: f32 = 7.0;
/// 卡片上下内边距：文字行高之外再留的呼吸空间。没有这层内边距时
/// 第二行价格/涨跌幅的字形下缘紧贴甚至越出卡片色带边缘
const CARD_PAD: f32 = 4.0;

/// 行槽真实尺度：egui 的文本行高不是字号本身（≈字号 ×1.3~1.5，含字距行隙），
/// 早期用固定 34pt 槽位、星标按 FONT_BODY 自然撑高，第二行价格被挤出槽位、
/// 压到下一行的名称上——视觉上星标错位、价格贴着下个交易对。所有高度改为
/// 按当前字体度量推导，pitch = 第一行 + 行隙 + 第二行 + 行间留白
struct RowMetrics {
    /// 第一行（名称 + 星标）槽高：ui.horizontal 的行槽至少 interact_size.y 高，
    /// 文字行高不足时按行槽取高
    l1: f32,
    /// 整行节距：show_rows 的行高须与行内实际排版严格一致，否则视口错位。
    /// 内容（背景色带）占 band = pitch - ROW_GAP，底部 ROW_GAP 留白作行间距
    pitch: f32,
    /// 单行悬停/选中色带高度
    band: f32,
}

/// 行槽最小高：`ui.horizontal` 分配的子 Ui 高度恒为 `interact_size.y`
/// （默认 18pt），行内文字在槽内垂直居中——文字行高（约 12~15pt）不够时
/// **槽位仍然是 18pt**。band 若按文字行高算，第二行价格/涨跌的槽底就会
/// 越出卡片下缘（实测越出 4.7pt：band=37.3 而两行实际占 46pt）。
/// 故 band 按 slot = max(文字行高, interact_size.y) 推导
fn row_metrics(ui: &egui::Ui) -> RowMetrics {
    let slot_h = ui.spacing().interact_size.y;
    let l1 = ui
        .ctx()
        .fonts_mut(|f| f.row_height(&egui::FontId::proportional(theme::FONT_MD)));
    let l2 = ui
        .ctx()
        .fonts_mut(|f| f.row_height(&egui::FontId::monospace(theme::FONT_SM)));
    let l1 = l1.max(slot_h);
    let l2 = l2.max(slot_h);
    let band = l1 + LINE_GAP + l2 + 2.0 * CARD_PAD;
    let mut pitch = band + ROW_GAP;
    // 小节标题固定 22pt 占位：行槽不能比它矮，否则标题溢出自己的槽位
    if pitch < SECTION_H {
        pitch = SECTION_H;
    }
    RowMetrics {
        l1,
        pitch,
        band: (pitch - ROW_GAP).max(band),
    }
}

/// 虚拟化列表的行模型：小节标题或交易对行（bool = 是否已自选）
enum Row<'a> {
    Section(&'static str),
    Sym(&'a str, bool),
}

/// 单行的渲染上下文：把价格精度与点击回传的出参打包成一个入参，
/// 避免 symbol_row 参数超过 clippy 的 too_many_arguments 阈值
struct RowAct<'a> {
    /// 价格显示精度（tick 小数位）
    dp: usize,
    switched: &'a mut Option<String>,
    toggled: &'a mut Option<String>,
}

/// 左侧交易对列表：搜索 + 自选置顶 + 全量虚拟化列表
pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    // 字体度量在帧内是稳定的（除非用户改缩放，缩放后本帧自然重算）
    let m = row_metrics(ui);

    // 实体滚动条（非悬浮）：悬浮条不占布局宽度却会把子控件的可用区撑宽约一个
    // 条宽，行尾的星标因此被推出面板裁剪区（只露出半颗），还压住面板拖拽把手；
    // 实体条真实占位，星标永远完整地停在内容区最右侧
    ui.style_mut().spacing.scroll.floating = false;
    // 滚动条与面板内缘留出间距：轨道贴边会盖住面板拖拽把手，把手拖不动
    ui.style_mut().spacing.scroll.bar_outer_margin = 6.0;
    // 搜索框 + 非空时的 × 清除按钮（内联，避免占一行）
    let has_filter = !app.page.symbol_filter.is_empty();
    ui.horizontal(|ui| {
        let mut search = egui::TextEdit::singleline(&mut app.page.symbol_filter)
            .id_salt("sidebar_search")
            .hint_text(crate::i18n::tr("搜索交易对…"))
            .text_color(theme::cur().text)
            // 与全站控件高一致（26）：默认 interact_size(~18) 偏矮，且 × 按钮更高会错位
            .min_size(egui::vec2(0.0, theme::CONTROL_H));
        search = if has_filter {
            // 预留 × 按钮宽度 + 间距余量，避免文本框/按钮贴到面板边被裁
            search.desired_width(ui.available_width() - 34.0)
        } else {
            search.desired_width(f32::INFINITY)
        };
        ui.add(search);
        if has_filter {
            let clear = egui::Button::new("×")
                .fill(egui::Color32::TRANSPARENT)
                .min_size(egui::vec2(0.0, theme::CONTROL_H));
            if ui
                .add(clear)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(crate::i18n::tr("清空搜索"))
                .clicked()
            {
                app.page.symbol_filter.clear();
            }
        }
    });
    ui.add_space(2.0);

    let filter = app.page.symbol_filter.trim().to_uppercase();
    let favs: Vec<String> = app
        .state
        .favorites(app.market)
        .into_iter()
        .filter(|s| filter.is_empty() || s.contains(&filter))
        .collect();

    let mut switched: Option<String> = None;
    let mut toggled: Option<String> = None;

    // 规则簿为空说明 exchangeInfo 尚未加载（网络/代理问题），与"搜索无结果"
    // 是两回事。自选列表来自本地配置可照常显示，全部区给出加载提示
    let rules_loaded = {
        let rules = app.state.rules.read();
        !rules[app.market as usize].symbols.is_empty()
    };
    if !rules_loaded {
        if !favs.is_empty() {
            theme::section(ui, crate::i18n::tr("自选"));
            for sym in &favs {
                // 规则书未加载时无 tick 可依，回退 2 位（USDT 计价对的常见精度）
                let mut act = RowAct {
                    dp: 2,
                    switched: &mut switched,
                    toggled: &mut toggled,
                };
                symbol_row(ui, app, sym, true, &m, &mut act);
            }
            ui.add_space(2.0);
        }
        ui.label(
            egui::RichText::new(crate::i18n::tr("交易规则加载中，请检查网络与代理"))
                .size(theme::FONT_MD)
                .color(theme::cur().text_dim),
        );
        apply_actions(app, switched, toggled);
        return;
    }

    // 全部交易对：过滤结果按 (市场, 过滤词) 缓存，避免每帧从规则簿克隆
    // ~2600 个符号字符串；行数据每帧只构造引用视图（favs 排除是少量
    // contains 检查，逐帧做成本低）。列表面板自身的 show_rows 虚拟化——
    // 每帧只构造可见行的控件。缓存键带规则版本：ReloadAll 重拉
    // exchangeInfo 后旧列表必须失效
    let rules_ver = app
        .state
        .rules_version
        .load(std::sync::atomic::Ordering::Relaxed);
    let cached = app
        .symbols_cache
        .as_ref()
        .filter(|(m, f, v, _)| *m == app.market && *f == filter && *v == rules_ver);
    let all_filtered = match cached {
        Some((_, _, _, list)) => std::sync::Arc::clone(list),
        None => {
            let list: std::sync::Arc<Vec<String>> = std::sync::Arc::new({
                let rules = app.state.rules.read();
                let book = &rules[app.market as usize];
                book.symbols
                    .iter()
                    .filter(|s| filter.is_empty() || s.contains(&filter))
                    .cloned()
                    .collect()
            });
            app.symbols_cache = Some((
                app.market,
                filter.clone(),
                rules_ver,
                std::sync::Arc::clone(&list),
            ));
            list
        }
    };
    let all_syms: Vec<&String> = all_filtered.iter().filter(|s| !favs.contains(s)).collect();

    if favs.is_empty() && all_syms.is_empty() {
        // 空态放在 ScrollArea 之前：auto_shrink(false) 的滚动区会占满剩余空间，
        // 放在其后面的空态提示永远在视口外
        widgets::empty_state(
            ui,
            "◎",
            crate::i18n::tr("无匹配交易对"),
            crate::i18n::tr("尝试其他关键词"),
        );
        return;
    }

    // 统一行模型：自选置顶 + 全部进同一个滚动列表，同一行高、同一行渲染，
    // 两个分区风格完全一致（此前自选区直接渲染、全部区走行槽虚拟化，
    // 行距与悬停热区都不一致）
    let mut rows: Vec<Row> = Vec::with_capacity(favs.len() + all_syms.len() + 2);
    if !favs.is_empty() {
        rows.push(Row::Section("自选"));
        for s in &favs {
            rows.push(Row::Sym(s, true));
        }
    }
    if !all_syms.is_empty() {
        rows.push(Row::Section("全部"));
        for s in &all_syms {
            rows.push(Row::Sym(s, false));
        }
    }

    // show_rows 按「行高 + item_spacing.y」节距推算可见行范围与内容总高，
    // 下方画行用的却是纯 SYMBOL_ROW_H 节距：间距不归零时两者错位，
    // 视口底部会永远差几行不渲染（列表未到底就出现空白）
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, m.pitch, rows.len(), |ui, range| {
            // 每行放进精确槽位：不依赖控件自然高度（声明行距与实际行距
            // 不一致时，内容会随滚动深度漂移、与滚动条错位）
            let x_range = ui.max_rect().x_range();
            let top = ui.max_rect().top();
            let start = range.start;
            for i in range {
                let y0 = top + (i - start) as f32 * m.pitch;
                let rect = egui::Rect::from_x_y_ranges(x_range, y0..=(y0 + m.pitch));
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(rect),
                    |row_ui| match &rows[i] {
                        Row::Section(title) => section_row(row_ui, title, &m),
                        Row::Sym(sym, is_fav) => {
                            // 价格精度取该交易对 tick 的小数位（= quote 侧报价精度，
                            // 与盘口/交易页同源，USDT 计价对显示 2 位、
                            // SHIB 这类低价币保留其 tick 全部位数不被压成 0.00）。
                            // 只对可见行逐个查规则书，全量 2600 个符号每帧查会
                            // 无谓地抢锁
                            let dp = app
                                .state
                                .with_rule(app.market, sym, |r| {
                                    r.map(|x| x.price_dp() as usize).unwrap_or(2)
                                });
                            let mut act = RowAct {
                                dp,
                                switched: &mut switched,
                                toggled: &mut toggled,
                            };
                            symbol_row(row_ui, app, sym, *is_fav, &m, &mut act);
                        }
                    },
                );
            }
        });
    apply_actions(app, switched, toggled);
}

/// 统一处理行内点击结果：加/取自选、切换交易对
fn apply_actions(app: &mut BnApp, switched: Option<String>, toggled: Option<String>) {
    if let Some(sym) = toggled {
        app.state.toggle_favorite(app.market, &sym);
    }
    if let Some(sym) = switched {
        app.switch_symbol(sym);
    }
}

/// 小节标题行：与交易对行同槽高、垂直居中，整行淡色带与下方列表分隔
fn section_row(ui: &mut egui::Ui, title: &str, m: &RowMetrics) {
    let band = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), m.pitch - 2.0),
    );
    // 淡色带 0.35 alpha：比悬停行更弱，只作视觉分隔不作交互暗示
    ui.painter()
        .rect_filled(band, 3.0, theme::cur().row_hover.gamma_multiply(0.35));
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(7.0);
        theme::section(ui, title);
    });
}

/// 侧边栏交易对行：整行是一张卡片——星标在卡片最前，名称紧随其后，
/// 第二行（实时价格 + 24h 涨跌幅，无行情时留空占位）缩进与名称对齐。
/// 点击卡片任意位置即切换交易对；星标点击单独拦截为加/取自选
fn symbol_row(
    ui: &mut egui::Ui,
    app: &BnApp,
    sym: &str,
    is_fav: bool,
    m: &RowMetrics,
    act: &mut RowAct,
) {
    let selected = *sym == app.page.symbol;
    // 卡片始终铺一层 card 底色（与面板底色拉开层级），高度 = 行内容高
    // （名称行 + 行隙 + 价格行），底部 ROW_GAP 留白不动，相邻卡片之间
    // 由此出现呼吸间隙
    let row_rect = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), m.band),
    );
    let painter = ui.painter();
    if selected {
        painter.rect_filled(row_rect, 3.0, theme::cur().row_hover);
        painter.rect_filled(
            egui::Rect::from_min_size(row_rect.min, egui::vec2(3.0, row_rect.height())),
            2.0,
            theme::cur().accent,
        );
    } else {
        painter.rect_filled(row_rect, 3.0, theme::cur().card);
    }
    // 悬停渐变叠在卡片底色之上（进/出淡入淡出），选中行不重复叠加
    let hovered = ui.rect_contains_pointer(row_rect);
    let anim = ui
        .ctx()
        .animate_bool(egui::Id::new("symrow").with(sym), hovered && !selected);
    if anim > 0.0 {
        painter.rect_filled(row_rect, 3.0, theme::cur().row_hover.gamma_multiply(anim));
    }
    // 整卡热区：interact 不推进光标，仅注册点击/悬停。星标在其后注册，
    // 层级更靠上、优先命中；二者点击互斥分发
    let card_resp = ui
        .interact(
            row_rect,
            egui::Id::new("symcard").with(sym),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(sym);
    ui.spacing_mut().item_spacing.y = LINE_GAP;
    let tick = app.state.ticker_of(app.market, sym);

    // 卡片顶部内边距：星标/名称行整体下移，与色带上缘拉开距离；
    // 底部由 band 里的同额余量兜住，价格行不再贴边
    ui.add_space(CARD_PAD);

    // ---- 第一行：星标 + 名称 ----
    // 星标热区固定宽高（STAR_W × m.l1），字形居中手绘：宽度不随
    // ★/☆ 字形回落变化，所有行的星标严格同一 X、同一 Y
    let star_clicked = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // 行首内边距：选中行的 3px accent 条压在卡片最左，内容须让开
            ui.add_space(6.0);
            let (star_rect, star_resp) = ui
                .allocate_exact_size(egui::vec2(STAR_W, m.l1), egui::Sense::click());
            if star_resp.hovered() {
                ui.painter().rect_filled(
                    star_rect.shrink(2.0),
                    3.0,
                    theme::cur().row_hover.gamma_multiply(0.6),
                );
            }
            ui.painter().text(
                star_rect.center(),
                egui::Align2::CENTER_CENTER,
                if is_fav { "★" } else { "☆" },
                egui::FontId::proportional(theme::FONT_MD),
                if is_fav {
                    theme::cur().accent
                } else {
                    theme::cur().text_dim
                },
            );
            let star_resp = star_resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(if is_fav {
                    crate::i18n::tr("取消自选")
                } else {
                    crate::i18n::tr("加入自选")
                });
            let star_clicked = star_resp.clicked();
            // 名称：纯展示（截断防长名溢出），点击切换由整卡热区负责
            let name_txt = {
                let t = egui::RichText::new(sym)
                    .size(theme::FONT_MD)
                    .color(if selected {
                        theme::cur().accent
                    } else {
                        theme::cur().text
                    });
                if selected {
                    t.strong()
                } else {
                    t
                }
            };
            ui.add(egui::Label::new(name_txt).truncate());
            star_clicked
        })
        .inner;

    // ---- 第二行：实时价格 + 24h 涨跌幅 ----
    if let Some(t) = tick {
        let chg = widgets::f64_decimal(&t.chg_pct());
        // 价格与涨跌幅同色：涨跌方向在一行内一眼可辨（白价格 + 彩涨跌时
        // 视线要在两段颜色间往返对照）
        let c = theme::signed_color(chg);
        ui.horizontal(|ui| {
            // 与第一行名称文字左对齐（星标槽之后）
            ui.add_space(NAME_X);
            // 不附千分位分组：侧栏紧凑场景需要纯粹的价格数字，逗号既占
            // 宽度又打断纵向扫读（68,432.15 → 68432.15）
            ui.label(
                egui::RichText::new(widgets::fmt_dec_plain(&t.close, act.dp))
                    .monospace()
                    .size(theme::FONT_SM)
                    .color(c),
            );
            // RTL 首个控件先让出右缘内边距，涨跌幅不再贴卡片右边
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(CARD_RIGHT_PAD);
                ui.label(
                    egui::RichText::new(format!("{chg:+.2}%"))
                        .monospace()
                        // 与同行价格统一 FONT_SM（此前硬编码 9.5 脱离字号阶梯）
                        .size(theme::FONT_SM)
                        .color(c),
                );
            });
        });
    } else {
        // 无行情时占住第二行槽位，避免有/无数据的行高不一致造成列表跳动
        ui.horizontal(|ui| {
            ui.add_space(NAME_X);
            ui.label(
                egui::RichText::new("—")
                    .monospace()
                    .size(theme::FONT_SM)
                    .color(theme::cur().text_dim),
            );
        });
    }

    // 点击分发：星标区域命中时只加/取自选，卡片其余区域才切换交易对
    // （星标热区注册更晚、层级在卡片之上，两处 clicked 互斥兜底）
    if star_clicked {
        *act.toggled = Some(sym.to_string());
    } else if card_resp.clicked() {
        *act.switched = Some(sym.to_string());
    }
}
