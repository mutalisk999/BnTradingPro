//! 启动流程：已有凭据库 → 解锁；无凭据库 → 配置 API 密钥（与主密码一体设置）
//!
//! 设计：深色背景 + 品牌光晕 + 居中认证卡片，主按钮金色。

use crate::app::{BnApp, Phase};
use crate::config;
use crate::engine::state::ToastKind;
use crate::security::vault::{self, Creds};
use crate::ui::theme;
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Stroke};

pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    let mut card_h_measured = 0.0_f32;
    // ---------- 背景 + 居中卡片同在一个 CentralPanel 内 ----------
    // 旧实现把卡片放在独立 Area 里用锚定居中：Area 有 sizing pass，两趟算出的
    // 卡片高度不一致，每帧锚点位置都在跳，视觉上像在"滚动"。
    // 改为在同一面板内直接布局：位置由单趟布局确定，无二次猜测。
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::cur().bg))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let p = ui.painter();
            // 光晕颜色随主题派生（accent/up），避免主题调整后此处残留旧色值
            let acc = theme::cur().accent;
            let up = theme::cur().up;
            p.circle_filled(
                Pos2::new(
                    rect.left() + rect.width() * 0.18,
                    rect.top() + rect.height() * 0.12,
                ),
                rect.height() * 0.42,
                Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), 10),
            );
            p.circle_filled(
                Pos2::new(
                    rect.right() - rect.width() * 0.15,
                    rect.bottom() - rect.height() * 0.08,
                ),
                rect.height() * 0.38,
                Color32::from_rgba_unmultiplied(up.r(), up.g(), up.b(), 8),
            );

            let avail = ui.max_rect();
            let card_w = 440.0;

            // 品牌标题固定在窗口上部：窗口再矮也不会被卡片遮挡或推出屏幕
            let title_y = avail.top() + 26.0;
            p.text(
                Pos2::new(avail.center().x, title_y),
                Align2::CENTER_CENTER,
                "BnTradingPro",
                FontId::proportional(34.0),
                theme::cur().accent,
            );
            p.text(
                Pos2::new(avail.center().x, title_y + 36.0),
                Align2::CENTER_CENTER,
                crate::i18n::tr("Binance 现货 / U本位合约 交易终端"),
                FontId::proportional(13.5),
                theme::cur().text_dim,
            );

            // 卡片垂直位置：以上一帧实测高度在标题以下区域居中；空间不足时
            // 顶到标题下方，底边永不越过窗口下边缘，超高部分由内部 ScrollArea
            // 滚动兜底。高度取上一帧测量值（滞后一帧收敛），静止后完全稳定。
            let region_top = title_y + 58.0;
            let card_top = (avail.center().y - app.auth_card_h * 0.5)
                .max(region_top)
                .min((avail.bottom() - 60.0).max(region_top));
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                    Pos2::new(avail.center().x - card_w / 2.0, card_top),
                    egui::vec2(card_w, (avail.bottom() - card_top).max(60.0)),
                )),
                |ui| {
                    // 仅当上一帧实测高度在当前区域放不下时才启用内部滚动（滚轮才生效）：
                    // 0.36 的 ScrollArea 带平滑惯性，常态下误滚滚轮会把表单滑出视野
                    // （用户报告的"滚动效果"）；放得下时对滚轮完全无反应。
                    let frame_h = if app.auth_card_h > ui.available_height() - 8.0 {
                        egui::ScrollArea::vertical()
                            .id_salt("auth_card_scroll")
                            .auto_shrink([false, true])
                            .show(ui, |ui| card_frame(app, ui, card_w))
                            .inner
                    } else {
                        card_frame(app, ui, card_w)
                    };
                    card_h_measured = frame_h;
                },
            );
        });
    if card_h_measured > 0.0 {
        app.auth_card_h = card_h_measured;
    }
    draw_env_hint(ui.ctx(), app);
}

fn card_header(ui: &mut egui::Ui, title: &str) {
    // 页面标题统一 16.5（egui heading 默认 19px 与其他页不一致）
    ui.label(egui::RichText::new(title).strong().size(16.5));
    ui.separator();
}

