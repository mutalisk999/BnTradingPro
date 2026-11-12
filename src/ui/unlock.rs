//! 启动流程：已有凭据库 → 解锁；无凭据库 → 配置 API 密钥（与主密码一体设置）
//!
//! 设计：深色背景 + 品牌光晕 + 居中认证卡片，主按钮金色。
//! 里程碑 1：try_unlock / save_keys 纯逻辑原样迁移；页面渲染在里程碑 3 移植
//! （光晕 canvas + TextField 表单 + btn_primary/btn_secondary/btn_danger）。

use crate::app::{BnApp, Phase};
use crate::config;
use crate::engine::state::ToastKind;
use crate::security::vault::{self, Creds};

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
