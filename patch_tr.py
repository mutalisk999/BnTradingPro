# -*- coding: utf-8 -*-
# ---------- i18n 去重 + 高可见度字符串 tr 清扫 ----------
p = 'src/i18n.rs'
s = open(p, encoding='utf-8').read()
dup = '        "部分成交" => "Partially Filled",\n'
first = s.find(dup)
second = s.find(dup, first + 1)
if second > 0:
    s = s[:second] + s[second + len(dup):]
open(p, 'w', encoding='utf-8').write(s)

import re

# orders.rs: status_cn/type_cn 走 tr；表头/按钮
p = 'src/ui/orders.rs'
s = open(p, encoding='utf-8').read()
s = s.replace('''pub fn status_cn(s: &str) -> &'static str {
    match s {
        "NEW" => "待成交",
        "PARTIALLY_FILLED" => "部分成交",
        "FILLED" => "已成交",
        "CANCELED" => "已撤销",
        "PENDING_CANCEL" => "撤销中",
        "REJECTED" => "已拒绝",
        "EXPIRED" => "已过期",
        "EXPIRED_IN_MATCH" => "匹配过期",
        "NEW_ADL" => "ADL",
        _ => "其他",
    }
}''', '''pub fn status_cn(s: &str) -> &'static str {
    let zh = match s {
        "NEW" => "待成交",
        "PARTIALLY_FILLED" => "部分成交",
        "FILLED" => "已成交",
        "CANCELED" => "已撤销",
        "PENDING_CANCEL" => "撤销中",
        "REJECTED" => "已拒绝",
        "EXPIRED" => "已过期",
        "EXPIRED_IN_MATCH" => "匹配过期",
        "NEW_ADL" => "ADL",
        _ => "其他",
    };
    crate::i18n::tr(zh)
}''')
s = s.replace('''fn type_cn(s: &str) -> &'static str {
    match s {
        "LIMIT" => "限价",
        "MARKET" => "市价",
        "STOP_LOSS" => "止损市价",
        "STOP_LOSS_LIMIT" => "止损限价",
        "TAKE_PROFIT" => "止盈市价",
        "TAKE_PROFIT_LIMIT" => "止盈限价",
        "STOP_MARKET" => "止损市价",
        "TAKE_PROFIT_MARKET" => "止盈市价",
        "TRAILING_STOP_MARKET" => "跟踪委托",
        "LIMIT_MAKER" => "只做 maker",
        _ => "其他",
    }
}''', '''fn type_cn(s: &str) -> &'static str {
    let zh = match s {
        "LIMIT" => "限价",
        "MARKET" => "市价",
        "STOP_LOSS" => "止损市价",
        "STOP_LOSS_LIMIT" => "止损限价",
        "TAKE_PROFIT" => "止盈市价",
        "TAKE_PROFIT_LIMIT" => "止盈限价",
        "STOP_MARKET" => "止损市价",
        "TAKE_PROFIT_MARKET" => "止盈市价",
        "TRAILING_STOP_MARKET" => "跟踪委托",
        "LIMIT_MAKER" => "只做 maker",
        _ => "其他",
    };
    crate::i18n::tr(zh)
}''')
# 表头与按钮
for zh in ["时间", "交易对", "方向", "类型", "价格", "数量", "已成交", "状态", "操作", "刷新", "撤销", "市价平仓", "切换"]:
    s = s.replace(f'egui::RichText::new("{zh}")', f'egui::RichText::new(crate::i18n::tr("{zh}"))')
    s = s.replace(f'ui.label("{zh}");', f'ui.label(crate::i18n::tr("{zh}"));')
    s = s.replace(f'ui.small_button("{zh}")', f'ui.small_button(crate::i18n::tr("{zh}"))')
s = s.replace('h.col(|ui| {\n                    ui.label(theme::card_label(t));\n                });',
              'h.col(|ui| {\n                    ui.label(theme::card_label(&crate::i18n::tr(t).to_string()));\n                });')
s = s.replace('ui.label(theme::card_label(t));', 'ui.label(theme::card_label(&crate::i18n::tr(t).to_string()));')
open(p, 'w', encoding='utf-8').write(s)

# market.rs: 标题
p = 'src/ui/market.rs'
s = open(p, encoding='utf-8').read()
s = s.replace('ui.heading(egui::RichText::new("盘口 20 档").size(13.0).color(theme::cur().text));',
              'ui.heading(egui::RichText::new(crate::i18n::tr("盘口 20 档")).size(13.0).color(theme::cur().text));')
