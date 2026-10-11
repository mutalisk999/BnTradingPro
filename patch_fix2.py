# -*- coding: utf-8 -*-
# ---------- 修复轮次 2 ----------
# 1. unlock.rs 常量 → cur() 访问器
p = 'src/ui/unlock.rs'
s = open(p, encoding='utf-8').read()
s = s.replace("use crate::ui::theme::{self, ACCENT, BG, BORDER, CARD, ERR, TEXT, TEXT_DIM, WARN};",
              "use crate::ui::theme;")
for a, b in [("TEXT_DIM", "theme::cur().text_dim"), ("ACCENT", "theme::cur().accent"),
             ("BORDER", "theme::cur().border"), ("CARD", "theme::cur().card"),
             ("BG", "theme::cur().bg"), ("WARN", "theme::cur().warn"),
             ("ERR", "theme::cur().err"), ("TEXT", "theme::cur().text")]:
    s = s.replace(a, b)
# 上面把 theme::cur().text_dim 中的 TEXT 也替换过？TEXT_DIM 先替换为 text_dim，顺序安全。
# 但注意 theme::cur().text 会误伤 "TEXT_DIM" 已替换完的情况——已按先长后短处理。
open(p, 'w', encoding='utf-8').write(s)

# 2. SettingsDraft 补 favorites 字段
p = 'src/app.rs'
s = open(p, encoding='utf-8').read()
old = """    pub lang: String,
    pub theme_mode: String,
}

impl SettingsDraft {
    pub fn from_cfg(cfg: &AppConfig) -> Self {
        Self {"""
new = """    pub lang: String,
    pub theme_mode: String,
    pub favorites_spot: Vec<String>,
    pub favorites_futures: Vec<String>,
}

impl SettingsDraft {
    pub fn from_cfg(cfg: &AppConfig) -> Self {
        Self {"""
assert old in s, "draft fav fields"
s = s.replace(old, new)
old = """            lang: cfg.lang.clone(),
            theme_mode: cfg.theme_mode.clone(),
        }
    }"""
new = """            lang: cfg.lang.clone(),
            theme_mode: cfg.theme_mode.clone(),
            favorites_spot: cfg.favorites_spot.clone(),
            favorites_futures: cfg.favorites_futures.clone(),
        }
    }"""
assert old in s, "draft from_cfg fav"
s = s.replace(old, new)

# 3. 顶栏切换的 move 错误
old = """                if ui.small_button(theme_txt).on_hover_text(crate::i18n::tr("切换主题")).clicked() {
                    let mut cfg = self.state.cfg.read().clone();
                    cfg.theme_mode = if cfg.theme_mode == "light" { "dark".into() } else { "light".into() };
                    cfg.save(&crate::config::data_dir());
                    *self.state.cfg.write() = cfg;
                    theme::set_mode_str(&cfg.theme_mode);
                    theme::apply(ui.ctx());
                    self.applied_theme = cfg.theme_mode.clone();
                }"""
new = """                if ui.small_button(theme_txt).on_hover_text(crate::i18n::tr("切换主题")).clicked() {
                    let mut cfg = self.state.cfg.read().clone();
                    let to_light = cfg.theme_mode == "light";
                    cfg.theme_mode = if to_light { "dark".into() } else { "light".into() };
                    cfg.save(&crate::config::data_dir());
                    theme::set_mode_str(&cfg.theme_mode);
                    theme::apply(ui.ctx());
                    self.applied_theme = cfg.theme_mode.clone();
                    *self.state.cfg.write() = cfg;
                }"""
assert old in s, "theme toggle"
s = s.replace(old, new)

old = """                if ui
                    .small_button(egui::RichText::new(lang_txt).size(12.0))
                    .on_hover_text(crate::i18n::tr("切换语言"))
                    .clicked()
                {
                    let mut cfg = self.state.cfg.read().clone();
                    cfg.lang = if cfg.lang == "en" { "zh".into() } else { "en".into() };
                    cfg.save(&crate::config::data_dir());
                    *self.state.cfg.write() = cfg;
                    crate::i18n::set_lang(if cfg.lang == "en" { crate::i18n::EN } else { crate::i18n::ZH });
                    self.applied_lang = cfg.lang.clone();
                }"""
new = """                if ui
                    .small_button(egui::RichText::new(lang_txt).size(12.0))
                    .on_hover_text(crate::i18n::tr("切换语言"))
                    .clicked()
                {
                    let mut cfg = self.state.cfg.read().clone();
                    let to_en = cfg.lang == "en";
                    cfg.lang = if to_en { "zh".into() } else { "en".into() };
                    cfg.save(&crate::config::data_dir());
                    crate::i18n::set_lang(if cfg.lang == "en" { crate::i18n::EN } else { crate::i18n::ZH });
                    self.applied_lang = cfg.lang.clone();
                    *self.state.cfg.write() = cfg;
                }"""
assert old in s, "lang toggle"
s = s.replace(old, new)
open(p, 'w', encoding='utf-8').write(s)

# 4. settings.rs Phase 修正
p = 'src/ui/settings.rs'
s = open(p, encoding='utf-8').read()
s = s.replace("app.phase = Phase::SetupMaster;", "app.phase = Phase::EnterKeys;")
open(p, 'w', encoding='utf-8').write(s)

# 5. i18n 兜底函数签名
p = 'src/i18n.rs'
s = open(p, encoding='utf-8').read()
s = s.replace("fn return_en_fallback(zh: &str) -> &'static str {",
              "fn return_en_fallback(zh: &'static str) -> &'static str {")
open(p, 'w', encoding='utf-8').write(s)
print("fix2 done")
