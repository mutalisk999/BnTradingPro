//! 启动流程页面：已有凭据库 → 解锁；无凭据库 → 配置 API 密钥（与主密码一体设置）
//!
//! 设计：背景光晕 + 品牌标题 + 居中认证卡片，主按钮金色。
//! 纯逻辑 try_unlock / save_keys 与 egui 版原样一致；渲染为 GPUI flex 构建器。

use crate::app::{BnApp, Phase};
use crate::config;
use crate::engine::state::ToastKind;
use crate::security::vault::{self, Creds};
use crate::ui::theme::{self, cur, faded, font_body, font_brand, font_lg, font_md, font_sm};
use crate::ui::widgets;
use gpui::{
    div, prelude::FluentBuilder, px, Bounds, ClickEvent, Context, Corner, Div, Edges, FontWeight,
    Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, StatefulInteractiveElement,
    Styled, Window,
};
use zeroize::Zeroizing;

/// 解锁已有凭据库：成功 → Main；库不存在 → EnterKeys；密码错误 → pw_err
pub fn try_unlock(app: &mut BnApp) {
    let dir = config::data_dir();
    match vault::load_creds(dir, &app.pw1) {
        Ok(creds) => {
            app.state.set_creds(Some(creds));
            app.pw1.clear();
            app.pw_err.clear();
            app.state
                .push_toast(ToastKind::Ok, crate::i18n::tr("已解锁，开始同步账户数据"));
            app.phase = Phase::Main;
        }
        Err(vault::VaultError::NotFound) => {
            app.phase = Phase::EnterKeys;
        }
        Err(_) => {
            app.pw_err = crate::i18n::tr("主密码错误，或凭据文件损坏").to_string();
        }
    }
}

/// 校验主密码并加密保存密钥，成功后进入主界面（保存按钮与 Secret 回车共用）
pub fn save_keys(app: &mut BnApp, dir: &std::path::Path, vault_exists: bool) {
    let creds = Creds {
        api_key: app.api_key.trim().to_string(),
        api_secret: app.api_secret.trim().to_string(),
    };
    let master = app.master.clone().or_else(|| {
        let m = app.pw1.clone();
        if m.is_empty() {
            None
        } else {
            Some(m)
        }
    });
    match master {
        Some(pw) => {
            // 已有凭据库：先校验主密码
            if vault_exists {
                match vault::load_creds(dir, &pw) {
                    Err(vault::VaultError::NotFound) => {}
                    Err(_) => {
                        app.keys_err = crate::i18n::tr("主密码错误").to_string();
                        return;
                    }
                    Ok(_) => {}
                }
            } else if pw.len() < 8 {
                app.keys_err = crate::i18n::tr("主密码至少 8 位").to_string();
                return;
            } else if pw != app.pw2 {
                app.keys_err = crate::i18n::tr("两次输入不一致").to_string();
                return;
            }
            match vault::save_creds(dir, &pw, &creds) {
                Ok(()) => {
                    app.state.set_creds(Some(creds));
                    app.api_key.clear();
                    app.api_secret.clear();
                    app.keys_err.clear();
                    app.master = None;
                    app.pw1.clear();
                    app.pw2.clear();
                    app.state
                        .push_toast(ToastKind::Ok, crate::i18n::tr("密钥已加密保存并连接"));
                    app.phase = Phase::Main;
                }
                Err(e) => app.keys_err = format!("{}{e}", crate::i18n::tr("保存失败: ")),
            }
        }
        None => {
            app.keys_err = crate::i18n::tr("请输入主密码").to_string();
        }
    }
}

// ---------- 页面渲染 ----------

