//! 设置页：环境、代理、端点覆盖、参数与密钥管理
//!
//! 输入绑定到 `BnApp.settings` 草稿（输入框状态必须跨帧存活），保存时写回配置。
//! 文本框实体挂在 `BnApp.settings_fields`（与下单表单 TradeFields 同一套
//! attach/on_change/sync 模式）；档位类选项用 pill 行（对应 egui 版 ComboBox）。
//!
//! 里程碑 9：GPUI 移植。egui 版的 form_center_probe 依赖 egui Ui 几何探针，
//! 随框架移除——GPUI 版行内垂直居中由 flex items_center 结构性保证，M10 评估
//! 是否可用 gpui test-support 写等价布局回归。

use crate::app::{BnApp, Phase, SettingsDraft};
use crate::config::{self, AppConfig};
use crate::engine::state::ToastKind;
use crate::engine::Command;
use crate::i18n::tr;
use crate::security::vault;
use crate::ui::textfield::TextField;
use crate::ui::theme::{self, cur, font_body, font_caption, font_lg, font_md, font_sm};
use crate::ui::widgets;
use gpui::{
    prelude::FluentBuilder, AppContext, div, px, Context, Div, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Stateful, StatefulInteractiveElement, Styled,
};
use std::sync::Arc;

// ---------- 输入框实体组 ----------

/// 设置页文本框：与草稿字段一一对应（数字框受输入过滤约束，非法值不写草稿）
#[derive(Clone)]
pub struct SettingsFields {
    pub proxy_url: Entity<TextField>,
    pub default_symbol: Entity<TextField>,
    pub recv_window: Entity<TextField>,
    pub http_timeout: Entity<TextField>,
    pub ov_spot_rest: Entity<TextField>,
    pub ov_spot_ws: Entity<TextField>,
    pub ov_fut_rest: Entity<TextField>,
    pub ov_fut_ws: Entity<TextField>,
}

impl SettingsFields {
    pub fn new(cx: &mut Context<BnApp>) -> Self {
        let mk = |cx: &mut Context<BnApp>, id: &'static str, numeric: bool, ph: &'static str| {
            cx.new(|c| {
                let mut f = TextField::new(c);
                f.set_id(id)
                    .set_numeric(numeric)
                    .set_placeholder(ph)
                    .set_font_size(font_md());
                f
            })
        };
        Self {
            proxy_url: mk(cx, "set_proxy", false, tr("http://127.0.0.1:7890 或 socks5://127.0.0.1:7890（支持 user:pass@）")),
            default_symbol: mk(cx, "set_def_sym", false, "BTCUSDT"),
            recv_window: mk(cx, "set_recv_win", true, "1000"),
            http_timeout: mk(cx, "set_http_to", true, "10"),
            ov_spot_rest: mk(cx, "set_ov_spot_rest", false, "https://api.binance.com"),
            ov_spot_ws: mk(cx, "set_ov_spot_ws", false, "wss://stream.binance.com:9443"),
            ov_fut_rest: mk(cx, "set_ov_fut_rest", false, "https://fapi.binance.com"),
            ov_fut_ws: mk(cx, "set_ov_fut_ws", false, "wss://fstream.binance.com"),
        }
    }

    /// 挂接写回回调（BnApp::new 里实体创建后调用：回调需要 WeakEntity 句柄）。
    /// 数字框解析失败不写草稿（草稿保持上一个合法值）
    pub fn attach(&mut self, weak: &gpui::WeakEntity<BnApp>, cx: &mut Context<BnApp>) {
        macro_rules! bind {
            ($e:expr, $set:expr) => {
                $e.update(cx, |tf, _| {
                    let weak = weak.clone();
                    tf.set_on_change(Box::new(move |s, _, cx| {
                        let s = s.to_string();
                        let _ = weak.update(cx, |app: &mut BnApp, _| $set(app, s));
                    }));
                });
            };
        }
        bind!(self.proxy_url, |app: &mut BnApp, v: String| app.settings.proxy_url = v);
        bind!(self.default_symbol, |app: &mut BnApp, v: String| app
            .settings
            .default_symbol = v);
        bind!(self.recv_window, |app: &mut BnApp, v: String| if let Ok(n) =
            v.trim().parse::<u64>()
        {
            app.settings.recv_window_ms = n;
        });
        bind!(self.http_timeout, |app: &mut BnApp, v: String| if let Ok(n) =
            v.trim().parse::<u64>()
        {
            app.settings.http_timeout_secs = n;
        });
        bind!(self.ov_spot_rest, |app: &mut BnApp, v: String| app
            .settings
            .override_spot_rest = v);
        bind!(self.ov_spot_ws, |app: &mut BnApp, v: String| app
            .settings
            .override_spot_ws = v);
        bind!(self.ov_fut_rest, |app: &mut BnApp, v: String| app
            .settings
            .override_fut_rest = v);
        bind!(self.ov_fut_ws, |app: &mut BnApp, v: String| app
            .settings
            .override_fut_ws = v);
    }

