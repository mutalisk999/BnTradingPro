//! 设置页：环境、代理、端点覆盖、参数与密钥管理
//!
//! 输入绑定到 `BnApp.settings` 草稿（egui 表单状态必须跨帧存活），保存时写回配置。

use crate::app::{BnApp, Phase, SettingsDraft};
use crate::config::{self, AppConfig};
use crate::engine::state::ToastKind;
use crate::security::vault;
use crate::ui::theme;
use eframe::egui;
use std::sync::Arc;

pub fn show(app: &mut BnApp, ui: &mut egui::Ui) {
    let state = Arc::clone(&app.state);
    let dir = config::data_dir();
    let mut apply = false;

    ui.heading(
        egui::RichText::new(crate::i18n::tr("设置"))
            .strong()
            .size(theme::FONT_LG),
    );
    ui.separator();

    // ---------- 环境 ----------
    theme::card_with_header(ui, crate::i18n::tr("交易环境"), |_| {}, |ui| {
        theme::form_row(ui, |ui| {
            ui.selectable_value(
                &mut app.settings.testnet,
                true,
                crate::i18n::tr("测试网 (Testnet)"),
            );
            ui.selectable_value(&mut app.settings.testnet, false, crate::i18n::tr("实盘"));
            if app.settings.testnet {
                theme::badge(ui, "TESTNET", theme::cur().accent);
                ui.label(
                    egui::RichText::new(crate::i18n::tr(
                        "测试网密钥在 testnet.binance.vision / testnet.binancefuture.com 申请",
                    ))
                    .size(theme::FONT_SM)
                    .color(theme::cur().text_dim),
                );
            } else {
                theme::badge(ui, crate::i18n::tr("实盘 · 真实资金"), theme::cur().err);
            }
        });
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "测试网与实盘的 API 密钥不通用，切换环境后需重新配置密钥",
            ))
            .size(theme::FONT_SM)
            .color(theme::cur().text_dim),
        );
        if !app.settings.testnet {
            ui.label(
                egui::RichText::new(crate::i18n::tr(
                    "⚠ 实盘模式下下单将使用真实资金，请确认密钥与环境正确！",
                ))
                .size(theme::FONT_MD)
                .color(theme::cur().err),
            );
        }
    });

    // ---------- 代理 ----------
    theme::card_with_header(
        ui,
        crate::i18n::tr("网络代理（REST 与 WebSocket 均生效）"),
        |_| {},
        |ui| {
            ui.checkbox(&mut app.settings.proxy_enabled, crate::i18n::tr("启用代理"));
        theme::form_row(ui, |ui| {
            theme::row_label(ui, crate::i18n::tr("地址"), 40.0);
            // 明文显示：便于核对代理主机与端口（凭据泄露风险已由
            // ProxyConfig 的 Debug 脱敏与 config.toml 权限约束兜底）
            ui.add(
                egui::TextEdit::singleline(&mut app.settings.proxy_url)
                    .text_color(theme::cur().text)
                    .desired_width(320.0)
                    .min_size(egui::vec2(0.0, theme::CONTROL_H))
                    .hint_text(
                        "http://127.0.0.1:7890 或 socks5://127.0.0.1:7890（支持 user:pass@）",
                    ),
            );
        });
    });

    // ---------- 外观与语言 ----------
    theme::card_with_header(ui, crate::i18n::tr("语言与主题"), |_| {}, |ui| {
        theme::form_row(ui, |ui| {
            theme::row_label(ui, crate::i18n::tr("语言"), 40.0);
            let mut lang = app.settings.lang.clone();
            egui::ComboBox::from_id_salt("lang_cb")
                .selected_text(if lang == "en" {
                    "English".to_string()
                } else {
                    "中文".to_string()
                })
                .width(90.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut lang, "zh".to_string(), "中文");
                    ui.selectable_value(&mut lang, "en".to_string(), "English");
                });
            app.settings.lang = lang;
            ui.separator();
            theme::row_label(ui, crate::i18n::tr("主题"), 40.0);
            let mut mode = app.settings.theme_mode.clone();
            egui::ComboBox::from_id_salt("theme_cb")
                .selected_text(if mode == "light" {
                    crate::i18n::tr("浅色").to_string()
                } else {
                    crate::i18n::tr("深色").to_string()
                })
                .width(90.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut mode, "dark".to_string(), crate::i18n::tr("深色"));
                    ui.selectable_value(&mut mode, "light".to_string(), crate::i18n::tr("浅色"));
                });
            app.settings.theme_mode = mode;
        });
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "保存后生效；顶栏右侧 深色/浅色 与 中/EN 按钮可即时切换",
            ))
            .size(theme::FONT_SM)
            .color(theme::cur().text_dim),
        );
    });

    // ---------- 参数 ----------
    theme::card_with_header(ui, crate::i18n::tr("请求参数"), |_| {}, |ui| {
        theme::form_row(ui, |ui| {
            ui.label(crate::i18n::tr("接收窗口"));
            // 英文界面标签本身就是 "recvWindow"，仅中文界面补英文小字说明
            if crate::i18n::lang() == crate::i18n::ZH {
                ui.label(
                    egui::RichText::new("recvWindow")
                        .size(theme::FONT_SM)
                        .color(theme::cur().text_dim),
                );
            }
            ui.add(
                egui::DragValue::new(&mut app.settings.recv_window_ms)
                    .range(1000..=60000)
                    .suffix(" ms"),
            );
            ui.separator();
            ui.label(crate::i18n::tr("HTTP 超时"));
            ui.add(
                egui::DragValue::new(&mut app.settings.http_timeout_secs)
                    .range(3..=120)
                    .suffix(" s"),
            );
            ui.separator();
            ui.label(crate::i18n::tr("日志级别"));
            egui::ComboBox::from_id_salt("log_level")
                .selected_text(app.settings.log_level.clone())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for l in ["trace", "debug", "info", "warn", "error"] {
                        ui.selectable_value(&mut app.settings.log_level, l.to_string(), l);
                    }
                });
        });
        theme::form_row(ui, |ui| {
            ui.label(crate::i18n::tr("默认交易对"));
            ui.add(
                egui::TextEdit::singleline(&mut app.settings.default_symbol)
                    .text_color(theme::cur().text)
                    .desired_width(110.0)
                    .min_size(egui::vec2(0.0, theme::CONTROL_H))
                    .hint_text("BTCUSDT"),
            );
            ui.label(crate::i18n::tr("默认K线周期"));
            egui::ComboBox::from_id_salt("default_iv")
                .selected_text(app.settings.default_interval.clone())
                .width(70.0)
                .show_ui(ui, |ui| {
                    for iv in crate::binance::Interval::all() {
                        ui.selectable_value(
                            &mut app.settings.default_interval,
                            iv.as_str().to_string(),
                            iv.label(),
                        );
                    }
                });
        });
    });

    // ---------- 端点覆盖 ----------
    let overridden = [
        &app.settings.override_spot_rest,
        &app.settings.override_spot_ws,
        &app.settings.override_fut_rest,
        &app.settings.override_fut_ws,
    ]
    .iter()
    .any(|s| !s.trim().is_empty());
    theme::card_with_header(
        ui,
        crate::i18n::tr("端点覆盖（留空使用内置默认）"),
        |ui| {
            if overridden {
                // 自定义端点会把携带 API Key 的请求重定向到任意主机，必须显式可见
                theme::badge(ui, crate::i18n::tr("自定义端点已启用"), theme::cur().warn);
            }
        },
        |ui| {
        egui::Grid::new("ep_grid")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label(crate::i18n::tr("现货 REST"));
                ui.add(
                    egui::TextEdit::singleline(&mut app.settings.override_spot_rest)
                        .text_color(theme::cur().text)
                        .desired_width(360.0)
                        .hint_text("https://api.binance.com"),
                );
                ui.end_row();
                ui.label(crate::i18n::tr("现货 WS"));
                ui.add(
                    egui::TextEdit::singleline(&mut app.settings.override_spot_ws)
                        .text_color(theme::cur().text)
                        .desired_width(360.0)
                        .hint_text("wss://stream.binance.com:9443"),
                );
                ui.end_row();
                ui.label(crate::i18n::tr("合约 REST"));
                ui.add(
                    egui::TextEdit::singleline(&mut app.settings.override_fut_rest)
                        .text_color(theme::cur().text)
                        .desired_width(360.0)
                        .hint_text("https://fapi.binance.com"),
                );
                ui.end_row();
                ui.label(crate::i18n::tr("合约 WS"));
                ui.add(
                    egui::TextEdit::singleline(&mut app.settings.override_fut_ws)
                        .text_color(theme::cur().text)
                        .desired_width(360.0)
                        .hint_text("wss://fstream.binance.com"),
                );
                ui.end_row();
            });
    });

    // ---------- 应用 ----------
    // 主操作按钮：accent 填充（与解锁页主按钮一致），视觉层级高于普通按钮
    if theme::btn_primary(ui, crate::i18n::tr("保存并重连"), true).clicked() {
        apply = true;
    }

    ui.add_space(6.0);
    ui.separator();

    // ---------- 密钥管理 ----------
    theme::card_with_header(ui, crate::i18n::tr("API 密钥"), |_| {}, |ui| {
        theme::form_row(ui, |ui| {
            if state.has_creds() {
                theme::badge(ui, crate::i18n::tr("已配置"), theme::cur().up);
            } else {
                theme::badge(
                    ui,
                    crate::i18n::tr("未配置（仅公开行情）"),
                    theme::cur().warn,
                );
            }
            if ui
                .small_button(if state.has_creds() {
                    crate::i18n::tr("更换密钥")
                } else {
                    crate::i18n::tr("配置密钥")
                })
                .clicked()
            {
                app.keys_err.clear();
                app.api_key.clear();
                app.api_secret.clear();
                app.phase = Phase::EnterKeys;
            }
            if vault::vault_exists(&dir) {
                // 破坏性操作两步确认：误点一次不会直接销毁唯一的本地密钥副本
                let armed = app.vault_delete_armed;
                let del = theme::btn_danger(
                    ui,
                    if armed {
                        crate::i18n::tr("确认删除凭据库？")
                    } else {
                        crate::i18n::tr("删除凭据库")
                    },
                    true,
                );
                if del.clicked() {
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
                            state.set_creds(None);
                            state.push_toast(
                                ToastKind::Info,
                                crate::i18n::tr("已删除本机凭据库").to_string(),
                            );
                            app.phase = Phase::EnterKeys;
                        } else {
                            state.push_toast(
                                ToastKind::Err,
                                crate::i18n::tr("删除凭据库失败").to_string(),
                            );
                        }
                    }
                }
            }
        });
    });

    // ---------- 关于 ----------
    theme::card_with_header(ui, crate::i18n::tr("关于"), |_| {}, |ui| {
        ui.label(
            egui::RichText::new(format!(
                "BnTradingPro v{} · {} {}",
                env!("CARGO_PKG_VERSION"),
                crate::i18n::tr("数据目录"),
                dir.display()
            ))
            .size(theme::FONT_SM)
            .color(theme::cur().text_dim),
        );
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "本工具仅供个人交易辅助与研究，不构成任何投资建议；使用实盘交易产生的一切后果由使用者自行承担。",
            ))
            .size(theme::FONT_SM)
            .color(theme::cur().text_dim),
        );
    });

    if apply {
        // 自选列表不在设置页编辑（由侧栏星标实时修改 state.cfg），必须以 live
        // 值覆盖草稿，否则保存设置会把用户启动后添加的自选静默回滚到旧快照。
        // 语言/主题设置页可编辑，以草稿为准（顶栏快捷切换会同步进草稿）。
        {
            let live = state.cfg.read();
            app.settings.favorites_spot = live.favorites_spot.clone();
            app.settings.favorites_futures = live.favorites_futures.clone();
        }
        let cfg: AppConfig = std::mem::take(&mut app.settings).into_cfg();
        // 保留草稿（into_cfg 消耗了草稿），保证继续编辑不丢字段
        app.settings = SettingsDraft::from_cfg(&cfg);
        // 测试网 → 实盘时不沿用端点覆盖：第三方测试网端点会把携带 API Key 的
        // 实盘签名请求发往该主机，自动清空并提示
        let was_testnet = state.cfg.read().testnet;
        let mut cfg = cfg;
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
                    crate::i18n::tr("已清空端点覆盖：测试网与实盘端点不通用").to_string(),
                );
            }
        }
        // 先更新内存再发落盘命令：引擎在另一线程出队时快照的是 state.cfg，
        // 顺序反了会把旧配置写回 config.toml
        state.update_cfg(cfg);
        state.send_cmd(crate::engine::Command::PersistConfig);
        state.push_toast(
            ToastKind::Ok,
            crate::i18n::tr("设置已保存，正在重连…").to_string(),
        );
    }
}
