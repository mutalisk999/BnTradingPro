//! 使用说明页：快速开始、功能导览、常见问题与风险提示（随语言切换）
//!
//! 排版与资产/设置页同构：每节一张卡片（card），避免整页纯文字流。

use crate::app::BnApp;
use crate::i18n::tr;
use crate::ui::theme;
use eframe::egui;

/// 一节 = 一张卡片：标题行 + 内容体
fn section(ui: &mut egui::Ui, title: &'static str, add: impl FnOnce(&mut egui::Ui)) {
    theme::card(ui, |ui| {
        ui.label(
            egui::RichText::new(tr(title))
                .size(theme::FONT_LG)
                .strong()
                .color(theme::cur().text),
        );
        ui.add_space(2.0);
        add(ui);
    });
}

fn para(ui: &mut egui::Ui, text: &'static str) {
    ui.label(
        egui::RichText::new(tr(text))
            .size(theme::FONT_BODY)
            .color(theme::cur().text),
    );
}

fn bullet(ui: &mut egui::Ui, text: &'static str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("•").color(theme::cur().accent));
        ui.label(
            egui::RichText::new(tr(text))
                .size(theme::FONT_BODY)
                .color(theme::cur().text),
        );
    });
}

fn kv(ui: &mut egui::Ui, k: &'static str, v: &'static str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(tr(k))
                .size(theme::FONT_MD)
                .strong()
                .color(theme::cur().accent),
        );
        ui.label(
            egui::RichText::new(tr(v))
                .size(theme::FONT_MD)
                .color(theme::cur().text),
        );
    });
}

pub fn show(_app: &mut BnApp, ui: &mut egui::Ui) {
    section(ui, "快速开始", |ui| {
        bullet(ui, "1. 在 testnet.binance.vision（现货）或 testnet.binancefuture.com（合约）用 GitHub 账号申请免费测试网 API Key");
        bullet(
            ui,
            "2. 启动本程序，在配置页输入 API Key / Secret 并设置主密码（用于加密保存到本机）",
        );
        bullet(
            ui,
            "3. 默认即测试网模式（顶部金色 TESTNET 徽标），可查看行情、下单、撤单，放心演练",
        );
        bullet(ui, "4. 切换实盘前请再三确认：实盘密钥来自 binance.com 的 API 管理，建议绑定 IP 白名单并只开启读取+交易权限");
    });

    section(ui, "页面导览", |ui| {
        kv(ui, "现货交易 / 期货交易", "左右结构：左侧交易对列表（支持搜索与⭐自选置顶），中间 K 线与订单区，右侧盘口、成交明细与下单面板。两个市场各自记忆交易对、周期与表单。");
        kv(ui, "K 线图", "滚轮缩放、按住拖动平移、双击回到最新。MA5/10/20/60 均线可在图上方开关；图上叠加当前挂单价、止损触发价与合约持仓开仓价。悬停显示十字光标与 OHLC 详情。");
        kv(ui, "下单面板", "限价/市价/止损止盈；比例按钮按可用余额估算数量；提交前有确认弹窗（实盘红色警示）。价格与数量会按交易所过滤器自动量化。");
        kv(
            ui,
            "订单区",
            "当前挂单可单笔撤销、一键撤销该交易对全部挂单或本市场全部挂单；合约持仓支持行内 TP/SL 附加、单笔平仓与一键全部平仓（均走确认弹窗）。",
        );
        kv(ui, "资产总览", "汇总现货资产、合约资金概况与当前持仓。");
        kv(
            ui,
            "设置",
            "环境切换（测试网/实盘）、代理、端点覆盖、语言与主题、密钥管理。",
        );
    });

    section(ui, "交易对与自选", |ui| {
        bullet(
            ui,
            "左侧列表顶部搜索框实时过滤；点击行尾的星标可将交易对加入自选，自选排在列表最顶部",
        );
        bullet(
            ui,
            "自选按市场分别保存（现货与合约互不影响），持久化到本机配置",
        );
    });

    section(ui, "快捷键（交易页）", |ui| {
        kv(ui, "B / S", "买入 / 卖出方向切换，并聚焦数量输入框（输入框获得焦点时不生效）");
        kv(ui, "Ctrl+Shift+C", "撤销当前交易对的全部挂单");
    });

    section(ui, "网络与代理", |ui| {
        bullet(ui, "程序直连 Binance；如需代理，在设置页启用并填写 http:// 或 socks5:// 地址（支持用户名密码），REST 与 WebSocket 全部生效");
        bullet(
            ui,
            "有自建网关或反代时，可在设置页覆盖四个端点（现货/合约 × REST/WS）",
        );
    });

    section(ui, "常见问题", |ui| {
        kv(
            ui,
            "提示 -2015 / 签名错误",
            "检查 Key 是否用对环境（测试网与实盘互不通用）、是否绑定 IP 白名单、Secret 是否复制完整。",
        );
        kv(
            ui,
            "提示 -1021 时间戳偏差",
            "程序会自动与服务器校时并重试；若反复出现请检查系统时钟。",
        );
        kv(
            ui,
            "行情显示未连接",
            "检查网络与代理设置；状态灯为红色时鼠标悬停可查看错误详情。",
        );
        kv(
            ui,
            "忘了主密码",
            "在解锁页点「重置凭据库」删除本机加密文件后重新配置（密钥需重新输入）。",
        );
    });

    section(ui, "风险提示", |ui| {
        para(
            ui,
            "本工具仅供个人交易辅助与研究学习，不构成任何投资建议。加密货币交易风险极高，使用实盘交易产生的一切后果由使用者自行承担。请务必先在测试网完整演练后再考虑实盘。",
        );
    });

    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(format!("BnTradingPro v{}", env!("CARGO_PKG_VERSION")))
            .size(theme::FONT_SM)
            .color(theme::cur().text_dim),
    );
}