    /// 外部变更回灌：文本不同才 set（正在输入的框两侧已一致，不会重置光标）
    fn sync(e: &Entity<TextField>, val: String, cx: &mut Context<BnApp>) {
        e.update(cx, |f, _| {
            if f.text() != val {
                f.set_text(val);
            }
        });
    }

    pub fn sync_all(&self, app: &BnApp, cx: &mut Context<BnApp>) {
        let d = &app.settings;
        Self::sync(&self.proxy_url, d.proxy_url.clone(), cx);
        Self::sync(&self.default_symbol, d.default_symbol.clone(), cx);
        Self::sync(&self.recv_window, d.recv_window_ms.to_string(), cx);
        Self::sync(&self.http_timeout, d.http_timeout_secs.to_string(), cx);
        Self::sync(&self.ov_spot_rest, d.override_spot_rest.clone(), cx);
        Self::sync(&self.ov_spot_ws, d.override_spot_ws.clone(), cx);
        Self::sync(&self.ov_fut_rest, d.override_fut_rest.clone(), cx);
        Self::sync(&self.ov_fut_ws, d.override_fut_ws.clone(), cx);
    }
}

// ---------- 行内小控件 ----------

/// 档位 pill 行：当前值高亮，点击写草稿（pick 里可附即时生效逻辑）
fn pills(
    cx: &mut Context<BnApp>,
    id: &'static str,
    options: Vec<(&'static str, String)>,
    current: &str,
    pick: impl Fn(&mut BnApp, &str) + Clone + 'static,
) -> Div {
    let mut d = div().flex().flex_row().items_center().gap(px(theme::SPACE_SM));
    for (i, (v, label)) in options.into_iter().enumerate() {
        let pick = pick.clone();
        let active = v == current;
        d = d.child(
            widgets::pill_tab((id, i), label, active).on_click(cx.listener(
                move |app: &mut BnApp, _, _, cx| {
                    pick(app, v);
                    cx.notify();
                },
            )),
        );
    }
    d
}

/// 复选框方块（与 trade.rs 下单表单同一套 14×14 样式）
fn cb_box(checked: bool) -> Div {
    let t = cur();
    div()
        .w(px(14.0))
        .h(px(14.0))
        .rounded(px(3.0))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if checked { t.accent } else { t.text_dim })
        .text_size(font_caption())
        .text_color(t.on_accent)
        .when(checked, |d| d.bg(t.accent).child("✓"))
}

/// 复选框行：整行可点（egui checkbox 语义），切换回调可附即时生效逻辑
fn checkbox_row(
    cx: &mut Context<BnApp>,
    id: &'static str,
    checked: bool,
    label: &'static str,
    on_toggle: impl Fn(&mut BnApp, bool) + Clone + 'static,
) -> Stateful<Div> {
    let t = cur();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(theme::SPACE_MD))
        .cursor_pointer()
        .hover(|s| s.text_color(t.accent))
        .on_click(cx.listener(move |app: &mut BnApp, _, _, cx| {
            on_toggle(app, !checked);
            cx.notify();
        }))
        .child(cb_box(checked))
        .child(
            div()
                .text_size(font_body())
                .text_color(cur().text)
                .child(tr(label)),
        )
}

/// 定宽文本框容器
fn field_box(e: &Entity<TextField>, w: f32) -> Div {
    div().w(px(w)).child(e.clone())
}

