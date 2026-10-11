# -*- coding: utf-8 -*-
p = 'src/app.rs'
s = open(p, encoding='utf-8').read()

# 1. 结构体字段 + 初始化（先做，前两个断言已通过）
if 'symbol_picker_open: bool' not in s:
    old = """    pub symbol_filter: String,
    pub form: trade::OrderForm,"""
    new = """    pub symbol_filter: String,
    pub symbol_picker_open: bool,
    pub symbol_focus_search: bool,
    pub form: trade::OrderForm,"""
    assert old in s, "fields"
    s = s.replace(old, new)
if 'symbol_picker_open: false' not in s:
    old = """            symbol_filter: String::new(),
            form: trade::OrderForm::default(),"""
    new = """            symbol_filter: String::new(),
            symbol_picker_open: false,
            symbol_focus_search: false,
            form: trade::OrderForm::default(),"""
    assert old in s, "init"
    s = s.replace(old, new)

# 2. 行区间替换 page_header 整个函数 + 新增 symbol_picker
lines = s.split('\n')
start = next(i for i, l in enumerate(lines) if 'fn page_header' in l)
# 回溯到该函数的 doc 注释
doc_start = start
while doc_start > 0 and (lines[doc_start - 1].strip().startswith('///')):
    doc_start -= 1
end = next(i for i, l in enumerate(lines) if '交易页底部子页签' in l)
end -= 1  # 该 doc 注释行前的空行由新内容自带

new_fn = '''    /// 页头：交易对 / 周期 / 最新价
    fn page_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.symbol_picker(ui);
            ui.add_space(2.0);

            // 周期
            egui::ComboBox::from_id_salt("interval_cb")
                .selected_text(egui::RichText::new(self.interval.label()).strong())
                .width(64.0)
                .show_ui(ui, |ui| {
                    for iv in Interval::all() {
                        if ui
                            .selectable_label(self.interval == iv, iv.label())
                            .clicked()
                        {
                            self.switch_interval(iv);
                        }
                    }
                });

            ui.separator();
            // 最新价
            let tick = self
                .state
                .md
                .lock()
                .get(self.market, &self.symbol)
                .and_then(|md| md.ticker.clone());
            if let Some(t) = tick {
                let chg: f64 = t.price_change_percent.to_string().parse().unwrap_or(0.0);
                let c = theme::signed_color(chg);
                let px: f64 = t.last_price.to_string().parse().unwrap_or(0.0);
                ui.label(
                    egui::RichText::new(widgets::fmt_thousands(px, 2))
                        .monospace()
                        .strong()
                        .size(21.0)
                        .color(c),
                );
                ui.label(
                    egui::RichText::new(format!("{chg:+.2}%"))
                        .monospace()
                        .size(13.5)
                        .color(c),
                );
            } else {
                ui.label(egui::RichText::new("获取行情中…").color(theme::TEXT_DIM));
            }
        });
    }

    /// 自绘交易对选择弹层：搜索框自动聚焦、点击外部关闭
    fn symbol_picker(&mut self, ui: &mut egui::Ui) {
        let btn = ui.add(
            egui::Button::new(
                egui::RichText::new(format!("{}  ▾", self.symbol))
                    .strong()
                    .size(15.0),
            )
            .fill(theme::PANEL2)
            .rounding(4.0)
            .min_size(egui::vec2(150.0, 28.0)),
        );
        if btn.clicked() {
            self.symbol_picker_open = !self.symbol_picker_open;
            self.symbol_focus_search = self.symbol_picker_open;
            if self.symbol_picker_open {
                self.symbol_filter.clear();
            }
        }
        if !self.symbol_picker_open {
            return;
        }

        let mut keep_open = true;
        let mut popup_rect = egui::Rect::NOTHING;
        let pos = egui::Pos2::new(btn.rect.left(), btn.rect.bottom() + 4.0);
        egui::Area::new(egui::Id::new("symbol_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::default()
                    .fill(theme::PANEL2)
                    .stroke(Stroke::new(1.0_f32, theme::BORDER))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(280.0, 400.0));
                        // 搜索框：打开时自动聚焦
                        let search = ui.add(
                            egui::TextEdit::singleline(&mut self.symbol_filter)
                                .id_salt(egui::Id::new("symbol_search"))
                                .hint_text("搜索交易对（如 BTC）…")
                                .desired_width(f32::INFINITY),
                        );
                        if self.symbol_focus_search {
                            search.request_focus();
                            self.symbol_focus_search = false;
                        }
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let book = self.state.rules_of(self.market);
                            let f = self.symbol_filter.trim().to_uppercase();
                            let mut count = 0;
                            for s in &book.symbols {
                                if count >= 300 {
                                    break;
                                }
                                if f.is_empty() || s.contains(&f) {
                                    count += 1;
                                    if ui
                                        .selectable_label(*s == self.symbol, s.as_str())
                                        .clicked()
                                    {
                                        self.switch_symbol(s.clone());
                                        keep_open = false;
                                    }
                                }
                            }
                            if count == 0 {
                                ui.label(
                                    egui::RichText::new("无匹配交易对")
                                        .size(12.5)
                                        .color(theme::TEXT_DIM),
                                );
                            }
                        });
                    });
                popup_rect = ui.min_rect();
            });

        // 点击弹层与按钮之外的区域时关闭
        if let Some(click_pos) = ui.ctx().input(|i| {
            if i.pointer.any_click() {
                i.pointer.interact_pos()
            } else {
                None
            }
        }) {
            let outside = !popup_rect.contains(click_pos) && !btn.rect.contains(click_pos);
            if outside {
                keep_open = false;
            }
        }
        self.symbol_picker_open = keep_open;
    }

'''
lines[doc_start:end] = new_fn.split('\n')
s = '\n'.join(lines)

# 3. 顶栏内边距与最小高度
old = """        egui::TopBottomPanel::top("top_bar")
            .frame(
                egui::Frame::default()
                    .fill(theme::PANEL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::symmetric(12.0, 4.0)),
            )
            .show(ctx, |ui| self.topbar(ui));"""
new = """        egui::TopBottomPanel::top("top_bar")
            .frame(
                egui::Frame::default()
                    .fill(theme::PANEL)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.set_min_height(40.0);
                self.topbar(ui);
            });"""
assert old in s, "topbar frame"
s = s.replace(old, new)

# 4. 切换页签时收起弹层
old = """    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.save_active_page();"""
new = """    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.symbol_picker_open = false;
        self.save_active_page();"""
assert old in s, "set_tab"
s = s.replace(old, new)

open(p, 'w', encoding='utf-8').write(s)
print("app done")
