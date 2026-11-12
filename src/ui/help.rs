//! 使用说明页：快速开始、功能导览、常见问题与风险提示（随语言切换）
//!
//! 排版与资产/设置页同构：每节一张卡片（card），避免整页纯文字流。
//! 里程碑 9：GPUI 移植（静态文本 + 卡片布局）。

use crate::app::BnApp;
use crate::i18n::tr;
use crate::ui::theme::{cur, faded, font_body, font_lg, font_md, font_sm};
use crate::ui::widgets;
use gpui::{
    div, px, Context, Div, FontWeight, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
};

/// 一节 = 一张卡片：标题行 + 内容体
fn section(title: &'static str, body: Div) -> Div {
    widgets::card()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .text_size(font_lg())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cur().text)
                .child(tr(title)),
        )
        .child(body)
}

fn para(text: &'static str) -> Div {
    div()
        .text_size(font_body())
        .text_color(cur().text)
        .child(tr(text))
}

/// 项目符号行：• 着色 + 正文可换行（窄窗折行不裁切）
fn bullet(text: &'static str) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(px(6.0))
        .child(div().text_size(font_body()).text_color(cur().accent).child("•"))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(font_body())
                .text_color(cur().text)
                .child(tr(text)),
        )
}

/// 键值对行：键 accent 加粗、值正文色，均可换行
fn kv(k: &'static str, v: &'static str) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .child(
            div()
                .flex_shrink_0()
                .text_size(font_md())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cur().accent)
                .child(tr(k)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(font_md())
                .text_color(cur().text)
                .child(tr(v)),
        )
}

/// 多条内容纵向排列
fn col(items: Vec<Div>) -> Div {
    let mut d = div().flex().flex_col().gap(px(4.0));
    for it in items {
        d = d.child(it);
    }
    d
}

pub fn show(_app: &mut BnApp, _cx: &mut Context<BnApp>) -> Div {
    let t = cur();
    let root = div()
        .id("help_page")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(section(
                    "快速开始",
                    col(vec![
                        bullet("1. 在 testnet.binance.vision（现货）或 testnet.binancefuture.com（合约）用 GitHub 账号申请免费测试网 API Key"),
                        bullet("2. 启动本程序，在配置页输入 API Key / Secret 并设置主密码（用于加密保存到本机）"),
                        bullet("3. 默认即测试网模式（顶部金色 TESTNET 徽标），可查看行情、下单、撤单，放心演练"),
                        bullet("4. 切换实盘前请再三确认：实盘密钥来自 binance.com 的 API 管理，建议绑定 IP 白名单并只开启读取+交易权限"),
                    ]),
                ))
                .child(section(
                    "页面导览",
                    col(vec![
                        kv("现货交易 / 期货交易", "左右结构：左侧交易对列表（支持搜索与⭐自选置顶），中间 K 线与订单区，右侧盘口、成交明细与下单面板。两个市场各自记忆交易对、周期与表单。"),
                        kv("K 线图", "滚轮缩放、按住拖动平移、双击回到最新。MA5/10/20/60 均线可在图上方开关；图上叠加当前挂单价、止损触发价与合约持仓开仓价。悬停显示十字光标与 OHLC 详情。K 线缓存于本地磁盘，切换交易对或周期先秒开缓存，再增量刷新最新数据。"),
                        kv("下单面板", "限价/市价/止损止盈；合约另有跟踪委托（回调幅度 0.1-10%），现货另有 OCO（限价 + 止损限价成组，一边成交另一边自动撤销）；限价单可选 GTC/IOC/FOK（合约另有 GTX Post-Only）；合约下单可附带止盈/止损一次提交。比例按钮按可用余额估算数量；提交前有确认弹窗（实盘红色警示）。价格与数量会按交易所过滤器自动量化。"),
                        kv("订单区", "当前挂单可单笔撤销、一键撤销该交易对全部挂单或本市场全部挂单；合约持仓支持行内 TP/SL、平仓、反手（市价平仓后反向开同等数量）与全部平仓（均走确认弹窗）。订单历史与成交历史可一键导出 CSV（UTF-8 带 BOM，Excel 可直接打开），文件保存在数据目录 exports/ 下。"),
                        kv("资产总览", "汇总现货资产、合约资金概况与当前持仓。"),
                        kv("设置", "环境切换（测试网/实盘）、代理、端点覆盖、语言与主题、密钥管理。"),
                    ]),
                ))
                .child(section(
                    "交易对与自选",
                    col(vec![
                        bullet("左侧列表顶部搜索框实时过滤；点击行尾的星标可将交易对加入自选，自选排在列表最顶部"),
                        bullet("自选按市场分别保存（现货与合约互不影响），持久化到本机配置"),
                    ]),
                ))
                .child(section(
                    "盘口与成交明细",
                    col(vec![kv(
                        "盘口聚合档位",
                        "盘口上方的档位胶囊按 1/10/100/1000 倍最小变动单位聚合挂单量，便于观察大单密集区",
                    )]),
                ))
                .child(section(
                    "价格预警",
                    col(vec![
                        bullet("交易页顶部的「预警」按钮打开当前交易对的预警面板：输入目标价即可添加，上破/下破方向按当前价自动判断，同价位不会重复添加"),
                        bullet("预警在本地对最新价实时评估，到价弹出提示并自动移除（一次性触发）；列表随配置持久化，重启后仍生效"),
                    ]),
                ))
                .child(section(
                    "快捷键（交易页）",
                    col(vec![
                        kv("B / S", "买入 / 卖出方向切换，并聚焦数量输入框（输入框获得焦点时不生效）"),
                        kv("Ctrl+Shift+C", "连按两次撤销当前交易对的全部挂单"),
                    ]),
                ))
                .child(section(
                    "网络与代理",
                    col(vec![
                        bullet("程序直连 Binance；如需代理，在设置页启用并填写 http:// 或 socks5:// 地址（支持用户名密码），REST 与 WebSocket 全部生效"),
                        bullet("有自建网关或反代时，可在设置页覆盖四个端点（现货/合约 × REST/WS）"),
                        bullet("设置页「WS 下单」开启后，合约订单经 WebSocket 通道提交（延迟更低）；任何失败会自动回退 REST"),
                    ]),
                ))
                .child(section(
                    "常见问题",
                    col(vec![
                        kv("提示 -2015 / 签名错误", "检查 Key 是否用对环境（测试网与实盘互不通用）、是否绑定 IP 白名单、Secret 是否复制完整。"),
                        kv("提示 -1021 时间戳偏差", "程序会自动与服务器校时并重试；若反复出现请检查系统时钟。"),
                        kv("行情显示未连接", "检查网络与代理设置；状态灯为红色时鼠标悬停可查看错误详情。"),
                        kv("忘了主密码", "在解锁页点「重置凭据库」删除本机加密文件后重新配置（密钥需重新输入）。"),
                    ]),
                ))
                .child(section(
                    "风险提示",
                    para("本工具仅供个人交易辅助与研究学习，不构成任何投资建议。加密货币交易风险极高，使用实盘交易产生的一切后果由使用者自行承担。请务必先在测试网完整演练后再考虑实盘。"),
                ))
                .child(
                    div()
                        .pt(px(2.0))
                        .text_size(font_sm())
                        .text_color(faded(t.text_dim, 0.85))
                        .child(format!("BnTradingPro v{}", env!("CARGO_PKG_VERSION"))),
                ),
        );

    // 外层再包一层弹性容器（root 带 id/滚动为 Stateful<Div>，页面统一返回 Div）
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .child(root)
}