/// 认证卡片（Frame 包裹的标题+表单），返回外框实际高度供下一帧定位用
fn card_frame(app: &mut BnApp, ui: &mut egui::Ui, card_w: f32) -> f32 {
    egui::Frame::default()
        .fill(theme::cur().card)
        .stroke(Stroke::new(1.0_f32, theme::cur().border))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(26))
        .show(ui, |ui| {
            ui.set_width(card_w - 52.0);
            match app.phase {
                Phase::Unlock => card_header(ui, crate::i18n::tr("解锁")),
                Phase::EnterKeys => card_header(ui, crate::i18n::tr("配置 API 密钥")),
                Phase::Main => {}
            }
            ui.add_space(6.0);
            match app.phase {
                Phase::Unlock => unlock(app, ui),
                Phase::EnterKeys => enter_keys(app, ui),
                Phase::Main => {}
            }
        })
        .response
        .rect
        .height()
}

fn draw_env_hint(ctx: &egui::Context, app: &BnApp) {
    let testnet = app.state.cfg.read().testnet;
    let (txt, color) = if testnet {
        ("TESTNET", theme::cur().accent)
    } else {
        (crate::i18n::tr("实盘环境 · 真实资金"), theme::cur().err)
    };
    egui::Area::new(egui::Id::new("unlock_env"))
        .anchor(Align2::RIGHT_TOP, [-14.0, 14.0])
        .show(ctx, |ui| {
            theme::badge(ui, txt, color);
        });
}

fn error_text(ui: &mut egui::Ui, msg: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new("✗")
                .color(theme::cur().err)
                .strong()
                .size(12.5),
        );
        ui.label(egui::RichText::new(msg).color(theme::cur().err).size(12.5));
    });
}

/// 主按钮（全宽、金色底深色字）
fn primary_btn(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(text)
            .strong()
            .size(14.5)
            .color(theme::cur().on_accent),
    )
    .fill(theme::cur().accent)
    .corner_radius(5)
    .min_size(egui::vec2(ui.available_width(), 34.0));
    ui.add_enabled(enabled, btn).clicked()
}

/// 解锁已有凭据库
fn unlock(app: &mut BnApp, ui: &mut egui::Ui) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(crate::i18n::tr("输入主密码以解密本机保存的 API 密钥"))
                .size(12.0)
                .color(theme::cur().text_dim),
        )
        .wrap_mode(egui::TextWrapMode::Wrap),
    );
    ui.add_space(10.0);
    ui.label(theme::card_label(crate::i18n::tr("主密码")));
    let pw_resp = ui.add(
        egui::TextEdit::singleline(&mut *app.pw1)
            .password(true)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    // 仅在当前没有任何焦点控件时请求一次：每帧强制抢占会让用户点不进其他输入框
    if ui.ctx().memory(|m| m.focused().is_none()) {
        pw_resp.request_focus();
    }
    if pw_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        try_unlock(app);
    }
    if !app.pw_err.is_empty() {
        ui.add_space(2.0);
        error_text(ui, &app.pw_err);
    }
    ui.add_space(12.0);
    if primary_btn(ui, crate::i18n::tr("解锁"), true) {
        try_unlock(app);
    }
    ui.add_space(2.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let dir = config::data_dir();
        // 破坏性操作两步确认：误点一次不会直接销毁唯一的本地密钥副本
        let armed = app.vault_delete_armed;
        let reset_btn = ui.small_button(
            egui::RichText::new(if armed {
                crate::i18n::tr("确认删除凭据库？")
            } else {
                crate::i18n::tr("重置凭据库")
            })
            .size(11.5)
            .color(if armed {
                theme::cur().err
            } else {
                theme::cur().warn
            }),
        );
        if reset_btn.clicked() {
            if !armed {
                app.vault_delete_armed = true;
            } else {
                app.vault_delete_armed = false;
                let gone = match std::fs::remove_file(dir.join(vault::VAULT_FILE)) {
                    Ok(()) => true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                    Err(_) => false,
                };
                if gone {
                    app.pw1.clear();
                    app.pw2.clear();
                    app.pw_err.clear();
                    app.phase = Phase::EnterKeys;
                } else {
                    app.pw_err = crate::i18n::tr("删除凭据库失败").to_string();
                }
            }
        } else if armed && ui.input(|i| i.pointer.any_click()) {
            // 点击确认按钮以外的任意位置 → 退出待确认态
            app.vault_delete_armed = false;
        }
        reset_btn.on_hover_text(crate::i18n::tr("删除本机加密凭据库，重新开始设置"));
        if ui
            .small_button(
                egui::RichText::new(crate::i18n::tr("仅看行情，暂不解锁"))
                    .size(11.5)
                    .color(theme::cur().text_dim),
            )
            .clicked()
        {
            app.vault_delete_armed = false;
            app.phase = Phase::Main;
        }
    });
}

