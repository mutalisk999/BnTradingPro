//! 启动流程页面：首次设置主密码 → 输入 API 密钥 → 进入主界面；已存在凭据库则解锁
//!
//! 设计：深色背景 + 品牌光晕 + 居中认证卡片，分步指示，主按钮金色。

use crate::app::{BnApp, Phase};
use crate::config;
use crate::engine::state::ToastKind;
use crate::security::vault::{self, Creds};
use crate::ui::theme::{self, ACCENT, BG, BORDER, CARD, ERR, TEXT, TEXT_DIM, WARN};
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Stroke};

pub fn show(app: &mut BnApp, ctx: &egui::Context) {
    // ---------- 背景：深色 + 品牌光晕 ----------
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(BG))
        .show(ctx, |ui| {
            let rect = ui.max_rect();
            let p = ui.painter();
            // 两团品牌色光晕，营造氛围（静态、廉价）
            p.circle_filled(
                Pos2::new(rect.left() + rect.width() * 0.18, rect.top() + rect.height() * 0.12),
                rect.height() * 0.42,
                Color32::from_rgba_unmultiplied(240, 185, 11, 10),
            );
            p.circle_filled(
                Pos2::new(rect.right() - rect.width() * 0.15, rect.bottom() - rect.height() * 0.08),
                rect.height() * 0.38,
                Color32::from_rgba_unmultiplied(46, 203, 129, 8),
            );
            // 品牌标题
            let cx = rect.center().x;
            let cy = rect.center().y;
            p.text(
                Pos2::new(cx, cy - 196.0),
                Align2::CENTER_CENTER,
                "BnTradingPro",
                FontId::proportional(34.0),
                ACCENT,
            );
            p.text(
                Pos2::new(cx, cy - 164.0),
                Align2::CENTER_CENTER,
                "Binance 现货 / U本位合约 交易终端",
                FontId::proportional(13.5),
                TEXT_DIM,
            );
        });

    // ---------- 居中认证卡片 ----------
    egui::Area::new(egui::Id::new("auth_card"))
        .anchor(Align2::CENTER_CENTER, [0.0, 26.0])
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(CARD)
                .stroke(Stroke::new(1.0_f32, BORDER))
                .rounding(8.0)
                .inner_margin(egui::Margin::same(26.0))
                .show(ui, |ui| {
                    ui.set_min_width(400.0);
                    match app.phase {
                        Phase::SetupMaster => step_header(ui, "设置主密码", 1, 2),
                        Phase::Unlock => step_header(ui, "解锁", 0, 0),
                        Phase::EnterKeys => {
                            let step = if app.master.is_none() { 2 } else { 1 };
                            if app.master.is_none() {
                                // 从解锁/重配流程进入：仍是第 2 步
                                step_header(ui, "配置 API 密钥", 2, 2);
                            } else {
                                step_header(ui, "配置 API 密钥", step, 2);
                            }
                        }
                        Phase::Main => {}
                    }
                    ui.add_space(6.0);
                    match app.phase {
                        Phase::SetupMaster => setup_master(app, ui),
                        Phase::Unlock => unlock(app, ui),
                        Phase::EnterKeys => enter_keys(app, ui),
                        Phase::Main => {}
                    }
                });
        });
    draw_env_hint(ctx, app);
}

/// 卡片头：标题 + 分步指示（total 为 0 时不显示步骤）
fn step_header(ui: &mut egui::Ui, title: &str, step: usize, total: usize) {
    ui.label(egui::RichText::new(title).heading());
    if total > 0 {
        let dots: String = (1..=total)
            .map(|i| if i == step { "●" } else { "○" })
            .collect::<Vec<_>>()
            .join("  ");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            ui.label(egui::RichText::new(dots).size(10.0).color(TEXT_DIM));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                ui.label(
                    egui::RichText::new(format!("第 {step} / {total} 步"))
                        .size(11.5)
                        .color(TEXT_DIM),
                );
            });
        });
    }
    ui.separator();
}

fn draw_env_hint(ctx: &egui::Context, app: &BnApp) {
    let testnet = app.state.cfg.read().testnet;
    let (txt, color) = if testnet {
        ("TESTNET", ACCENT)
    } else {
        ("实盘环境 · 真实资金", ERR)
    };
    egui::Area::new(egui::Id::new("unlock_env"))
        .anchor(Align2::RIGHT_TOP, [-14.0, 14.0])
        .show(ctx, |ui| {
            theme::badge(ui, txt, color);
        });
}

fn error_text(ui: &mut egui::Ui, msg: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("✗").color(ERR).strong().size(12.5));
        ui.label(egui::RichText::new(msg).color(ERR).size(12.5));
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
    .fill(ACCENT)
    .rounding(5.0)
    .min_size(egui::vec2(ui.available_width(), 34.0));
    ui.add_enabled(enabled, btn).clicked()
}