/// 小号灰色提示
fn hint(text: &'static str) -> Div {
    div().text_size(font_sm()).text_color(cur().text_dim).child(tr(text))
}

/// 应用：草稿 → 配置 + 重连（原 egui 版语义逐行迁移）
fn apply_settings(app: &mut BnApp) {
    let state = Arc::clone(&app.state);
    // 自选列表不在设置页编辑（由侧栏星标实时修改 state.cfg），必须以 live
    // 值覆盖草稿，否则保存设置会把用户启动后添加的自选静默回滚到旧快照。
    // 语言/主题设置页可编辑，以草稿为准（顶栏快捷切换会同步进草稿）。
    {
        let live = state.cfg.read();
        app.settings.favorites_spot = live.favorites_spot.clone();
        app.settings.favorites_futures = live.favorites_futures.clone();
    }
    let mut cfg: AppConfig = std::mem::take(&mut app.settings).into_cfg();
    // 价格预警同样不在设置页编辑：以 live 值保留，防止保存设置清空预警
    cfg.alerts = state.cfg.read().alerts.clone();
    // 保留草稿（into_cfg 消耗了草稿），保证继续编辑不丢字段
    app.settings = SettingsDraft::from_cfg(&cfg);
    // 测试网 → 实盘时不沿用端点覆盖：第三方测试网端点会把携带 API Key 的
    // 实盘签名请求发往该主机，自动清空并提示
    let was_testnet = state.cfg.read().testnet;
    if was_testnet && !cfg.testnet {
        let had_override = !cfg.override_spot_rest.trim().is_empty()
            || !cfg.override_spot_ws.trim().is_empty()
            || !cfg.override_fut_rest.trim().is_empty()
            || !cfg.override_fut_ws.trim().is_empty();
        if had_override {
            cfg.override_spot_rest.clear();
            cfg.override_spot_ws.clear();
            cfg.override_fut_rest.clear();
            cfg.override_fut_ws.clear();
            // 草稿同步清空：否则下次点"保存并重连"时 into_cfg 会把旧覆盖
            // 静默写回（此时已是实盘，清空守卫不再触发）
            app.settings.override_spot_rest.clear();
            app.settings.override_spot_ws.clear();
            app.settings.override_fut_rest.clear();
            app.settings.override_fut_ws.clear();
            state.push_toast(
                ToastKind::Warn,
                tr("已清空端点覆盖：测试网与实盘端点不通用").to_string(),
            );
        }
    }
    // 先更新内存再发落盘命令：引擎在另一线程出队时快照的是 state.cfg，
    // 顺序反了会把旧配置写回 config.toml
    state.update_cfg(cfg);
    state.send_cmd(Command::PersistConfig);
    // 内存已应用、重连已发起；磁盘写入由引擎侧反馈（成功不打扰、
    // 失败推错误 toast），此处不替落盘结果下结论
    state.push_toast(ToastKind::Ok, tr("设置已应用，正在重连…").to_string());
}

// ---------- 页面 ----------

