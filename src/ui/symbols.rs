//! 左侧交易对列表：搜索 + 自选置顶 + 全量虚拟化列表（自选/全部同构渲染）

use crate::app::BnApp;
use crate::ui::{theme, widgets};
use eframe::egui;

/// 统一行高：小节标题与交易对行共用同一滚动列表、同一槽位高度。
/// 交易对行两行显示：名称 / 价格+涨跌
const SYMBOL_ROW_H: f32 = 36.0;
/// 星标按钮固定宽度：不依赖字形自然宽度（等宽字体优先下 "☆" 可能回落到
/// 全角字形），确保星标永远落在行槽裁剪区内、热区完整可点击
const STAR_W: f32 = 24.0;

/// 虚拟化列表的行模型：小节标题或交易对行（bool = 是否已自选）
enum Row<'a> {
    Section(&'static str),
    Sym(&'a str, bool),
}

/// 左侧交易对列表：搜索 + 自选置顶 + 全量虚拟化列表
pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
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
                symbol_row(ui, app, sym, true, &mut switched, &mut toggled);
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
        .show_rows(ui, SYMBOL_ROW_H, rows.len(), |ui, range| {
            // 每行放进精确槽位：不依赖控件自然高度（声明行距与实际行距
            // 不一致时，内容会随滚动深度漂移、与滚动条错位）
            let x_range = ui.max_rect().x_range();
            let top = ui.max_rect().top();
            let start = range.start;
            for i in range {
                let y0 = top + (i - start) as f32 * SYMBOL_ROW_H;
                let rect = egui::Rect::from_x_y_ranges(x_range, y0..=(y0 + SYMBOL_ROW_H));
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(rect),
                    |row_ui| match &rows[i] {
                        Row::Section(title) => section_row(row_ui, title),
                        Row::Sym(sym, is_fav) => {
                            symbol_row(row_ui, app, sym, *is_fav, &mut switched, &mut toggled);
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

/// 小节标题行：与交易对行同槽高、垂直居中，风格与行一致
fn section_row(ui: &mut egui::Ui, title: &str) {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(1.0);
        theme::section(ui, title);
    });
}

/// 侧边栏交易对行：两行显示——第一行 名称+星标，第二行 实时价格+涨跌幅
/// （!miniTicker@arr 数据，无行情时第二行留空）
fn symbol_row(
    ui: &mut egui::Ui,
    app: &BnApp,
    sym: &str,
    is_fav: bool,
    switched: &mut Option<String>,
    toggled: &mut Option<String>,
) {
    let selected = *sym == app.page.symbol;
    // 行悬停高亮：先在控件下层铺底色，再放按钮。
    // 悬停用 animate_bool 渐变（进/出淡入淡出）；选中行常亮 + 左侧 3px accent 条
    let row_rect = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), SYMBOL_ROW_H - 2.0),
    );
    let hovered = ui.rect_contains_pointer(row_rect);
    let anim = ui
        .ctx()
        .animate_bool(egui::Id::new("symrow").with(sym), hovered && !selected);
    let painter = ui.painter();
    if selected {
        painter.rect_filled(row_rect, 3.0, theme::cur().row_hover);
        painter.rect_filled(
            egui::Rect::from_min_size(row_rect.min, egui::vec2(3.0, row_rect.height())),
            2.0,
            theme::cur().accent,
        );
    } else if anim > 0.0 {
        painter.rect_filled(row_rect, 3.0, theme::cur().row_hover.gamma_multiply(anim));
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    let tick = app.state.ticker_of(app.market, sym);

    // ---- 第一行：名称 + 星标 ----
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // 行首内边距：选中行的 3px accent 条压在槽位最左，内容须让开
        ui.add_space(6.0);
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
        let name_w = (ui.available_width() - STAR_W - 4.0).max(0.0);
        // Label 而非 Button：Button 会把文本在定宽槽内居中，观察列表名称应左对齐；
        // Label::truncate 按槽宽截断，sense(click) 保留点击切换交易对
        let name_clicked = ui
            .allocate_ui(egui::vec2(name_w, 18.0), |ui| {
                ui.add(
                    egui::Label::new(name_txt)
                        .truncate()
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            })
            .inner;
        if name_clicked {
            *switched = Some(sym.to_string());
        }
        let star = if is_fav { "★" } else { "☆" };
        let star_txt = egui::RichText::new(star)
            .size(theme::FONT_BODY)
            .color(if is_fav {
                theme::cur().accent
            } else {
                theme::cur().text_dim
            });
        if ui
            .add(
                egui::Button::new(star_txt)
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(3)
                    .min_size(egui::vec2(STAR_W, 18.0)),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if is_fav {
                crate::i18n::tr("取消自选")
            } else {
                crate::i18n::tr("加入自选")
            })
            .clicked()
        {
            *toggled = Some(sym.to_string());
        }
    });

    // ---- 第二行：实时价格 + 24h 涨跌幅 ----
    if let Some(t) = tick {
        let chg = widgets::f64_decimal(&t.chg_pct());
        let c = theme::signed_color(chg);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(widgets::fmt_dec_grouped(&t.close))
                    .monospace()
                    .size(theme::FONT_SM)
                    .color(theme::cur().text),
            );
            // RTL 首个控件自然贴行右缘（与第一行星标同列）
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{chg:+.2}%"))
                        .monospace()
                        .size(9.5)
                        .color(c),
                );
            });
        });
    }
}
