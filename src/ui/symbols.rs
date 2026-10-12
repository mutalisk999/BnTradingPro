//! 左侧交易对列表：搜索 + 自选置顶 + 全量虚拟化列表

use crate::app::BnApp;
use crate::ui::theme;
use eframe::egui;

/// 左侧交易对列表：搜索 + 自选置顶 + 全量虚拟化列表
pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    ui.add(
        egui::TextEdit::singleline(&mut app.page.symbol_filter)
            .id_salt("sidebar_search")
            .hint_text(crate::i18n::tr("搜索交易对…"))
            .text_color(theme::cur().text)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(2.0);

    let filter = app.page.symbol_filter.trim().to_uppercase();
    let favs_all = app.state.favorites(app.market);
    let mut switched: Option<String> = None;
    let mut toggled: Option<String> = None;

    // 自选区（置顶）
    let favs: Vec<String> = favs_all
        .iter()
        .filter(|s| filter.is_empty() || s.contains(&filter))
        .cloned()
        .collect();
    if !favs.is_empty() {
        theme::section(ui, crate::i18n::tr("⭐ 自选"));
        for sym in &favs {
            symbol_row(ui, app, sym, true, &mut switched, &mut toggled);
        }
        ui.add_space(2.0);
    }

    // 规则簿为空说明 exchangeInfo 尚未加载（网络/代理问题），
    // 与"搜索无结果"是两回事，必须区分提示
    let rules_loaded = {
        let rules = app.state.rules.read();
        !rules[app.market as usize].symbols.is_empty()
    };
    if !rules_loaded {
        ui.label(
            egui::RichText::new(crate::i18n::tr("交易规则加载中，请检查网络与代理"))
                .size(12.0)
                .color(theme::cur().text_dim),
        );
        // 自选行的点击不能因早退而丢失
        if let Some(sym) = toggled {
            app.state.toggle_favorite(app.market, &sym);
        }
        if let Some(sym) = switched {
            app.switch_symbol(sym);
        }
        return;
    }

    // 全部交易对：过滤结果按 (市场, 过滤词) 缓存，避免每帧从规则簿克隆
    // ~2600 个符号字符串；favs 排除是少量 contains 检查，逐帧做成本低。
    // 列表用 show_rows 虚拟化——每帧只构造可见行的控件
    // 缓存键带规则版本：ReloadAll 重拉 exchangeInfo 后旧列表必须失效
    let rules_ver = app.state.rules_version.load(std::sync::atomic::Ordering::Relaxed);
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
    let all_syms: Vec<&String> = all_filtered
        .iter()
        .filter(|s| !favs.contains(s))
        .collect();
    theme::section(ui, crate::i18n::tr("全部"));
    // 槽位高度 ≥ 行内控件自然高度（星标按钮约 24px），避免行间点击区重叠
    const SYMBOL_ROW_H: f32 = 24.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, SYMBOL_ROW_H, all_syms.len(), |ui, range| {
            // 每行放进精确槽位：不依赖控件自然高度（声明行距与实际行距
            // 不一致时，内容会随滚动深度漂移、与滚动条错位）
            let x_range = ui.max_rect().x_range();
            let top = ui.max_rect().top();
            let start = range.start;
            for i in range {
                let y0 = top + (i - start) as f32 * SYMBOL_ROW_H;
                let rect = egui::Rect::from_x_y_ranges(x_range, y0..=(y0 + SYMBOL_ROW_H));
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |row_ui| {
                    symbol_row(
                        row_ui,
                        app,
                        all_syms[i],
                        false,
                        &mut switched,
                        &mut toggled,
                    );
                });
            }
        });
    if all_syms.is_empty() && favs.is_empty() {
        ui.label(
            egui::RichText::new(crate::i18n::tr("无匹配交易对"))
                .size(12.0)
                .color(theme::cur().text_dim),
        );
    }

    if let Some(sym) = toggled {
        app.state.toggle_favorite(app.market, &sym);
    }
    if let Some(sym) = switched {
        app.switch_symbol(sym);
    }
}


/// 侧边栏交易对行：名称（点击切换）+ 星标（点击加/取自选）
fn symbol_row(
    ui: &mut egui::Ui,
    app: &BnApp,
    sym: &str,
    is_fav: bool,
    switched: &mut Option<String>,
    toggled: &mut Option<String>,
) {
    let selected = *sym == app.page.symbol;
    // 行悬停高亮：先在控件下层铺底色，再放按钮
    let row_rect =
        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 22.0));
    let hovered = ui.rect_contains_pointer(row_rect);
    if hovered && !selected {
        ui.painter()
            .rect_filled(row_rect, 3.0, theme::cur().row_hover);
    }
    ui.horizontal(|ui| {
        let name_txt = {
            let t = egui::RichText::new(sym).size(12.5).color(if selected {
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
        let name_btn = egui::Button::new(name_txt)
            .fill(if selected {
                theme::cur().card
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::NONE)
            .corner_radius(3)
            .min_size(egui::vec2(ui.available_width() - 26.0, 20.0));
        if ui
            .add(name_btn)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            *switched = Some(sym.to_string());
        }
        let star = if is_fav { "★" } else { "☆" };
        let star_txt = egui::RichText::new(star).size(13.0).color(if is_fav {
            theme::cur().accent
        } else {
            theme::cur().text_dim
        });
        if ui
            .add(
                egui::Button::new(star_txt)
                    .fill(egui::Color32::TRANSPARENT)
                    .corner_radius(3),
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
}

