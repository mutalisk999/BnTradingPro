# -*- coding: utf-8 -*-
import re

# ---------- 1. config.rs: 自选收藏字段 ----------
p = 'src/config.rs'
s = open(p, encoding='utf-8').read()
old = """    /// 界面语言（zh/en）
    pub lang: String,
    /// 主题（dark/light）
    pub theme_mode: String,"""
new = """    /// 界面语言（zh/en）
    pub lang: String,
    /// 主题（dark/light）
    pub theme_mode: String,
    /// 现货自选交易对（置顶显示）
    pub favorites_spot: Vec<String>,
    /// 合约自选交易对
    pub favorites_futures: Vec<String>,"""
assert old in s, "cfg fields"
s = s.replace(old, new)
old = """            lang: "zh".to_string(),
            theme_mode: "dark".to_string(),"""
new = """            lang: "zh".to_string(),
            theme_mode: "dark".to_string(),
            favorites_spot: Vec::new(),
            favorites_futures: Vec::new(),"""
assert old in s, "cfg default"
s = s.replace(old, new)
open(p, 'w', encoding='utf-8').write(s)

# ---------- 2. state.rs: 收藏读写辅助 ----------
p = 'src/engine/state.rs'
s = open(p, encoding='utf-8').read()
old = """    /// 单个交易对的规则（避免整本规则簿克隆）"""
new = """    /// 自选交易对列表
    pub fn favorites(&self, market: Market) -> Vec<String> {
        let cfg = self.cfg.read();
        match market {
            Market::Spot => cfg.favorites_spot.clone(),
            Market::UmFutures => cfg.favorites_futures.clone(),
        }
    }

    /// 切换自选（并落盘，不触发引擎重连）
    pub fn toggle_favorite(&self, market: Market, symbol: &str) {
        let cfg = {
            let mut cfg = self.cfg.write();
            let list = match market {
                Market::Spot => &mut cfg.favorites_spot,
                Market::UmFutures => &mut cfg.favorites_futures,
            };
            if let Some(i) = list.iter().position(|x| x == symbol) {
                list.remove(i);
            } else {
                list.insert(0, symbol.to_string());
            }
            cfg.clone()
        };
        cfg.save(&crate::config::data_dir());
    }

    /// 是否已加自选
    pub fn is_favorite(&self, market: Market, symbol: &str) -> bool {
        self.favorites(market).iter().any(|x| x == symbol)
    }

    /// 单个交易对的规则（避免整本规则簿克隆）"""
assert old in s, "state helpers"
s = s.replace(old, new)
open(p, 'w', encoding='utf-8').write(s)

# ---------- 3. main.rs: mod i18n + 主题/语言初始化 ----------
p = 'src/main.rs'
s = open(p, encoding='utf-8').read()
old = """mod app;
mod binance;
mod config;
mod engine;
mod security;
mod ui;"""
new = """mod app;
mod binance;
mod config;
mod engine;
mod i18n;
mod security;
mod ui;"""
assert old in s, "mods"
s = s.replace(old, new)
old = """    theme::apply_fonts(&cc.egui_ctx);"""
if old not in s:
    # main.rs 不调用字体；在 BnApp::new 里处理。此分支跳过
    pass
open(p, 'w', encoding='utf-8').write(s)

# ---------- 4. theme 常量清扫：theme::X → theme::cur().x ----------
mapping = [
    ("theme::PANEL2", "theme::cur().panel2"),
    ("theme::TEXT_DIM", "theme::cur().text_dim"),
    ("theme::ACCENT", "theme::cur().accent"),
    ("theme::BORDER", "theme::cur().border"),
    ("theme::CARD", "theme::cur().card"),
    ("theme::PANEL", "theme::cur().panel"),
    ("theme::TEXT", "theme::cur().text"),
    ("theme::BG", "theme::cur().bg"),
    ("theme::UP", "theme::cur().up"),
    ("theme::DOWN", "theme::cur().down"),
    ("theme::WARN", "theme::cur().warn"),
    ("theme::ERR", "theme::cur().err"),
]
files = [
    'src/app.rs',
    'src/ui/chart.rs', 'src/ui/market.rs', 'src/ui/trade.rs', 'src/ui/orders.rs',
    'src/ui/assets.rs', 'src/ui/settings.rs', 'src/ui/unlock.rs', 'src/ui/widgets.rs',
]
for f in files:
    s = open(f, encoding='utf-8').read()
    for a, b in mapping:
        s = s.replace(a, b)
    open(f, 'w', encoding='utf-8').write(s)

print("sweep done")
