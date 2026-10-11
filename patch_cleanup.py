# -*- coding: utf-8 -*-
# ---------- 警告清理 + settings 选择器 ----------
p = 'src/i18n.rs'
s = open(p, encoding='utf-8').read()
# 去重（后出现的重复项删除）
for dup in [
    '        "未配置 API 密钥" => "No API key configured",\n',
    '        "部分成交" => "Partially Filled",\n',
    '        "已成交" => "Filled",\n',
    '        "状态" => "Status",\n',
]:
    # 只删除第二次出现
    first = s.find(dup)
    second = s.find(dup, first + 1)
    if second > 0:
        s = s[:second] + s[second + len(dup):]
# "设置" 重复：后面那处是 "设置" => "Set"（下单面板杠杆按钮）？检查
# 实际重复是 35 行 "设置" => "Settings" 与 81 行另一处 "设置" —— 删除后者
idx = s.find('"设置" => "Settings"')
second = s.find('"设置" =>', idx + 10)
if second > 0:
    line_end = s.find('\n', second)
    s = s[:second] + s[line_end + 1:]
open(p, 'w', encoding='utf-8').write(s)

# state.rs: 移除未使用的 is_favorite
p = 'src/engine/state.rs'
s = open(p, encoding='utf-8').read()
old = """
    /// 是否已加自选
    pub fn is_favorite(&self, market: Market, symbol: &str) -> bool {
        self.favorites(market).iter().any(|x| x == symbol)
    }
"""
s = s.replace(old, "")
open(p, 'w', encoding='utf-8').write(s)

# theme.rs: 移除未使用的 toggle_mode
p = 'src/ui/theme.rs'
s = open(p, encoding='utf-8').read()
old = """
/// 快捷切换主题并应用（顶栏按钮用）
pub fn toggle_mode(ctx: &egui::Context) {
    set_mode(if is_light() { DARK_MODE } else { LIGHT_MODE });
    apply(ctx);
}
"""
s = s.replace(old, "")
open(p, 'w', encoding='utf-8').write(s)

# settings.rs: 语言/主题选择器（请求参数组之前插入）+ tr 化
p = 'src/ui/settings.rs'
s = open(p, encoding='utf-8').read()
old = """    // ---------- 请求参数 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new("请求参数").strong());"""
new = """    // ---------- 外观与语言 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("语言与主题")).strong());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("语言")).strong());
            let mut lang = app.settings.lang.clone();
            egui::ComboBox::from_id_salt("lang_cb")
                .selected_text(if lang == "en" {
                    "English".to_string()
                } else {
                    "中文".to_string()
                })
                .width(90.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut lang, "zh".to_string(), "中文");
                    ui.selectable_value(&mut lang, "en".to_string(), "English");
                });
            app.settings.lang = lang;
            ui.separator();
            ui.label(egui::RichText::new(crate::i18n::tr("主题")).strong());
            let mut mode = app.settings.theme_mode.clone();
            egui::ComboBox::from_id_salt("theme_cb")
                .selected_text(if mode == "light" {
                    crate::i18n::tr("浅色").to_string()
                } else {
                    crate::i18n::tr("深色").to_string()
                })
                .width(90.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut mode, "dark".to_string(), crate::i18n::tr("深色"));
                    ui.selectable_value(&mut mode, "light".to_string(), crate::i18n::tr("浅色"));
                });
            app.settings.theme_mode = mode;
        });
        ui.label(
            egui::RichText::new(crate::i18n::tr("保存后生效；顶栏右侧 ☀/🌙 与 中/EN 按钮可即时切换"))
                .size(11.0)
                .color(theme::cur().text_dim),
        );
    });

    // ---------- 请求参数 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("请求参数")).strong());"""
assert old in s, "settings selectors"
s = s.replace(old, new)
open(p, 'w', encoding='utf-8').write(s)
print("cleanup done")
