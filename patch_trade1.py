# -*- coding: utf-8 -*-
p = 'src/ui/trade.rs'
s = open(p, encoding='utf-8').read()

# 1. 买/卖大按钮：34→36 高、圆角、更强配色
old = """    // ---------- 买/卖切换 ----------
    // 双向持仓模式下买卖即决定开多/开空，持仓方向随方向按钮联动（见 build_request）
    ui.columns(2, |cols| {
        let buy = egui::RichText::new(if market == Market::UmFutures {
            "买入 / 开多"
        } else {
            "买入"
        })
        .strong();
        let buy = egui::Button::new(buy).fill(if app.form.side == Side::Buy {
            theme::UP
        } else {
            theme::PANEL2
        });
        if cols[0]
            .add_sized([cols[0].available_width(), 30.0], buy)
            .clicked()
        {
            app.form.side = Side::Buy;
        }
        let sell = egui::RichText::new(if market == Market::UmFutures {
            "卖出 / 开空"
        } else {
            "卖出"
        })
        .strong();
        let sell = egui::Button::new(sell).fill(if app.form.side == Side::Sell {
            theme::DOWN
        } else {
            theme::PANEL2
        });
        if cols[1]
            .add_sized([cols[1].available_width(), 30.0], sell)
            .clicked()
        {
            app.form.side = Side::Sell;
        }
    });"""
new = """    // ---------- 买/卖切换 ----------
    // 双向持仓模式下买卖即决定开多/开空，持仓方向随方向按钮联动（见 build_request）
    ui.columns(2, |cols| {
        let buy = egui::RichText::new(if market == Market::UmFutures {
            "买入 / 开多"
        } else {
            "买入"
        })
        .strong()
        .size(14.5)
        .color(Color32::WHITE);
        let buy = egui::Button::new(buy)
            .fill(if app.form.side == Side::Buy {
                theme::UP
            } else {
                Color32::from_rgb(30, 37, 48)
            })
            .rounding(5.0);
        if cols[0]
            .add_sized([cols[0].available_width(), 36.0], buy)
            .clicked()
        {
            app.form.side = Side::Buy;
        }
        let sell = egui::RichText::new(if market == Market::UmFutures {
            "卖出 / 开空"
        } else {
            "卖出"
        })
        .strong()
        .size(14.5)
        .color(Color32::WHITE);
        let sell = egui::Button::new(sell)
            .fill(if app.form.side == Side::Sell {
                theme::DOWN
            } else {
                Color32::from_rgb(30, 37, 48)
            })
            .rounding(5.0);
        if cols[1]
            .add_sized([cols[1].available_width(), 36.0], sell)
            .clicked()
        {
            app.form.side = Side::Sell;
        }
    });
    ui.add_space(2.0);"""
assert old in s, "buy sell"
s = s.replace(old, new)

# 2. 订单类型等区域卡片化：类型 + （合约方向/杠杆）
old = """    // ---------- 订单类型 ----------
    ui.horizontal(|ui| {
        ui.label("类型");
        let kinds = UiKind::available(market);
        let cur = if kinds.contains(&app.form.kind) {
            app.form.kind
        } else {
            kinds[0]
        };
        egui::ComboBox::from_id_salt("order_kind")
            .selected_text(cur.label())
            .width(110.0)
            .show_ui(ui, |ui| {
                for k in kinds {
                    ui.selectable_value(&mut app.form.kind, k, k.label());
                }
            });
    });"""
new = """    // ---------- 订单类型 ----------
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::card_label("类型"));
            let kinds = UiKind::available(market);
            let cur = if kinds.contains(&app.form.kind) {
                app.form.kind
            } else {
                kinds[0]
            };
            egui::ComboBox::from_id_salt("order_kind")
                .selected_text(cur.label())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for k in kinds {
                        ui.selectable_value(&mut app.form.kind, k, k.label());
                    }
                });
        });"""
assert old in s, "kind card open"
s = s.replace(old, new)

open(p, 'w', encoding='utf-8').write(s)
print("trade step1 done")
