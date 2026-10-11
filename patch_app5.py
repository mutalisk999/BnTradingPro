# -*- coding: utf-8 -*-
# ---------- app.rs 大补丁：Help页签 / 侧边栏 / ⓘ属性 / 顶栏切换 / 语言主题应用 / 登录流程合并 ----------
p = 'src/app.rs'
s = open(p, encoding='utf-8').read()

# 1. Phase 移除 SetupMaster
old = """#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    SetupMaster,
    Unlock,
    EnterKeys,
    Main,
}"""
new = """#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 已有凭据库，等待解锁
    Unlock,
    /// 配置 API 密钥（首次/重置/更换；与主密码一体设置）
    EnterKeys,
    Main,
}"""
assert old in s, "phase"
s = s.replace(old, new)

# 2. Tab 加 Help
old = """#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Spot,
    Futures,
    Portfolio,
    Settings,
}

impl Tab {
    pub fn label(&self) -> &'static str {
        match self {
            Tab::Spot => "现货交易",
            Tab::Futures => "期货交易",
            Tab::Portfolio => "Portfolio",
            Tab::Settings => "设置",
        }
    }"""
new = """#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Spot,
    Futures,
    Portfolio,
    Settings,
    Help,
}

impl Tab {
    pub fn label(&self) -> &'static str {
        match self {
            Tab::Spot => crate::i18n::tr("现货交易"),
            Tab::Futures => crate::i18n::tr("期货交易"),
            Tab::Portfolio => crate::i18n::tr("Portfolio"),
            Tab::Settings => crate::i18n::tr("设置"),
            Tab::Help => crate::i18n::tr("使用说明"),
        }
    }"""
assert old in s, "tab"
s = s.replace(old, new)

# 3. BnApp 字段：applied 主题/语言 + 属性弹层 + 移除旧弹层字段
old = """    pub symbol_filter: String,
    pub symbol_picker_open: bool,
    pub symbol_focus_search: bool,
    pub form: trade::OrderForm,"""
new = """    pub symbol_filter: String,
    pub symbol_info_open: bool,
    pub form: trade::OrderForm,"""
assert old in s, "fields"
s = s.replace(old, new)

old = """    pub tab: Tab,"""
new = """    pub tab: Tab,
    /// 已应用的主题/语言（用于配置变更时重新应用）
    pub applied_theme: String,
    pub applied_lang: String,"""
assert old in s, "applied fields"
s = s.replace(old, new)

old = """            symbol_filter: String::new(),
            symbol_picker_open: false,
            symbol_focus_search: false,
            form: trade::OrderForm::default(),"""
new = """            symbol_filter: String::new(),
            symbol_info_open: false,
            form: trade::OrderForm::default(),"""
assert old in s, "init fields"
s = s.replace(old, new)

# 4. new(): phase 初始化 + 主题/语言应用
old = """        let phase = if vault::vault_exists(&dir) {
            Phase::Unlock
        } else {
            Phase::SetupMaster
        };
        let interval = Interval::from_str(&cfg.default_interval);"""
new = """        let phase = if vault::vault_exists(&dir) {
            Phase::Unlock
        } else {
            Phase::EnterKeys
        };
        let interval = Interval::from_str(&cfg.default_interval);
        theme::set_mode_str(&cfg.theme_mode);
        theme::apply(&cc.egui_ctx);
        crate::i18n::set_lang(if cfg.lang == "en" {
            crate::i18n::EN
        } else {
            crate::i18n::ZH
        });"""
assert old in s, "new phase"
s = s.replace(old, new)

old = """            tab: Tab::Spot,"""
new = """            tab: Tab::Spot,
            applied_theme: cfg.theme_mode.clone(),
            applied_lang: cfg.lang.clone(),"""
assert old in s, "init applied"
s = s.replace(old, new)

# 5. main_ui: Help 分支 + update 开头的主题/语言同步
old = """            Tab::Settings => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| settings::show(self, ui));
                });
            }
        }"""
new = """            Tab::Settings => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| settings::show(self, ui));
                });
            }
            Tab::Help => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| crate::ui::help::show(self, ui));
                });
            }
        }"""
assert old in s, "help arm"
s = s.replace(old, new)

# 6. topbar: 加入 Help 页签 + 右侧主题/语言快捷切换
old = """            for tab in [Tab::Spot, Tab::Futures, Tab::Portfolio, Tab::Settings] {
                if theme::nav_tab(ui, tab.label(), self.tab == tab) {
                    self.set_tab(tab);
                }
            }"""
new = """            for tab in [Tab::Spot, Tab::Futures, Tab::Portfolio, Tab::Settings, Tab::Help] {
                if theme::nav_tab(ui, tab.label(), self.tab == tab) {
                    self.set_tab(tab);
                }
            }"""
assert old in s, "nav tabs"
s = s.replace(old, new)

old = """            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let testnet = self.state.cfg.read().testnet;
                if testnet {
                    theme::badge(ui, "TESTNET", theme::cur().accent);
                } else {
                    theme::badge(ui, "实盘 · 真实资金", theme::cur().err);
                }"""
new = """            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 主题 / 语言快捷切换
                let (theme_txt, lang_txt) = if theme::is_light() { ("🌙", "EN") } else { ("☀", "中") };
                if ui.small_button(theme_txt).on_hover_text(crate::i18n::tr("切换主题")).clicked() {
                    let mut cfg = self.state.cfg.read().clone();
                    cfg.theme_mode = if cfg.theme_mode == "light" { "dark".into() } else { "light".into() };
                    cfg.save(&crate::config::data_dir());
                    *self.state.cfg.write() = cfg;
                    theme::set_mode_str(&cfg.theme_mode);
                    theme::apply(ctx);
                    self.applied_theme = cfg.theme_mode.clone();
                }
                if ui
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
                }
                ui.separator();
                let testnet = self.state.cfg.read().testnet;
                if testnet {
                    theme::badge(ui, "TESTNET", theme::cur().accent);
                } else {
                    theme::badge(ui, &crate::i18n::tr("实盘 · 真实资金").to_string(), theme::cur().err);
                }"""
assert old in s, "topbar toggles"
s = s.replace(old, new)

# 7. trading_ui: 左侧交易对边栏
old = """    /// 交易页：页头（交易对/周期/价格） + 图表 | 盘口+成交 | 下单面板 + 底部订单区
    fn trading_ui(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("trade_panel")"""
new = """    /// 交易页：左侧交易对列表 + 页头 + 图表 | 盘口+成交 | 下单面板 + 底部订单区
    fn trading_ui(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("symbols_panel")
            .resizable(true)
            .default_width(185.0)
            .width_range(140.0..=320.0)
            .frame(egui::Frame::default().fill(theme::cur().panel).inner_margin(8.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.symbols_sidebar(ui));
            });

        egui::SidePanel::right("trade_panel")"""
assert old in s, "sidebar panel"
s = s.replace(old, new)

open(p, 'w', encoding='utf-8').write(s)
print("app patch A done")
