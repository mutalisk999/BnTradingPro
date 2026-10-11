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

pub fn show(app: &mut BnApp, ctx: &egui::Context) {
    // ---------- 背景：深色 + 品牌光晕 ----------
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::cur().bg))
        .show(ctx, |ui| {
            let rect = ui.max_rect();
            let p = ui.painter();
            p.circle_filled(
                Pos2::new(
                    rect.left() + rect.width() * 0.18,
                    rect.top() + rect.height() * 0.12,
                ),
                rect.height() * 0.42,
                Color32::from_rgba_unmultiplied(240, 185, 11, 10),
            );
            p.circle_filled(
                Pos2::new(
                    rect.right() - rect.width() * 0.15,
                    rect.bottom() - rect.height() * 0.08,
                ),
                rect.height() * 0.38,
                Color32::from_rgba_unmultiplied(46, 203, 129, 8),
            );
            let cx = rect.center().x;
            let cy = rect.center().y;
            p.text(
                Pos2::new(cx, cy - 196.0),
                Align2::CENTER_CENTER,
                "BnTradingPro",
                FontId::proportional(34.0),
                theme::cur().accent,
            );
            p.text(
                Pos2::new(cx, cy - 164.0),
                Align2::CENTER_CENTER,
                crate::i18n::tr("Binance 现货 / U本位合约 交易终端"),
                FontId::proportional(13.5),
                theme::cur().text_dim,
            );
        });

    // ---------- 居中认证卡片 ----------
    egui::Area::new(egui::Id::new("auth_card"))
        .anchor(Align2::CENTER_CENTER, [0.0, 26.0])
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(theme::cur().card)
                .stroke(Stroke::new(1.0_f32, theme::cur().border))
                .rounding(8.0)
                .inner_margin(egui::Margin::same(26.0))
                .show(ui, |ui| {
                    ui.set_min_width(400.0);
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
                });
        });
    draw_env_hint(ctx, app);
}

fn card_header(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).heading());
    ui.separator();
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
            .color(Color32::from_rgb(28, 22, 2)),
    )
    .fill(theme::cur().accent)
    .rounding(5.0)
    .min_size(egui::vec2(ui.available_width(), 34.0));
    ui.add_enabled(enabled, btn).clicked()
}

/// 解锁已有凭据库
fn unlock(app: &mut BnApp, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(crate::i18n::tr("输入主密码以解密本机保存的 API 密钥"))
            .size(12.0)
            .color(theme::cur().text_dim),
    );
    ui.add_space(10.0);
    ui.label(theme::card_label(crate::i18n::tr("主密码")));
    let pw_resp = ui.add(
        egui::TextEdit::singleline(&mut app.pw1)
            .password(true)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
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
        if ui
            .small_button(
                egui::RichText::new(crate::i18n::tr("重置凭据库"))
                    .size(11.5)
                    .color(theme::cur().warn),
            )
            .on_hover_text(crate::i18n::tr("删除本机加密凭据库，重新开始设置"))
            .clicked()
        {
            let _ = std::fs::remove_file(dir.join(vault::VAULT_FILE));
            app.phase = Phase::EnterKeys;
        }
        if ui
            .small_button(
                egui::RichText::new(crate::i18n::tr("仅看行情，暂不解锁"))
                    .size(11.5)
                    .color(theme::cur().text_dim),
            )
            .clicked()
        {
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
    ui.label(
        egui::RichText::new(if testnet {
            crate::i18n::tr("当前环境：测试网（testnet.binance.vision / testnet.binancefuture.com 申请）")
        } else {
            crate::i18n::tr("当前环境：实盘（binance.com → API 管理，建议绑定 IP 白名单并只开启现货/合约交易权限）")
        })
        .size(12.0)
        .color(if testnet { theme::cur().accent } else { theme::cur().err }),
    );
    ui.add_space(10.0);

    // 主密码：一体设置（无库）或验证（有库）
    let need_master = vault_exists || app.master.is_none();
    if need_master {
        if vault_exists {
            ui.label(theme::card_label(crate::i18n::tr("主密码（用于加密保存）")));
        } else {
            ui.label(
                egui::RichText::new(crate::i18n::tr("主密码用于加密本机保存的 API Key/Secret（Argon2id + AES-256-GCM）。忘记将无法恢复。"))
                    .size(12.0)
                    .color(theme::cur().text_dim),
            );
            ui.add_space(4.0);
            ui.label(theme::card_label(crate::i18n::tr("主密码（至少 8 位）")));
            ui.add(
                egui::TextEdit::singleline(&mut app.pw1)
                    .password(true)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            ui.add_space(4.0);
            ui.label(theme::card_label(crate::i18n::tr("再输一次")));
            ui.add(
                egui::TextEdit::singleline(&mut app.pw2)
                    .password(true)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            ui.add_space(6.0);
        }
    }

    ui.label(theme::card_label("API Key"));
    ui.add(
        egui::TextEdit::singleline(&mut app.api_key)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .text_color(theme::cur().text),
    );
    ui.add_space(6.0);
    ui.label(theme::card_label("API Secret"));
    ui.add(
        egui::TextEdit::singleline(&mut app.api_secret)
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
    if !vault_exists {
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
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "跳过后仍可查看实时行情与 K 线，随时可在设置页配置密钥",
            ))
            .size(11.0)
            .color(theme::cur().text_dim.gamma_multiply(0.85)),
        );
    }
}