/// 认证页：背景光晕（canvas）+ 品牌标题 + 居中卡片 + 右上角环境徽标。
/// 矮窗口下卡片纵向溢出时由 overflow 遮挡裁剪（既有取舍：遮挡优于顶歪）
pub fn page(app: &mut BnApp, window: &mut Window, cx: &mut Context<BnApp>) -> Div {
    let t = *theme::c(cx);
    // 自动聚焦：当前无任何焦点控件时聚焦主密码框（egui 版同款——每帧强制
    // 抢占会让用户点不进其他输入框，仅在无焦点时请求一次）
    let has_focus = window.focused(cx).is_some();
    let vault_exists = vault::vault_exists(config::data_dir());
    let need_master = vault_exists || app.master.is_none();
    let testnet = app.state.cfg.read().testnet;
    if !has_focus {
        match app.phase {
            Phase::Unlock => app.pw1_field.read(cx).focus(window),
            Phase::EnterKeys if need_master => app.pw1_field.read(cx).focus(window),
            Phase::EnterKeys => app.api_key_field.read(cx).focus(window),
            Phase::Main => {}
        }
    }

    div()
        .relative()
        .flex_1()
        .flex()
        .flex_col()
        .bg(t.bg)
        .overflow_hidden()
        // 「点击其他位置解除」待确认态的兜底：点按钮以外的任意处先解除武装
        // （按钮自身 on_mouse_down 停止传播，武装/确认由其 on_click 完成）
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|app: &mut BnApp, _, _, cx| {
                if app.vault_delete_armed {
                    app.vault_delete_armed = false;
                    cx.notify();
                }
            }),
        )
        // 背景光晕：canvas 在整页 bounds 上画两枚大半透明圆
        .child(glow_canvas())
        // 右上角环境徽标（TESTNET / 实盘）
        .child(env_hint(testnet))
        // 品牌标题固定在窗口上部：窗口再矮也不会被卡片遮挡或推出屏幕
        .child(
            div()
                .relative()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(26.0))
                .child(
                    div()
                        .text_size(font_brand())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.accent)
                        .child("BnTradingPro"),
                )
                .child(
                    div()
                        .mt(px(theme::SPACE_MD))
                        .text_size(font_body())
                        .text_color(t.text_dim)
                        .child(crate::i18n::tr("Binance 现货 / U本位合约 交易终端")),
                )
                // 卡片区域 = 标题以下（flex_1 剩余空间），内容纵向居中——
                // 不能按整窗居中：标题占掉顶部后整窗居中会让卡片偏上
                .child(
                    div()
                        .flex_1()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .px(px(theme::SPACE_XL))
                        .child(card(app, cx)),
                ),
        )
}

/// 背景光晕：accent（左上）与 up（右下）两枚大半透明圆，
/// 颜色随主题派生，避免主题调整后残留旧色值
fn glow_canvas() -> impl IntoElement {
    gpui::canvas(
        |bounds, _, _| bounds,
        move |bounds, _, window, _| {
            let t = cur();
            let h = bounds.size.height;
            let mut circle = |center: gpui::Point<gpui::Pixels>, radius: gpui::Pixels, color| {
                let size = radius * 2.0;
                window.paint_quad(gpui::quad(
                    Bounds::from_corner_and_size(
                        Corner::TopLeft,
                        gpui::point(center.x - radius, center.y - radius),
                        gpui::size(size, size),
                    ),
                    radius,
                    color,
                    Edges::all(px(0.0)),
                    gpui::transparent_black(),
                    gpui::BorderStyle::Solid,
                ));
            };
            circle(
                gpui::point(
                    bounds.origin.x + bounds.size.width * 0.18,
                    bounds.origin.y + h * 0.12,
                ),
                h * 0.42,
                faded(t.accent, 10.0 / 255.0),
            );
            circle(
                gpui::point(
                    bounds.origin.x + bounds.size.width - bounds.size.width * 0.15,
                    bounds.origin.y + h - h * 0.08,
                ),
                h * 0.38,
                faded(t.up, 8.0 / 255.0),
            );
        },
    )
}

/// 右上角环境徽标：TESTNET（accent）/ 实盘环境 · 真实资金（err）
fn env_hint(testnet: bool) -> impl IntoElement {
    let (txt, color) = if testnet {
        ("TESTNET".to_string(), cur().accent)
    } else {
        (
            crate::i18n::tr("实盘环境 · 真实资金").to_string(),
            cur().err,
        )
    };
    div()
        .absolute()
        .top(px(theme::SPACE_XXL))
        .right(px(theme::SPACE_XXL))
        .child(widgets::badge(txt, color))
}

