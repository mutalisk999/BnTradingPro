# -*- coding: utf-8 -*-
# ---------- app.rs 补丁 B：侧边栏内容 + 页头ⓘ属性弹层 + 移除旧弹层 + tr 字符串 ----------
p = 'src/app.rs'
s = open(p, encoding='utf-8').read()

# 1. page_header 里 symbol_picker 调用替换为 ⓘ 属性按钮
old = """    /// 页头：交易对 / 周期 / 最新价
    fn page_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.symbol_picker(ui);
            ui.add_space(2.0);"""
new = """    /// 页头：交易对 / 属性ⓘ / 周期 / 最新价
    fn page_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(self.symbol.clone())
                    .strong()
                    .size(16.5),
            );
            self.symbol_info(ui);
            ui.add_space(4.0);"""
assert old in s, "header head"
s = s.replace(old, new)

# 2. 删除 symbol_picker 方法整体，替换为 symbols_sidebar + symbol_info
start = s.index("    /// 自绘交易对选择弹层：搜索框自动聚焦、点击外部关闭")
end_marker = "    /// 交易页底部子页签：挂单 / 持仓(期货) / 历史 / 成交"
end = s.index(end_marker)
new_methods = '''    /// 左侧边栏：搜索 + 自选置顶 + 全部交易对
    fn symbols_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.symbol_filter)
                .id_salt(egui::Id::new("sidebar_search"))
                .hint_text(crate::i18n::tr("搜索交易对…"))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(2.0);

        let book = self.state.rules_of(self.market);
        let favs = self.state.favorites(self.market);
        let filter = self.symbol_filter.trim().to_uppercase();
        let mut switched: Option<String> = None;
        let mut toggled: Option<String> = None;

        // 自选区（置顶）
        let favs: Vec<String> = favs
            .into_iter()
            .filter(|s| filter.is_empty() || s.contains(&filter))
            .collect();
        if !favs.is_empty() {
            theme::section(ui, &crate::i18n::tr("⭐ 自选").to_string());
            for sym in &favs {
                symbol_row(ui, self, sym, true, &mut switched, &mut toggled);
            }
            ui.add_space(2.0);
        }

        // 全部交易对（过滤掉已展示的自选）
        theme::section(ui, &crate::i18n::tr("全部").to_string());
        let mut count = 0;
        for sym in &book.symbols {
            if count >= 400 {
                ui.label(
                    egui::RichText::new(crate::i18n::tr("（仅显示前 400 个，请用搜索过滤）"))
                        .size(10.5)
                        .color(theme::cur().text_dim),
                );
                break;
            }
            if !filter.is_empty() && !sym.contains(&filter) {
                continue;
            }
            if favs.contains(sym) {
                continue;
            }
            count += 1;
            symbol_row(ui, self, sym, false, &mut switched, &mut toggled);
        }
        if count == 0 && favs.is_empty() {
            ui.label(
                egui::RichText::new(crate::i18n::tr("无匹配交易对"))
                    .size(12.0)
                    .color(theme::cur().text_dim),
            );
        }

        if let Some(sym) = toggled {
            self.state.toggle_favorite(self.market, &sym);
        }
        if let Some(sym) = switched {
            self.switch_symbol(sym);
        }
    }

    /// 交易对属性弹层（ⓘ）
    fn symbol_info(&mut self, ui: &mut egui::Ui) {
        let btn = ui
            .small_button(egui::RichText::new("ⓘ").size(12.0))
            .on_hover_text(crate::i18n::tr("交易对属性"));
        if btn.clicked() {
            self.symbol_info_open = !self.symbol_info_open;
        }
        if !self.symbol_info_open {
            return;
        }
        let Some(r) = self.state.rule_of(self.market, &self.symbol) else {
            self.symbol_info_open = false;
            return;
        };
        let mut popup_rect = egui::Rect::NOTHING;
        let pos = egui::Pos2::new(btn.rect.left(), btn.rect.bottom() + 4.0);
        egui::Area::new(egui::Id::new("symbol_info_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(theme::cur().panel2)
                    .stroke(egui::Stroke::new(1.0_f32, theme::cur().border))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_min_width(300.0);
                        ui.label(
                            egui::RichText::new(crate::i18n::tr("交易对属性"))
                                .strong()
                                .size(13.5),
                        );
                        ui.separator();
                        egui::Grid::new("symbol_info_grid")
                            .num_columns(2)
                            .spacing([14.0, 3.0])
                            .show(ui, |ui| {
                                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                                    ui.label(theme::card_label(k));
                                    ui.label(
                                        egui::RichText::new(v).monospace().size(12.0),
                                    );
                                    ui.end_row();
                                };
                                row(ui, &crate::i18n::tr("状态").to_string(), r.status.clone());
                                row(ui, &crate::i18n::tr("基础资产").to_string(), r.base.clone());
                                row(ui, &crate::i18n::tr("计价资产").to_string(), r.quote.clone());
                                if self.market == crate::binance::Market::UmFutures {
                                    row(ui, &crate::i18n::tr("合约类型").to_string(), r.contract_type.clone());
                                    row(ui, &crate::i18n::tr("保证金资产").to_string(), r.margin_asset.clone());
                                    row(ui, &crate::i18n::tr("标的类型").to_string(), {
                                        let subs = r.underlying_sub_types.join("/");
                                        if r.underlying_type.is_empty() {
                                            subs
                                        } else if subs.is_empty() {
                                            r.underlying_type.clone()
                                        } else {
                                            format!("{} · {}", r.underlying_type, subs)
                                        }
                                    });
                                    row(ui, &crate::i18n::tr("上架时间").to_string(), {
                                        if r.onboard_date > 0 {
                                            chrono::Local
                                                .timestamp_millis_opt(r.onboard_date)
                                                .single()
                                                .map(|t| t.format("%Y-%m-%d").to_string())
                                                .unwrap_or_default()
                                        } else {
                                            "—".into()
                                        }
                                    });
                                }
                                row(ui, &crate::i18n::tr("价格精度").to_string(), r.price_precision.to_string());
                                row(ui, &crate::i18n::tr("数量精度").to_string(), r.qty_precision.to_string());
                                row(ui, &crate::i18n::tr("最小变动").to_string(), r.tick.normalize().to_string());
                                row(ui, &crate::i18n::tr("最小数量").to_string(), r.min_qty.normalize().to_string());
                                if r.max_qty > rust_decimal::Decimal::ZERO {
                                    row(ui, &crate::i18n::tr("最大数量").to_string(), r.max_qty.normalize().to_string());
                                }
                                row(ui, &crate::i18n::tr("最小名义金额").to_string(), r.min_notional.normalize().to_string());
                                if !r.order_types.is_empty() {
                                    row(ui, &crate::i18n::tr("支持订单类型").to_string(), r.order_types.join(", "));
                                }
                                if !r.permissions.is_empty() {
                                    row(ui, &crate::i18n::tr("权限").to_string(), r.permissions.join(", "));
                                }
                            });
                    });
                popup_rect = ui.min_rect();
            });
        // 点击弹层外关闭
        if let Some(click_pos) = ui.ctx().input(|i| {
            if i.pointer.any_click() {
                i.pointer.interact_pos()
            } else {
                None
            }
        }) {
            if !popup_rect.contains(click_pos) && !btn.rect.contains(click_pos) {
                self.symbol_info_open = false;
            }
        }
    }

'''
s = s[:start] + new_methods + s[end:]