fn try_unlock(app: &mut BnApp) {
    let dir = config::data_dir();
    match vault::load_creds(&dir, &app.pw1) {
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

/// 配置 API 密钥（首次/重置/更换）。
/// 无凭据库时主密码与密钥一体设置；已有凭据库时需先输入现有主密码。
fn enter_keys(app: &mut BnApp, ui: &mut egui::Ui) {
    let dir = config::data_dir();
    let vault_exists = vault::vault_exists(&dir);
    let testnet = app.state.cfg.read().testnet;
    ui.add(
        egui::Label::new(
            egui::RichText::new(if testnet {
                crate::i18n::tr("当前环境：测试网（testnet.binance.vision / testnet.binancefuture.com 申请）")
            } else {
                crate::i18n::tr("当前环境：实盘（binance.com → API 管理，建议绑定 IP 白名单并只开启现货/合约交易权限）")
            })
            .size(12.0)
            .color(if testnet { theme::cur().accent } else { theme::cur().err }),
        )
        .wrap_mode(egui::TextWrapMode::Wrap),
    );
    ui.add_space(10.0);

    // 主密码：一体设置（无库）或验证（有库）
    let need_master = vault_exists || app.master.is_none();
    if need_master {
        if vault_exists {
            // 已有凭据库时也必须能输入主密码（用于验证），否则 can_save 永远为
            // false，"更换密钥"流程会困死在本页
            ui.label(theme::card_label(crate::i18n::tr("主密码（验证现有主密码）")));
            let pw_resp = ui.add(
                egui::TextEdit::singleline(&mut *app.pw1)
                    .password(true)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            if ui.ctx().memory(|m| m.focused().is_none()) {
                pw_resp.request_focus();
            }
            ui.add_space(6.0);
        } else {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(crate::i18n::tr("主密码用于加密本机保存的 API Key/Secret（Argon2id + AES-256-GCM）。忘记将无法恢复。"))
                        .size(12.0)
                        .color(theme::cur().text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(4.0);
            ui.label(theme::card_label(crate::i18n::tr("主密码（至少 8 位）")));
            ui.add(
                egui::TextEdit::singleline(&mut *app.pw1)
                    .password(true)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            ui.add_space(4.0);
            ui.label(theme::card_label(crate::i18n::tr("再输一次")));
            ui.add(
                egui::TextEdit::singleline(&mut *app.pw2)
                    .password(true)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            ui.add_space(6.0);
        }
    }

    ui.label(theme::card_label("API Key"));
    // API Key 同样遮蔽：仅凭 Key 即可创建 listenKey 读取成交/余额推送，
    // 不能在录屏/共享屏幕时明文暴露
    ui.add(
        egui::TextEdit::singleline(&mut *app.api_key)
            .password(true)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .text_color(theme::cur().text),
    );
    ui.add_space(6.0);
    ui.label(theme::card_label("API Secret"));
    ui.add(
        egui::TextEdit::singleline(&mut *app.api_secret)
            .password(true)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .text_color(theme::cur().text),
    );
    if !app.keys_err.is_empty() {
        ui.add_space(2.0);
        error_text(ui, &app.keys_err);
    }
    ui.add_space(12.0);

    let can_save = !app.api_key.trim().is_empty()
        && !app.api_secret.trim().is_empty()
        && (!need_master || !app.pw1.trim().is_empty());
    let mut do_save = false;
    if primary_btn(ui, crate::i18n::tr("保存并连接"), can_save) {
        do_save = true;
    }
    if do_save {
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
                    match vault::load_creds(&dir, &pw) {
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
                match vault::save_creds(&dir, &pw, &creds) {
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
    ui.add_space(2.0);
    // 跳过入口两种分支都要有：换钥流程（已有库）也需要能随时退出本页，
    // 否则用户点开"更换密钥"后无路可退
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui
            .small_button(
                egui::RichText::new(crate::i18n::tr("跳过，仅看行情"))
                    .size(11.5)
                    .color(theme::cur().text_dim),
            )
            .clicked()
        {
            app.api_key.clear();
            app.api_secret.clear();
            app.pw1.clear();
            app.pw2.clear();
            app.phase = Phase::Main;
        }
    });
    ui.add(
        egui::Label::new(
            egui::RichText::new(crate::i18n::tr(
                "跳过后仍可查看实时行情与 K 线，随时可在设置页配置密钥",
            ))
            .size(11.0)
            .color(theme::cur().text_dim.gamma_multiply(0.85)),
        )
        .wrap_mode(egui::TextWrapMode::Wrap),
    );
}
