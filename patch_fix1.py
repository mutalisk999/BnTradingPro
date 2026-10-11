# -*- coding: utf-8 -*-
# ---------- 修复轮次 ----------
p = 'src/ui/help.rs'
s = open(p, encoding='utf-8').read()
if 'use eframe::egui;' not in s:
    s = s.replace("use crate::i18n::tr;", "use crate::i18n::tr;\nuse eframe::egui;")
open(p, 'w', encoding='utf-8').write(s)

p = 'src/app.rs'
s = open(p, encoding='utf-8').read()

# 顶栏快捷切换里 ctx → ui.ctx()
s = s.replace("""                if ui.small_button(theme_txt).on_hover_text(crate::i18n::tr("切换主题")).clicked() {
                    let mut cfg = self.state.cfg.read().clone();
                    cfg.theme_mode = if cfg.theme_mode == "light" { "dark".into() } else { "light".into() };
                    cfg.save(&crate::config::data_dir());
                    *self.state.cfg.write() = cfg;
                    theme::set_mode_str(&cfg.theme_mode);
                    theme::apply(ctx);
                    self.applied_theme = cfg.theme_mode.clone();
                }""",
"""                if ui.small_button(theme_txt).on_hover_text(crate::i18n::tr("切换主题")).clicked() {
                    let mut cfg = self.state.cfg.read().clone();
                    cfg.theme_mode = if cfg.theme_mode == "light" { "dark".into() } else { "light".into() };
                    cfg.save(&crate::config::data_dir());
                    *self.state.cfg.write() = cfg;
                    theme::set_mode_str(&cfg.theme_mode);
                    theme::apply(ui.ctx());
                    self.applied_theme = cfg.theme_mode.clone();
                }""")

# set_tab 里残留的旧弹层字段
s = s.replace("""    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.symbol_picker_open = false;
        self.save_active_page();""",
"""    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.symbol_info_open = false;
        self.save_active_page();""")

# into_cfg: 补 lang/theme/favorites
old = """    pub fn into_cfg(self) -> AppConfig {
        AppConfig {
            testnet: self.testnet,
            default_symbol: self.default_symbol.trim().to_uppercase(),
            default_interval: self.default_interval,
            recv_window_ms: self.recv_window_ms,
            http_timeout_secs: self.http_timeout_secs,
            log_level: self.log_level,
            override_spot_rest: self.override_spot_rest,
            override_spot_ws: self.override_spot_ws,
            override_fut_rest: self.override_fut_rest,
            override_fut_ws: self.override_fut_ws,
            proxy: crate::config::ProxyConfig {
                enabled: self.proxy_enabled,
                url: self.proxy_url,
            },
        }
    }"""
new = """    pub fn into_cfg(self) -> AppConfig {
        AppConfig {
            testnet: self.testnet,
            default_symbol: self.default_symbol.trim().to_uppercase(),
            default_interval: self.default_interval,
            recv_window_ms: self.recv_window_ms,
            http_timeout_secs: self.http_timeout_secs,
            log_level: self.log_level,
            lang: self.lang,
            theme_mode: self.theme_mode,
            favorites_spot: self.favorites_spot,
            favorites_futures: self.favorites_futures,
            override_spot_rest: self.override_spot_rest,
            override_spot_ws: self.override_spot_ws,
            override_fut_rest: self.override_fut_rest,
            override_fut_ws: self.override_fut_ws,
            proxy: crate::config::ProxyConfig {
                enabled: self.proxy_enabled,
                url: self.proxy_url,
            },
        }
    }"""
assert old in s, "into_cfg"
s = s.replace(old, new)

# SettingsDraft 字段 + from_cfg
old = """    pub proxy_enabled: bool,
    pub proxy_url: String,
}

impl SettingsDraft {
    pub fn from_cfg(cfg: &AppConfig) -> Self {
        Self {"""
new = """    pub proxy_enabled: bool,
    pub proxy_url: String,
    pub lang: String,
    pub theme_mode: String,
}

impl SettingsDraft {
    pub fn from_cfg(cfg: &AppConfig) -> Self {
        Self {"""
assert old in s, "draft fields"
s = s.replace(old, new)

old = """            proxy_enabled: cfg.proxy.enabled,
            proxy_url: cfg.proxy.url.clone(),
        }
    }"""
new = """            proxy_enabled: cfg.proxy.enabled,
            proxy_url: cfg.proxy.url.clone(),
            lang: cfg.lang.clone(),
            theme_mode: cfg.theme_mode.clone(),
        }
    }"""
assert old in s, "draft from_cfg"
s = s.replace(old, new)

# symbol_row: RichText::strong(bool) → 条件调用
old = """    let selected = *sym == app.symbol;
    ui.horizontal(|ui| {
        let name_txt = egui::RichText::new(sym)
            .size(12.5)
            .strong(selected)
            .color(if selected {
                theme::cur().accent
            } else {
                theme::cur().text
            });"""
new = """    let selected = *sym == app.symbol;
    ui.horizontal(|ui| {
        let name_txt = {
            let t = egui::RichText::new(sym)
                .size(12.5)
                .color(if selected {
                    theme::cur().accent
                } else {
                    theme::cur().text
                });
            if selected { t.strong() } else { t }
        };"""
assert old in s, "row strong"
s = s.replace(old, new)

open(p, 'w', encoding='utf-8').write(s)
print("fix round done")