# 3. 侧边栏行渲染辅助（追加在文件末尾 status_dot_bare 后）
old = """/// 简化状态灯（布尔）
fn status_dot_bare(ui: &mut egui::Ui, ok: bool) {"""
new = """/// 侧边栏交易对行：名称（点击切换）+ 星标（点击加/取自选）
fn symbol_row(
    ui: &mut egui::Ui,
    app: &BnApp,
    sym: &str,
    is_fav: bool,
    switched: &mut Option<String>,
    toggled: &mut Option<String>,
) {
    let selected = *sym == app.symbol;
    ui.horizontal(|ui| {
        let name_txt = egui::RichText::new(sym)
            .size(12.5)
            .strong(selected)
            .color(if selected {
                theme::cur().accent
            } else {
                theme::cur().text
            });
        let name_btn = egui::Button::new(name_txt)
            .fill(if selected {
                theme::cur().card
            } else {
                Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::NONE)
            .rounding(3.0)
            .min_size(egui::vec2(ui.available_width() - 26.0, 20.0));
        if ui.add(name_btn).clicked() {
            *switched = Some(sym.to_string());
        }
        let star = if is_fav { "★" } else { "☆" };
        let star_txt = egui::RichText::new(star)
            .size(13.0)
            .color(if is_fav {
                theme::cur().accent
            } else {
                theme::cur().text_dim
            });
        if ui.add(egui::Button::new(star_txt).fill(Color32::TRANSPARENT).rounding(3.0))
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

/// 简化状态灯（布尔）
fn status_dot_bare(ui: &mut egui::Ui, ok: bool) {"""
assert old in s, "row helper"
s = s.replace(old, new)

