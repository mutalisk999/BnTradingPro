//! 设置页：环境、代理、端点覆盖、参数与密钥管理

use crate::app::{BnApp, Phase};
use crate::config;
use crate::engine::state::ToastKind;
use crate::security::vault;
use crate::ui::theme;
use eframe::egui;
use std::sync::Arc;

pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let mut cfg = state.cfg_snapshot();
    let dir = config::data_dir();
    let mut apply = false;

    ui.heading(egui::RichText::new("设置").size(14.0));
    ui.separator();

    // ---------- 环境 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new("交易环境").strong());
        ui.horizontal(|ui| {
            ui.selectable_value(&mut cfg.testnet, true, "测试网 (Testnet)");
            ui.selectable_value(&mut cfg.testnet, false, "实盘");
            if cfg.testnet {
                theme::badge(ui, "TESTNET", theme::ACCENT);
                ui.label(
                    egui::RichText::new(
                        "测试网密钥在 testnet.binance.vision / testnet.binancefuture.com 申请",
                    )
                    .size(11.0)
                    .color(theme::TEXT_DIM),
                );
            } else {
                theme::badge(ui, "实盘 · 真实资金", theme::ERR);
            }
        });
        if !cfg.testnet {
            ui.label(
                egui::RichText::new("⚠ 实盘模式下下单将使用真实资金，请确认密钥与环境正确！")
                    .size(12.0)
                    .color(theme::ERR),
            );
        }
    });

    // ---------- 代理 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new("网络代理（REST 与 WebSocket 均生效）").strong());
        ui.checkbox(&mut cfg.proxy.enabled, "启用代理");
        ui.horizontal(|ui| {
            ui.label("地址");
            ui.add(
                egui::TextEdit::singleline(&mut cfg.proxy.url)
                    .desired_width(320.0)
                    .hint_text(
                        "http://127.0.0.1:7890 或 socks5://127.0.0.1:7890（支持 user:pass@）",
                    ),
            );
        });
    });

    // ---------- 参数 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new("请求参数").strong());
        ui.horizontal(|ui| {
            ui.label("recvWindow");
            ui.add(
                egui::DragValue::new(&mut cfg.recv_window_ms)
                    .range(1000..=60000)
                    .suffix(" ms"),
            );
            ui.separator();
            ui.label("HTTP 超时");
            ui.add(
                egui::DragValue::new(&mut cfg.http_timeout_secs)
                    .range(3..=120)
                    .suffix(" s"),
            );
            ui.separator();
            ui.label("日志级别");
            egui::ComboBox::from_id_salt("log_level")
                .selected_text(cfg.log_level.clone())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for l in ["trace", "debug", "info", "warn", "error"] {
                        ui.selectable_value(&mut cfg.log_level, l.to_string(), l);
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("默认交易对");
            ui.add(
                egui::TextEdit::singleline(&mut cfg.default_symbol)
                    .desired_width(110.0)
                    .hint_text("BTCUSDT"),
            );
            ui.label("默认K线周期");
            egui::ComboBox::from_id_salt("default_iv")
                .selected_text(cfg.default_interval.clone())
                .width(70.0)
                .show_ui(ui, |ui| {
                    for iv in crate::binance::Interval::all() {
                        ui.selectable_value(
                            &mut cfg.default_interval,
                            iv.as_str().to_string(),
                            iv.label(),
                        );
                    }
                });
        });
    });

    // ---------- 端点覆盖 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new("端点覆盖（留空使用内置默认）").strong());
        egui::Grid::new("ep_grid")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label("现货 REST");
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.override_spot_rest)
                        .desired_width(360.0)
                        .hint_text("https://api.binance.com"),
                );
                ui.end_row();
                ui.label("现货 WS");
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.override_spot_ws)
                        .desired_width(360.0)
                        .hint_text("wss://stream.binance.com:9443"),
                );
                ui.end_row();
                ui.label("合约 REST");
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.override_fut_rest)
                        .desired_width(360.0)
                        .hint_text("https://fapi.binance.com"),
                );
                ui.end_row();
                ui.label("合约 WS");
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.override_fut_ws)
                        .desired_width(360.0)
                        .hint_text("wss://fstream.binance.com"),
                );
                ui.end_row();
            });
    });

    // ---------- 应用 ----------
    if ui
        .add(
            egui::Button::new(egui::RichText::new("保存并重连").strong())
                .min_size(egui::vec2(140.0, 28.0)),
        )
        .clicked()
    {
        cfg.default_symbol = cfg.default_symbol.trim().to_uppercase();
        apply = true;
    }

    ui.add_space(6.0);
    ui.separator();

    // ---------- 密钥管理 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new("API 密钥").strong());
        ui.horizontal(|ui| {
            if state.has_creds() {
                theme::badge(ui, "已配置", theme::UP);
            } else {
                theme::badge(ui, "未配置（仅公开行情）", theme::WARN);
            }
            if ui
                .small_button(if state.has_creds() {
                    "更换密钥"
                } else {
                    "配置密钥"
                })
                .clicked()
            {
                app.keys_err.clear();
                app.api_key.clear();
                app.api_secret.clear();
                app.phase = Phase::EnterKeys;
            }
            if vault::vault_exists(&dir) && ui.small_button("删除凭据库").clicked() {
                let _ = std::fs::remove_file(dir.join(vault::VAULT_FILE));
                state.set_creds(None);
                state.push_toast(ToastKind::Info, "已删除本机凭据库");
                app.phase = Phase::SetupMaster;
            }
        });
    });

    // ---------- 关于 ----------
    ui.group(|ui| {
        ui.label(egui::RichText::new("关于").strong());
        ui.label(egui::RichText::new(format!(
            "BnTradingPro v{} · 数据目录: {}",
            env!("CARGO_PKG_VERSION"),
            dir.display()
        )).size(11.5).color(theme::TEXT_DIM));
        ui.label(egui::RichText::new("本工具仅供个人交易辅助与研究，不构成任何投资建议；使用实盘交易产生的一切后果由使用者自行承担。").size(11.0).color(theme::TEXT_DIM));
    });

    if apply {
        cfg.save(&dir);
        state.update_cfg(cfg);
        state.push_toast(ToastKind::Ok, "设置已保存，正在重连…");
    }
}