/// 首次使用：创建主密码
fn setup_master(app: &mut BnApp, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("主密码用于加密本机保存的 API Key/Secret（Argon2id + AES-256-GCM）。忘记将无法恢复，需删除凭据库重新配置。")
            .size(12.0)
            .color(TEXT_DIM),
    );
    ui.add_space(10.0);

    ui.label(theme::card_label("主密码（至少 8 位）"));
    ui.add(
        egui::TextEdit::singleline(&mut app.pw1)
            .password(true)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    ui.add_space(4.0);
    ui.label(theme::card_label("再输一次"));
    ui.add(
        egui::TextEdit::singleline(&mut app.pw2)
            .password(true)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    if !app.pw_err.is_empty() {
        ui.add_space(2.0);
        error_text(ui, &app.pw_err);
    }
    ui.add_space(12.0);
    if primary_btn(ui, "继续", true) {
        if app.pw1.len() < 8 {
            app.pw_err = "主密码至少 8 位".into();
        } else if app.pw1 != app.pw2 {
            app.pw_err = "两次输入不一致".into();
        } else {
            app.master = Some(app.pw1.clone());
            app.pw1.clear();
            app.pw2.clear();
            app.pw_err.clear();
            app.phase = Phase::EnterKeys;
        }
    }
}

/// 解锁已有凭据库
fn unlock(app: &mut BnApp, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("输入主密码以解密本机保存的 API 密钥")
            .size(12.0)
            .color(TEXT_DIM),
    );
    ui.add_space(10.0);
    ui.label(theme::card_label("主密码"));
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
    if primary_btn(ui, "解锁", true) {
        try_unlock(app);
    }
    ui.add_space(2.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let dir = config::data_dir();
        if ui
            .small_button(
                egui::RichText::new("重置凭据库")
                    .size(11.5)
                    .color(WARN),
            )
            .on_hover_text("删除本机加密凭据库，重新开始设置")
            .clicked()
        {
            let _ = std::fs::remove_file(dir.join(vault::VAULT_FILE));
            app.phase = Phase::SetupMaster;
        }
        if ui
            .small_button(egui::RichText::new("仅看行情，暂不解锁").size(11.5))
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
                .push_toast(ToastKind::Ok, "已解锁，开始同步账户数据");
            app.phase = Phase::Main;
        }
        Err(vault::VaultError::NotFound) => {
            app.phase = Phase::SetupMaster;
        }
        Err(_) => {
            app.pw_err = "主密码错误，或凭据文件损坏".into();
        }
    }
}

/// 输入 API 密钥
fn enter_keys(app: &mut BnApp, ui: &mut egui::Ui) {
    let testnet = app.state.cfg.read().testnet;
    ui.label(
        egui::RichText::new(if testnet {
            "当前环境：测试网（testnet.binance.vision / testnet.binancefuture.com 申请）"
        } else {
            "当前环境：实盘（binance.com → API 管理，建议绑定 IP 白名单并只开启现货/合约交易权限）"
        })
        .size(12.0)
        .color(if testnet { ACCENT } else { ERR }),
    );
    ui.add_space(10.0);
    // 从设置页/重配流程进入时需再次提供主密码用于加密
    if app.master.is_none() {
        ui.label(theme::card_label("主密码（用于加密保存）"));
        ui.add(
            egui::TextEdit::singleline(&mut app.pw1)
                .password(true)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace),
        );
        ui.add_space(6.0);
    }
    ui.label(theme::card_label("API Key"));
    ui.add(
        egui::TextEdit::singleline(&mut app.api_key)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .text_color(TEXT),
    );
    ui.add_space(6.0);
    ui.label(theme::card_label("API Secret"));
    ui.add(
        egui::TextEdit::singleline(&mut app.api_secret)
            .password(true)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .text_color(TEXT),
    );
    if !app.keys_err.is_empty() {
        ui.add_space(2.0);
        error_text(ui, &app.keys_err);
    }
    ui.add_space(12.0);
    let can_save = !app.api_key.trim().is_empty() && !app.api_secret.trim().is_empty();
    let mut do_save = false;
    if primary_btn(ui, "保存并连接", can_save) {
        do_save = true;
    }
    if do_save {
        let creds = Creds {
            api_key: app.api_key.trim().to_string(),
            api_secret: app.api_secret.trim().to_string(),
        };
        let dir = config::data_dir();
        let master = app.master.clone().or_else(|| {
            let m = app.pw1.clone();
            if m.is_empty() { None } else { Some(m) }
        });
        match master {
            Some(pw) => {
                // 校验主密码（若凭据库已存在）
                if vault::vault_exists(&dir) {
                    match vault::load_creds(&dir, &pw) {
                        Err(vault::VaultError::NotFound) => {}
                        Err(_) => {
                            app.keys_err = "主密码错误".into();
                            return;
                        }
                        Ok(_) => {}
                    }
                }
                match vault::save_creds(&dir, &pw, &creds) {
                    Ok(()) => {
                        app.state.set_creds(Some(creds));
                        app.api_key.clear();
                        app.api_secret.clear();
                        app.keys_err.clear();
                        // 用完即清明文主密码
                        app.master = None;
                        app.pw1.clear();
                        app.state
                            .push_toast(ToastKind::Ok, "密钥已加密保存并连接");
                        app.phase = Phase::Main;
                    }
                    Err(e) => app.keys_err = format!("保存失败: {e}"),
                }
            }
            None => {
                app.keys_err = "请输入主密码".into();
            }
        }
    }
    ui.add_space(2.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui
            .small_button(egui::RichText::new("跳过，仅看行情").size(11.5).color(TEXT_DIM))
            .clicked()
        {
            app.api_key.clear();
            app.api_secret.clear();
            app.phase = Phase::Main;
        }
    });
    // 提示未配置也能浏览行情
    ui.label(
        egui::RichText::new("跳过后仍可查看实时行情与 K 线，随时可在设置页配置密钥")
            .size(11.0)
            .color(TEXT_DIM.gamma_multiply(0.85)),
    );
}