/// 认证卡片（440px：标题行 + 分隔线 + 按阶段的表单）
fn card(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let t = *theme::c(cx);
    let title = match app.phase {
        Phase::Unlock => crate::i18n::tr("解锁").to_string(),
        Phase::EnterKeys => crate::i18n::tr("配置 API 密钥").to_string(),
        Phase::Main => String::new(),
    };
    let card = widgets::card()
        .w(px(440.0))
        .px(px(theme::SPACE_XXL))
        .py(px(theme::SPACE_XL))
        .my(px(0.0))
        .border_color(t.border)
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(font_lg())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(title),
        )
        .child(widgets::separator());
    match app.phase {
        Phase::Unlock => unlock_form(app, cx),
        Phase::EnterKeys => enter_keys_form(app, cx),
        Phase::Main => card,
    }
}

/// 错误提示行：圆形图标底 + 实心圆点 + 红字（✗ 字形在等宽字体优先的
/// 字体栈下有缺字回退成豆腐块的风险，与 toast 同款画法）
fn error_text(msg: &str) -> Div {
    let t = cur();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_MD))
        .child(
            div()
                .w(px(16.0))
                .h(px(16.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(faded(t.err, 0.2))
                .child(
                    div()
                        .w(px(7.0))
                        .h(px(7.0))
                        .rounded_full()
                        .bg(Hsla::from(t.err)),
                ),
        )
        .child(
            div()
                .flex_1()
                .text_size(font_md())
                .text_color(t.err)
                .child(msg.to_string()),
        )
}

/// 解锁表单：说明 + 主密码 + 错误 + 解锁/仅看行情/重置凭据库
fn unlock_form(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let t = *theme::c(cx);
    div()
        .flex()
        .flex_col()
        .mt(px(theme::SPACE_XS))
        .gap(px(theme::SPACE_MD))
        .child(
            div()
                .text_size(font_md())
                .text_color(t.text_dim)
                .child(crate::i18n::tr("输入主密码以解密本机保存的 API 密钥").to_string()),
        )
        .child(widgets::card_label(crate::i18n::tr("主密码")))
        .child(app.pw1_field.clone())
        .when(!app.pw_err.is_empty(), |d| d.child(error_text(&app.pw_err)))
        .child(
            widgets::btn_primary("unlock_go", crate::i18n::tr("解锁"), true).on_click(
                cx.listener(|app: &mut BnApp, _, _, cx| {
                    app.pw1 = Zeroizing::new(app.pw1_field.read(cx).text().to_string());
                    try_unlock(app);
                    cx.notify();
                }),
            ),
        )
        .child(
            widgets::btn_secondary("unlock_skip", crate::i18n::tr("仅看行情，暂不解锁"), true)
                .on_click(cx.listener(|app: &mut BnApp, _, window, cx| {
                    app.vault_delete_armed = false;
                    app.phase = Phase::Main;
                    window.blur();
                    cx.notify();
                })),
        )
        .child(reset_row(app, cx))
}

/// 重置凭据库行（右对齐破坏性按钮 + 两步确认；武装态文案随 app 状态翻转）
fn reset_row(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let armed = app.vault_delete_armed;
    div().flex().flex_row().justify_end().child(
        widgets::btn_danger(
            "vault_reset",
            if armed {
                crate::i18n::tr("确认删除凭据库？")
            } else {
                crate::i18n::tr("重置凭据库")
            },
            true,
        )
        // 阻断页级「点击其他位置解除」兜底：按钮自身的 on_click
        // 负责武装/确认翻转
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_click(cx.listener(|app: &mut BnApp, _: &ClickEvent, _, cx| {
            if !app.vault_delete_armed {
                app.vault_delete_armed = true;
            } else {
                app.vault_delete_armed = false;
                let gone = match std::fs::remove_file(config::data_dir().join(vault::VAULT_FILE))
                {
                    Ok(()) => true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                    Err(_) => false,
                };
                if gone {
                    app.pw1_field.update(cx, |f, _| f.clear());
                    app.pw2_field.update(cx, |f, _| f.clear());
                    app.pw1.clear();
                    app.pw2.clear();
                    app.pw_err.clear();
                    app.phase = Phase::EnterKeys;
                } else {
                    app.pw_err = crate::i18n::tr("删除凭据库失败").to_string();
                }
            }
            cx.notify();
        })),
    )
}

/// 配钥表单：环境提示 + 主密码（一体设置/验证）+ API Key/Secret + 保存/跳过
fn enter_keys_form(app: &mut BnApp, cx: &mut Context<BnApp>) -> Div {
    let t = *theme::c(cx);
    let vault_exists = vault::vault_exists(config::data_dir());
    let testnet = app.state.cfg.read().testnet;
    let need_master = vault_exists || app.master.is_none();

    let mut col = div().flex().flex_col().mt(px(theme::SPACE_XS)).gap(px(theme::SPACE_MD));
    col = col.child(
        div()
            .text_size(font_md())
            .text_color(if testnet { t.accent } else { t.err })
            .child(
                if testnet {
                    crate::i18n::tr("当前环境：测试网（testnet.binance.vision / testnet.binancefuture.com 申请）")
                } else {
                    crate::i18n::tr("当前环境：实盘（binance.com → API 管理，建议绑定 IP 白名单并只开启现货/合约交易权限）")
                }
                .to_string(),
            ),
    );

    // 主密码：一体设置（无库）或验证（有库）
    if need_master {
        if vault_exists {
            // 已有凭据库时也必须能输入主密码（用于验证），否则 can_save 永远为
            // false，"更换密钥"流程会困死在本页
            col = col
                .child(widgets::card_label(crate::i18n::tr(
                    "主密码（验证现有主密码）",
                )))
                .child(app.pw1_field.clone());
        } else {
            col = col
                .child(
                    div()
                        .text_size(font_md())
                        .text_color(t.text_dim)
                        .child(
                            crate::i18n::tr("主密码用于加密本机保存的 API Key/Secret（Argon2id + AES-256-GCM）。忘记将无法恢复。")
                                .to_string(),
                        ),
                )
                // 两个密码框并排一行：省掉一整行高度，卡片更紧凑
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(theme::SPACE_LG))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(theme::SPACE_XS))
                                .child(widgets::card_label(crate::i18n::tr(
                                    "主密码（至少 8 位）",
                                )))
                                .child(app.pw1_field.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(theme::SPACE_XS))
                                .child(widgets::card_label(crate::i18n::tr("再输一次")))
                                .child(app.pw2_field.clone()),
                        ),
                );
        }
    }

    let can_save = app.can_save_keys(cx);
    col = col
        .child(widgets::card_label("API Key"))
        .child(app.api_key_field.clone())
        .child(widgets::card_label("API Secret"))
        .child(app.api_secret_field.clone())
        .when(!app.keys_err.is_empty(), |d| {
            d.child(error_text(&app.keys_err))
        })
        .child(
            widgets::btn_primary("keys_save", crate::i18n::tr("保存并连接"), can_save).on_click(
                cx.listener(|app: &mut BnApp, _, window, cx| {
                    app.sync_auth_fields(cx);
                    let can_save = app.can_save_keys(cx);
                    if can_save {
                        save_keys(
                            app,
                            config::data_dir(),
                            vault::vault_exists(config::data_dir()),
                        );
                    }
                    if app.phase == Phase::Main {
                        window.blur();
                    }
                    cx.notify();
                }),
            ),
        );

    // 跳过入口两种分支都要有：换钥流程（已有库）也需要能随时退出本页，
    // 否则用户点开"更换密钥"后无路可退
    col = col
        .child(
            div()
                .w_full()
                .h(px(1.0))
                .my(px(theme::SPACE_SM))
                .bg(faded(t.border, 0.7)),
        )
        .child(
            div()
                .text_size(font_sm())
                .text_color(t.text_dim)
                .child(
                    crate::i18n::tr("跳过后仍可查看实时行情与 K 线，随时可在设置页配置密钥")
                        .to_string(),
                ),
        )
        .child(
            widgets::btn_secondary("keys_skip", crate::i18n::tr("跳过，仅看行情"), true)
                .on_click(cx.listener(|app: &mut BnApp, _, window, cx| {
                    app.pw1_field.update(cx, |f, _| f.clear());
                    app.pw2_field.update(cx, |f, _| f.clear());
                    app.api_key_field.update(cx, |f, _| f.clear());
                    app.api_secret_field.update(cx, |f, _| f.clear());
                    app.api_key.clear();
                    app.api_secret.clear();
                    app.pw1.clear();
                    app.pw2.clear();
                    app.phase = Phase::Main;
                    window.blur();
                    cx.notify();
                })),
        );
    col
}
