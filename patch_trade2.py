# -*- coding: utf-8 -*-
p = 'src/ui/trade.rs'
s = open(p, encoding='utf-8').read()

# 卡片闭合：把"合约方向/只减仓/杠杆"包进类型卡片内，杠杆块结束后闭合 card
old = """        });
    }

    // ---------- 价格 / 触发价 / 数量 ----------"""
new = """        });
        });
    }

    // ---------- 价格 / 触发价 / 数量 ----------"""
assert old in s, "card close"
s = s.replace(old, new, 1)

# 价格/数量卡片化：包住 价格、触发价、数量、比例填充
old = """    // ---------- 价格 / 触发价 / 数量 ----------
    if app.form.kind != UiKind::Market {"""
new = """    // ---------- 价格 / 触发价 / 数量 ----------
    theme::card(ui, |ui| {
    if app.form.kind != UiKind::Market {"""
assert old in s, "price card open"
s = s.replace(old, new, 1)

old = """    ui.horizontal(|ui| {
        ui.label("比例");
        for pct in [25.0, 50.0, 75.0, 100.0] {
            if ui
                .button(format!("{}%", pct as i32))
                .on_hover_text("按可用余额估算")
                .clicked()
            {
                fill_by_pct(app, pct / 100.0);
            }
        }
    });"""
new = """    ui.horizontal(|ui| {
        ui.label(theme::card_label("比例"));
        for pct in [25.0, 50.0, 75.0, 100.0] {
            if ui
                .button(format!("{}%", pct as i32))
                .on_hover_text("按可用余额估算")
                .clicked()
            {
                fill_by_pct(app, pct / 100.0);
            }
        }
    });
    }); // 价格数量卡片"""
assert old in s, "pct card close"
s = s.replace(old, new, 1)

# 卡片内的标签统一小标签样式
s = s.replace('ui.label("价格");\n            widgets::num_field', 'ui.label(theme::card_label("价格"));\n            widgets::num_field')
s = s.replace('ui.label("触发价");\n            widgets::num_field', 'ui.label(theme::card_label("触发价"));\n            widgets::num_field')
s = s.replace('ui.label("数量");\n            widgets::num_field', 'ui.label(theme::card_label("数量"));\n            widgets::num_field')
s = s.replace('ui.label("杠杆");', 'ui.label(theme::card_label("杠杆"));')
s = s.replace('ui.label("持仓方向");', 'ui.label(theme::card_label("持仓方向"));')

# 提交按钮：38 高、圆角、白字
old = """    let btn = egui::Button::new(
        egui::RichText::new(format!("{word} {symbol}"))
            .strong()
            .size(15.0),
    )
    .fill(color)
    .min_size(egui::vec2(ui.available_width(), 34.0));"""
new = """    let btn = egui::Button::new(
        egui::RichText::new(format!("{word} {symbol}"))
            .strong()
            .size(15.5)
            .color(Color32::WHITE),
    )
    .fill(color)
    .rounding(5.0)
    .min_size(egui::vec2(ui.available_width(), 38.0));"""
assert old in s, "submit"
s = s.replace(old, new)

open(p, 'w', encoding='utf-8').write(s)
print("trade step2 done")
