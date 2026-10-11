//! 启动流程页面：首次设置主密码 → 输入 API 密钥 → 进入主界面；已存在凭据库则解锁

use crate::app::{BnApp, Phase};
use crate::config;
use crate::engine::state::ToastKind;
use crate::security::vault::{self, Creds};
use eframe::egui;
use egui::Align2;

pub fn show(app: &mut BnApp, ctx: &egui::Context) {
    // 背景与标题
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(crate::ui::theme::BG))
        .show(ctx, |ui| {
            let rect = ui.max_rect();
            let cx = rect.center().x;
            let cy = rect.center().y;
            ui.painter().text(
                egui::Pos2::new(cx, cy - 200.0),
                Align2::CENTER_CENTER,
                "BnTradingPro",
                egui::FontId::proportional(30.0),
                crate::ui::theme::ACCENT,
            );
            ui.painter().text(
                egui::Pos2::new(cx, cy - 174.0),
                Align2::CENTER_CENTER,
                "Binance 现货 / U本位合约 交易终端",
                egui::FontId::proportional(13.0),
                crate::ui::theme::TEXT_DIM,
            );
        });

    // 居中卡片
    egui::Area::new(egui::Id::new("auth_card"))
        .anchor(Align2::CENTER_CENTER, [0.0, 30.0])
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(crate::ui::theme::PANEL2)
                .stroke(egui::Stroke::new(1.0_f32, crate::ui::theme::BORDER))
                .inner_margin(24.0)
                .show(ui, |ui| {
                    ui.set_min_width(400.0);
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

fn draw_env_hint(ctx: &egui::Context, app: &BnApp) {
    let testnet = app.state.cfg.read().testnet;
    let (txt, color) = if testnet {
        ("TESTNET", crate::ui::theme::ACCENT)
    } else {
        ("实盘环境 · 真实资金", crate::ui::theme::ERR)
    };
    egui::Area::new(egui::Id::new("unlock_env"))
        .anchor(Align2::RIGHT_TOP, [-12.0, 12.0])
        .show(ctx, |ui| {
            crate::ui::theme::badge(ui, txt, color);
        });
}

/// 首次使用：创建主密码
fn setup_master(app: &mut BnApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("创建主密码").heading());
    ui.label(egui::RichText::new("主密码用于加密本机保存的 API Key/Secret（Argon2id + AES-256-GCM）。忘记将无法恢复，需删除凭据库重新配置。").size(11.5).color(crate::ui::theme::TEXT_DIM));
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.add_sized([60.0, 18.0], egui::Label::new("主密码"));
        ui.add(
            egui::TextEdit::singleline(&mut app.pw1)
                .password(true)
                .desired_width(240.0)
                .hint_text("至少 8 位"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("再输一次");
        ui.add(
            egui::TextEdit::singleline(&mut app.pw2)
                .password(true)
                .desired_width(240.0)
                .hint_text("再次输入主密码"),
        );
    });
    if !app.pw_err.is_empty() {
        ui.label(
            egui::RichText::new(&app.pw_err)
                .color(crate::ui::theme::ERR)
                .size(12.0),
        );
    }
    ui.add_space(8.0);
    if ui
        .add_sized([240.0, 30.0], egui::Button::new("继续"))
        .clicked()
    {
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
    ui.label(egui::RichText::new("解锁").heading());
    ui.label(
        egui::RichText::new("输入主密码以解密本机保存的 API 密钥")
            .size(11.5)
            .color(crate::ui::theme::TEXT_DIM),
    );
    ui.add_space(8.0);
    let pw_resp = ui.add(
        egui::TextEdit::singleline(&mut app.pw1)
            .password(true)
            .desired_width(240.0)
            .hint_text("主密码"),
    );
    if pw_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        try_unlock(app);
    }
    if !app.pw_err.is_empty() {
        ui.label(
            egui::RichText::new(&app.pw_err)
                .color(crate::ui::theme::ERR)
                .size(12.0),
        );
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .add_sized([150.0, 30.0], egui::Button::new("解锁"))
            .clicked()
        {
            try_unlock(app);
        }
        if ui.small_button("仅看行情，暂不解锁").clicked() {
            app.phase = Phase::Main;
        }
    });
    let dir = config::data_dir();
    if ui.small_button("删除凭据库并重置").clicked() {
        let _ = std::fs::remove_file(dir.join(vault::VAULT_FILE));
        app.phase = Phase::SetupMaster;
    }
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
    ui.label(egui::RichText::new("配置 Binance API 密钥").heading());
    let testnet = app.state.cfg.read().testnet;
    ui.label(
        egui::RichText::new(if testnet {
            "当前环境：测试网（testnet.binance.vision / testnet.binancefuture.com 申请）"
        } else {
            "当前环境：实盘（binance.com → API 管理，建议绑定 IP 白名单并只开启现货/合约交易权限）"
        })
        .size(11.5)
        .color(if testnet {
            crate::ui::theme::ACCENT
        } else {
            crate::ui::theme::ERR
        }),
    );
    ui.add_space(8.0);
    // 从设置页进入时需再次提供主密码用于加密
    if app.master.is_none() {
        ui.label(egui::RichText::new("主密码（用于加密保存）").size(12.0));
        ui.add(
            egui::TextEdit::singleline(&mut app.pw1)
                .password(true)
                .desired_width(300.0)
                .hint_text("本机凭据库的主密码"),
        );
        ui.add_space(4.0);
    }
    ui.label("API Key");
    ui.add(
        egui::TextEdit::singleline(&mut app.api_key)
            .desired_width(300.0)
            .font(egui::TextStyle::Monospace),
    );
    ui.add_space(2.0);
    ui.label("API Secret");
    ui.add(
        egui::TextEdit::singleline(&mut app.api_secret)
            .password(true)
            .desired_width(300.0)
            .font(egui::TextStyle::Monospace),
    );
    if !app.keys_err.is_empty() {
        ui.label(
            egui::RichText::new(&app.keys_err)
                .color(crate::ui::theme::ERR)
                .size(12.0),
        );
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let can_save = !app.api_key.trim().is_empty() && !app.api_secret.trim().is_empty();
        if ui
            .add_enabled(can_save, egui::Button::new("保存并连接"))
            .clicked()
        {
            let creds = Creds {
                api_key: app.api_key.trim().to_string(),
                api_secret: app.api_secret.trim().to_string(),
            };
            let dir = config::data_dir();
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
                            app.state.push_toast(ToastKind::Ok, "密钥已加密保存并连接");
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
        if ui.button("跳过，仅看行情").clicked() {
            app.api_key.clear();
            app.api_secret.clear();
            app.phase = Phase::Main;
        }
    });
}