# 4. 顶部状态标签 tr 化
s = s.replace('egui::RichText::new("REST")', 'egui::RichText::new(crate::i18n::tr("REST"))')
s = s.replace('egui::RichText::new("行情")', 'egui::RichText::new(crate::i18n::tr("行情"))')
s = s.replace('egui::RichText::new("账户")', 'egui::RichText::new(crate::i18n::tr("账户"))')
s = s.replace('egui::RichText::new("未配置密钥")', 'egui::RichText::new(crate::i18n::tr("未配置密钥"))')
s = s.replace('egui::RichText::new("获取行情中…")', 'egui::RichText::new(crate::i18n::tr("获取行情中…"))')
s = s.replace('theme::badge(ui, "TESTNET", theme::cur().accent);',
              'theme::badge(ui, "TESTNET", theme::cur().accent);')

# 5. 确认弹窗 tr 化
old = """                    let (word, testnet) = (
                        if c.side == Side::Buy {
                            crate::i18n::tr("买入")
                        } else {
                            crate::i18n::tr("卖出")
                        },
                        self.state.cfg.read().testnet,
                    );
                    ui.label(
                        egui::RichText::new(if testnet {
                            format!("【测试网】{word}确认")
                        } else {
                            format!("【实盘】{word}确认 · 真实资金！")
                        })"""
if old in s:
    new = """                    let (word, testnet) = (
                        if c.side == Side::Buy {
                            crate::i18n::tr("买入")
                        } else {
                            crate::i18n::tr("卖出")
                        },
                        self.state.cfg.read().testnet,
                    );
                    ui.label(
                        egui::RichText::new(if testnet {
                            format!("{}{}{}", crate::i18n::tr("【测试网】"), word, crate::i18n::tr("确认下单").replace(crate::i18n::tr("确认下单"), "确认"))
                        } else {
                            format!("{}{}{}", crate::i18n::tr("【实盘】"), word, crate::i18n::tr("确认 · 真实资金！"))
                        })"""
    s = s.replace(old, new)

# 确认弹窗其余标签
s = s.replace('egui::Window::new(egui::RichText::new("确认下单").strong())',
              'egui::Window::new(egui::RichText::new(crate::i18n::tr("确认下单")).strong())')
s = s.replace('egui::RichText::new("确认提交")', 'egui::RichText::new(crate::i18n::tr("确认提交"))')
s = s.replace('if ui.button("取消").clicked() {', 'if ui.button(crate::i18n::tr("取消")).clicked() {')

# 6. update(): 主题/语言跟随配置
old = """    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match self.phase {"""
new = """    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 主题/语言跟随配置（设置页或顶栏修改后即时生效）
        {
            let cfg = self.state.cfg.read();
            if self.applied_theme != cfg.theme_mode {
                theme::set_mode_str(&cfg.theme_mode);
                theme::apply(ctx);
                self.applied_theme = cfg.theme_mode.clone();
            }
            if self.applied_lang != cfg.lang {
                crate::i18n::set_lang(if cfg.lang == "en" {
                    crate::i18n::EN
                } else {
                    crate::i18n::ZH
                });
                self.applied_lang = cfg.lang.clone();
            }
        }
        match self.phase {"""
assert old in s, "update"
s = s.replace(old, new)

open(p, 'w', encoding='utf-8').write(s)
print("app patch B done")