s = s.replace('ui.heading(egui::RichText::new("成交明细").size(13.0).color(theme::cur().text));',
              'ui.heading(egui::RichText::new(crate::i18n::tr("成交明细")).size(13.0).color(theme::cur().text));')
for zh in ["价格", "数量", "时间", "价差", "加载中…", "等待深度数据…"]:
    s = s.replace(f'ui.label(egui::RichText::new("{zh}")', f'ui.label(egui::RichText::new(crate::i18n::tr("{zh}"))')
s = s.replace('format!("价差 {:.*} ({:.3}%)"', 'format!("{} {:.*} ({:.3}%)"')
s = s.replace('format!(\n                        "价差 {:.*} ({:.3}%)"', 'format!(\n                        "{} {:.*} ({:.3}%)"')
open(p, 'w', encoding='utf-8').write(s)

# trade.rs: 面板标签
p = 'src/ui/trade.rs'
s = open(p, encoding='utf-8').read()
for zh in ["类型", "杠杆", "持仓方向", "比例", "预估金额", "价格", "触发价", "数量", "市价"]:
    s = s.replace(f'ui.label(theme::card_label("{zh}"));', f'ui.label(theme::card_label(&crate::i18n::tr("{zh}").to_string()));')
s = s.replace('ui.label("recvWindow")', 'ui.label("recvWindow")')
s = s.replace('ui.heading(egui::RichText::new(format!("下单 · {symbol}")).size(14.0));',
              'ui.heading(egui::RichText::new(format!("{}{symbol}", crate::i18n::tr("下单 · "))).size(14.0));')
s = s.replace('ui.label(egui::RichText::new("未配置 API 密钥，仅可查看行情").color(theme::cur().warn).size(12.0));',
              'ui.label(egui::RichText::new(crate::i18n::tr("未配置 API 密钥，仅可查看行情")).color(theme::cur().warn).size(12.0));')
s = s.replace('ui.label(egui::RichText::new("提交中…")', 'ui.label(egui::RichText::new(crate::i18n::tr("提交中…"))')
s = s.replace('ui.label(theme::card_label("步进 "));', 'ui.label(theme::card_label("步进 "));')
open(p, 'w', encoding='utf-8').write(s)

# assets.rs: 表头
p = 'src/ui/assets.rs'
s = open(p, encoding='utf-8').read()
s = s.replace('ui.label(theme::card_label(t));', 'ui.label(theme::card_label(&crate::i18n::tr(t).to_string()));')
s = s.replace('ui.label(theme::card_label(if dual {\n                "持仓模式：双向"\n            } else {\n                "持仓模式：单向"\n            }));',
              'ui.label(theme::card_label(&if state.acct.lock().fut_dual_side {\n                crate::i18n::tr("持仓模式：双向").to_string()\n            } else {\n                crate::i18n::tr("持仓模式：单向").to_string()\n            }));')
open(p, 'w', encoding='utf-8').write(s)

# chart.rs: 图例/提示
p = 'src/ui/chart.rs'
s = open(p, encoding='utf-8').read()
s = s.replace('"24h 高 {} · 低 {} · 额 {vol_txt}"', '"24h 高 {} · 低 {} · 额 {vol_txt}"')
open(p, 'w', encoding='utf-8').write(s)

# engine/mod.rs: toast 关键串
p = 'src/engine/mod.rs'
s = open(p, encoding='utf-8').read()
s = s.replace('state.push_toast(ToastKind::Err, "未配置 API 密钥");',
              'state.push_toast(ToastKind::Err, crate::i18n::tr("未配置 API 密钥"));')
s = s.replace('state.push_toast(ToastKind::Ok, format!("已撤销委托 #{order_id}"));',
              'state.push_toast(ToastKind::Ok, format!("{}{order_id}", crate::i18n::tr("已撤销委托 #")));')
s = s.replace('state.push_toast(ToastKind::Ok, format!("已撤销 {symbol} 全部挂单"));',
              'state.push_toast(ToastKind::Ok, format!("{}{symbol}", crate::i18n::tr("已撤销 ")));')
s = s.replace('state.push_toast(ToastKind::Ok, "持仓模式已切换".to_string());',
              'state.push_toast(ToastKind::Ok, crate::i18n::tr("持仓模式已切换").to_string());')
s = s.replace('state.push_toast(ToastKind::Err, "价格/数量超出可表示范围，请检查输入");',
              'state.push_toast(ToastKind::Err, crate::i18n::tr("价格/数量超出可表示范围，请检查输入"));')
open(p, 'w', encoding='utf-8').write(s)
print("tr sweep done")