pub fn show(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    // 外部变更回灌（保存重连后草稿重建、顶栏快捷切换同步进草稿）
    let fields = app.settings_fields.clone();
    fields.sync_all(app, cx);

    let state = Arc::clone(&app.state);
    let dir = config::data_dir();
    let t = cur();

    // ---------- 环境 ----------
    let testnet = app.settings.testnet;
    let env_pills = pills(
        cx,
        "env_pills",
        vec![
            ("testnet", tr("测试网 (Testnet)").to_string()),
            ("mainnet", tr("实盘").to_string()),
        ],
        if testnet { "testnet" } else { "mainnet" },
        |app: &mut BnApp, v: &str| app.settings.testnet = v == "testnet",
    );
    let mut env_body = div().flex().flex_col().gap(px(theme::SPACE_MD)).child(
        widgets::form_row()
            .child(env_pills)
            .child(div().flex_1())
            .child(widgets::badge(
                if testnet { "TESTNET" } else { tr("实盘 · 真实资金") },
                if testnet { t.accent } else { t.err },
            )),
    );
    if testnet {
        env_body = env_body.child(hint(
            "测试网密钥在 testnet.binance.vision / testnet.binancefuture.com 申请",
        ));
    }
    env_body = env_body.child(hint("测试网与实盘的 API 密钥不通用，切换环境后需重新配置密钥"));
    if !testnet {
        env_body = env_body.child(
            div()
                .text_size(font_md())
                .text_color(t.err)
                .child(tr("⚠ 实盘模式下下单将使用真实资金，请确认密钥与环境正确！")),
        );
    }
    let env_card = widgets::card_with_header(tr("交易环境"), None, env_body.into_any_element());

    // ---------- 代理 ----------
    let proxy_on = app.settings.proxy_enabled;
    let proxy_card = widgets::card_with_header(
        tr("网络代理（REST 与 WebSocket 均生效）"),
        None,
        div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_MD))
            .child(checkbox_row(
                cx,
                "set_proxy_en",
                proxy_on,
                "启用代理",
                |app: &mut BnApp, v: bool| app.settings.proxy_enabled = v,
            ))
            .child(
                widgets::form_row()
                    .child(widgets::row_label(tr("地址"), 68.0))
                    // 明文显示：便于核对代理主机与端口（凭据泄露风险已由
                    // ProxyConfig 的 Debug 脱敏与 config.toml 权限约束兜底）
                    .child(field_box(&app.settings_fields.proxy_url, 320.0)),
            )
            .into_any_element(),
    );

    // ---------- 外观与语言 ----------
    let lang_pills = pills(
        cx,
        "lang_pills",
        vec![("zh", "中文".to_string()), ("en", "English".to_string())],
        &app.settings.lang,
        |app: &mut BnApp, v: &str| app.settings.lang = v.to_string(),
    );
    // 字号：与涨跌色同路径，直写运行时配置并落盘（字号逐帧现算，无需重装字体）
    let font_pills = pills(
        cx,
        "font_pills",
        vec![
            ("small", tr("小").to_string()),
            ("medium", tr("中").to_string()),
            ("large", tr("大").to_string()),
        ],
        &app.settings.font_size,
        |app: &mut BnApp, v: &str| {
            app.settings.font_size = v.to_string();
            let mut cfg = app.state.cfg.read().clone();
            cfg.font_size = v.to_string();
            *app.state.cfg.write() = cfg;
            app.state.send_cmd(Command::PersistConfig);
        },
    );
    let theme_mode = app.settings.theme_mode.clone();
    let theme_pills = pills(
        cx,
        "theme_pills",
        vec![("dark", tr("深色").to_string()), ("light", tr("浅色").to_string())],
        &theme_mode,
        |app: &mut BnApp, v: &str| app.settings.theme_mode = v.to_string(),
    );
    // 涨跌方向：仅改全局色表（signed_color/up_color/down_color 即时翻转），
    // 不涉及字体/Style，无需重新 apply
    let dir_current = theme::PriceDir::from_cfg(&app.settings.price_dir);
    let price_pills = pills(
        cx,
        "price_pills",
        vec![
            ("green_up", theme::PriceDir::GreenUp.label().to_string()),
            ("red_up", theme::PriceDir::RedUp.label().to_string()),
        ],
        dir_current.as_cfg(),
        |app: &mut BnApp, v: &str| {
            app.settings.price_dir = v.to_string();
            theme::set_price_dir_str(v);
            // 同步进运行时配置：顶栏/盘口等直接读 theme::up_color 的区域立即生效
            let mut cfg = app.state.cfg.read().clone();
            cfg.price_dir = v.to_string();
            *app.state.cfg.write() = cfg;
            app.state.send_cmd(Command::PersistConfig);
        },
    );
    let look_body = div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .child(
            widgets::form_row()
                .child(widgets::row_label(tr("语言"), 68.0))
                .child(lang_pills),
        )
        .child(
            widgets::form_row()
                .child(widgets::row_label(tr("字体大小"), 68.0))
                .child(font_pills),
        )
        .child(
            widgets::form_row()
                .child(widgets::row_label(tr("主题"), 68.0))
                .child(theme_pills),
        )
        .child(
            widgets::form_row()
                .child(widgets::row_label(tr("涨跌色"), 68.0))
                .child(price_pills),
        )
        .child(hint("保存后生效；顶栏右侧 深色/浅色 与 中/EN 按钮可即时切换"));
    let look_card =
        widgets::card_with_header(tr("语言与主题"), None, look_body.into_any_element());

    // ---------- 参数 ----------
    let recv_hint_zh = crate::i18n::lang() == crate::i18n::ZH;
    let log_current = app.settings.log_level.clone();
    let log_pills = pills(
        cx,
        "log_pills",
        ["trace", "debug", "info", "warn", "error"]
            .iter()
            .map(|l| (*l, l.to_string()))
            .collect(),
        &log_current,
        |app: &mut BnApp, v: &str| app.settings.log_level = v.to_string(),
    );
    let iv_current = app.settings.default_interval.clone();
    let iv_options: Vec<(&'static str, String)> = crate::binance::Interval::all()
        .iter()
        .map(|iv| (iv.as_str(), iv.label().to_string()))
        .collect();
    let iv_pills = pills(
        cx,
        "iv_pills",
        iv_options,
        &iv_current,
        |app: &mut BnApp, v: &str| app.settings.default_interval = v.to_string(),
    );
    let ws_on = app.settings.ws_order;
    let ws_cb = checkbox_row(
        cx,
        "set_ws_order",
        ws_on,
        "WS 下单",
        |app: &mut BnApp, v: bool| app.settings.ws_order = v,
    )
    .tooltip(move |_, cx| {
        widgets::tooltip_view(
            tr("合约订单经 WebSocket 通道提交（延迟更低）；任何失败自动回退 REST").to_string(),
            cx,
        )
    });
    let lbl = |text: &'static str| -> Div {
        div()
            .text_size(font_body())
            .text_color(cur().text)
            .child(tr(text))
    };
    let param_body = div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        // 接收窗口：仅中文界面补英文小字说明
        .child(
            widgets::form_row()
                .child(lbl("接收窗口"))
                .when(recv_hint_zh, |r| {
                    r.child(
                        div()
                            .text_size(font_sm())
                            .text_color(cur().text_dim)
                            .child("recvWindow"),
                    )
                })
                .child(field_box(&app.settings_fields.recv_window, 72.0)),
        )
        .child(
            widgets::form_row()
                .child(lbl("HTTP 超时"))
                .child(field_box(&app.settings_fields.http_timeout, 64.0)),
        )
        .child(
            widgets::form_row()
                .child(lbl("日志级别"))
                .child(log_pills),
        )
        .child(
            widgets::form_row()
                .child(ws_cb),
        )
        .child(
            widgets::form_row()
                .child(lbl("默认交易对"))
                .child(field_box(&app.settings_fields.default_symbol, 110.0)),
        )
        .child(
            widgets::form_row()
                .child(lbl("默认K线周期"))
                .child(iv_pills),
        );
    let param_card =
        widgets::card_with_header(tr("请求参数"), None, param_body.into_any_element());

    // ---------- 端点覆盖 ----------
    let overridden = [
        &app.settings.override_spot_rest,
        &app.settings.override_spot_ws,
        &app.settings.override_fut_rest,
        &app.settings.override_fut_ws,
    ]
    .iter()
    .any(|s| !s.trim().is_empty());
    let ep_row = |label: &'static str, e: &Entity<TextField>| -> Div {
        widgets::form_row()
            .child(
                div()
                    .text_size(font_body())
                    .text_color(cur().text)
                    .child(tr(label)),
            )
            .child(field_box(e, 360.0))
    };
    let ep_body = div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .child(ep_row("现货 REST", &app.settings_fields.ov_spot_rest))
        .child(ep_row("现货 WS", &app.settings_fields.ov_spot_ws))
        .child(ep_row("合约 REST", &app.settings_fields.ov_fut_rest))
        .child(ep_row("合约 WS", &app.settings_fields.ov_fut_ws));
    // 自定义端点会把携带 API Key 的请求重定向到任意主机，必须显式可见
    let ep_right: Option<gpui::AnyElement> = if overridden {
        Some(
            widgets::badge(tr("自定义端点已启用"), cur().warn).into_any_element(),
        )
    } else {
        None
    };
    let ep_card = widgets::card_with_header(
        tr("端点覆盖（留空使用内置默认）"),
        ep_right,
        ep_body.into_any_element(),
    );

    // ---------- 应用 ----------
    // 主操作按钮：accent 填充（与解锁页主按钮一致），视觉层级高于普通按钮
    let save_btn = widgets::btn_primary("settings_save", tr("保存并重连"), true).on_click(
        cx.listener(|app: &mut BnApp, _, _, cx| {
            apply_settings(app);
            cx.notify();
        }),
    );

    // ---------- 密钥管理 ----------
    let has_creds = state.has_creds();
    let keys_body = div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .child(
            widgets::form_row()
                .child(widgets::badge(
                    if has_creds {
                        tr("已配置")
                    } else {
                        tr("未配置（仅公开行情）")
                    },
                    if has_creds { cur().up } else { cur().warn },
                ))
                .child(
                    widgets::btn_default(
                        "set_keys_edit",
                        if has_creds { tr("更换密钥") } else { tr("配置密钥") },
                        true,
                    )
                    .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                        app.keys_err.clear();
                        app.api_key.clear();
                        app.api_secret.clear();
                        app.api_key_field.update(cx, |f, _| f.set_text(""));
                        app.api_secret_field.update(cx, |f, _| f.set_text(""));
                        app.phase = Phase::EnterKeys;
                        cx.notify();
                    })),
                )
                .when(vault::vault_exists(dir), |row| {
                    // 破坏性操作两步确认：误点一次不会直接销毁唯一的本地密钥副本
                    let armed = app.vault_delete_armed;
                    row.child(
                        widgets::btn_danger(
                            "set_vault_del",
                            if armed {
                                tr("确认删除凭据库？")
                            } else {
                                tr("删除凭据库")
                            },
                            true,
                        )
                        .on_click(cx.listener(|app: &mut BnApp, _, _, cx| {
                            if !app.vault_delete_armed {
                                app.vault_delete_armed = true;
                            } else {
                                app.vault_delete_armed = false;
                                let dir = config::data_dir();
                                let gone = match std::fs::remove_file(dir.join(vault::VAULT_FILE))
                                {
                                    Ok(()) => true,
                                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                                    Err(_) => false,
                                };
                                if gone {
                                    app.state.set_creds(None);
                                    app.state.push_toast(
                                        ToastKind::Info,
                                        tr("已删除本机凭据库").to_string(),
                                    );
                                    app.phase = Phase::EnterKeys;
                                } else {
                                    app.state.push_toast(
                                        ToastKind::Err,
                                        tr("删除凭据库失败").to_string(),
                                    );
                                }
                            }
                            cx.notify();
                        })),
                    )
                }),
        );
    let keys_card = widgets::card_with_header(tr("API 密钥"), None, keys_body.into_any_element());

    // ---------- 关于 ----------
    let about_body = div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .child(
            div()
                .text_size(font_sm())
                .text_color(cur().text_dim)
                .child(format!(
                    "BnTradingPro v{} · {} {}",
                    env!("CARGO_PKG_VERSION"),
                    tr("数据目录"),
                    dir.display()
                )),
        )
        .child(
            div()
                .text_size(font_sm())
                .text_color(cur().text_dim)
                .child(tr(
                    "本工具仅供个人交易辅助与研究，不构成任何投资建议；使用实盘交易产生的一切后果由使用者自行承担。",
                )),
        );
    let about_card = widgets::card_with_header(tr("关于"), None, about_body.into_any_element());

    // ---------- 组页 ----------
    let root = div()
        .id("settings_page")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .child(
            div()
                .px(px(theme::SPACE_XL))
                .py(px(theme::SPACE_LG))
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_LG))
                .child(
                    div()
                        .text_size(font_lg())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cur().text)
                        .child(tr("设置")),
                )
                .child(widgets::separator())
                .child(env_card)
                .child(proxy_card)
                .child(look_card)
                .child(param_card)
                .child(ep_card)
                .child(save_btn)
                .child(keys_card)
                .child(about_card),
        );

    // 外层再包一层弹性容器（root 带 id/滚动为 Stateful<Div>，页面统一返回 Div）
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .child(root)
}
