//! 加密凭据库（vault）
//!
//! 文件格式（`vault.bin`）：`BTPV1` | salt(16) | nonce(12) | AES-256-GCM 密文
//! 明文为 JSON 序列化的 API Key/Secret，密钥由主密码经 Argon2id 派生。

use crate::security::kdf::{self, SALT_LEN};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const VAULT_FILE: &str = "vault.bin";
/// V1：Argon2id m=19MiB/t=2（OWASP 基线，仅读取旧库）
const MAGIC_V1: &[u8; 5] = b"BTPV1";
/// V2：Argon2id m=64MiB/t=3（当前保存格式）
const MAGIC_V2: &[u8; 5] = b"BTPV2";
const NONCE_LEN: usize = 12;

/// API 凭据。Debug 输出脱敏；Drop 时零化内存，避免密钥残留在已释放的堆上
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Creds {
    pub api_key: String,
    pub api_secret: String,
}

impl std::fmt::Debug for Creds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Creds")
            .field("api_key", &"<redacted>")
            .field("api_secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("尚未创建凭据库")]
    NotFound,
    #[error("主密码错误，或凭据文件已损坏")]
    BadPassword,
    #[error("凭据文件损坏")]
    Corrupt,
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

/// 加密保存 API 凭据
pub fn save_creds(dir: &Path, password: &str, creds: &Creds) -> Result<(), String> {
    let salt = {
        let mut s = [0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut s);
        s
    };
    let key = kdf::derive_key(password, &salt, 2)?;
    let cipher = Aes256Gcm::new_from_slice(&key[..]).map_err(|e| e.to_string())?;
    let nonce = {
        let mut n = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut n);
        n
    };
    // 明文缓冲用 Zeroizing 包裹，加解密后即从内存抹除
    let plaintext = Zeroizing::new(serde_json::to_vec(creds).map_err(|e| e.to_string())?);
    let msg: &[u8] = &plaintext;
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg, aad: MAGIC_V2 })
        .map_err(|e| format!("{}{e}", crate::i18n::tr("加密失败: ")))?;

    let mut buf = Vec::with_capacity(MAGIC_V2.len() + SALT_LEN + NONCE_LEN + ct.len());
    buf.extend_from_slice(MAGIC_V2);
    buf.extend_from_slice(&salt);
    buf.extend_from_slice(&nonce);
    buf.extend_from_slice(&ct);

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(VAULT_FILE);
    // 先写临时文件再原子替换，避免写入中断留下损坏的凭据库。
    // unix 下用 OpenOptions.mode(0o600) 直接以受限权限创建 tmp：先按默认
    // 0644 落盘再 chmod 存在权限时窗，且 chmod 失败不可忽略（否则 rename
    // 后凭据库长期可被同机其他用户读取，密文可被离线爆破主密码）
    let tmp = dir.join(format!("{VAULT_FILE}.tmp"));
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| {
                format!(
                    "{}({}): {e}",
                    crate::i18n::tr("写入凭据文件失败"),
                    tmp.display()
                )
            })?;
        f.write_all(&buf).map_err(|e| {
            format!(
                "{}({}): {e}",
                crate::i18n::tr("写入凭据文件失败"),
                tmp.display()
            )
        })?;
        // 兜底：文件已存在等异常路径下 mode 不生效，rename 前再校验一次
        if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "{}({}): {e}",
                crate::i18n::tr("收紧凭据文件权限失败"),
                tmp.display()
            ));
        }
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, &buf).map_err(|e| {
        format!(
            "{}({}): {e}",
            crate::i18n::tr("写入凭据文件失败"),
            tmp.display()
        )
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        format!(
            "{}({}): {e}",
            crate::i18n::tr("替换凭据文件失败"),
            path.display()
        )
    })?;
    Ok(())
}

/// 用主密码解锁凭据
pub fn load_creds(dir: &Path, password: &str) -> Result<Creds, VaultError> {
    let path = dir.join(VAULT_FILE);
    let buf = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => VaultError::NotFound,
        _ => VaultError::Io(e),
    })?;
    // 按魔数识别格式版本：V1 旧参数仅用于读取，保存时一律升级为 V2
    let (version, magic): (u8, &[u8]) = if buf.starts_with(MAGIC_V2) {
        (2, MAGIC_V2)
    } else if buf.starts_with(MAGIC_V1) {
        (1, MAGIC_V1)
    } else {
        return Err(VaultError::Corrupt);
    };
    if buf.len() < MAGIC_V2.len() + SALT_LEN + NONCE_LEN {
        return Err(VaultError::Corrupt);
    }
    let mut off = MAGIC_V2.len();
    let salt = &buf[off..off + SALT_LEN];
    off += SALT_LEN;
    let nonce = &buf[off..off + NONCE_LEN];
    off += NONCE_LEN;
    let ct = &buf[off..];

    let key = kdf::derive_key(password, salt, version).map_err(|_| VaultError::BadPassword)?;
    let cipher = Aes256Gcm::new_from_slice(&key[..]).map_err(|_| VaultError::Corrupt)?;
    let pt = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: magic,
                },
            )
            .map_err(|_| VaultError::BadPassword)?,
    );
    let plain: &[u8] = &pt;
    serde_json::from_slice(plain).map_err(|_| VaultError::Corrupt)
}

pub fn vault_exists(dir: &Path) -> bool {
    dir.join(VAULT_FILE).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "btp_vault_test_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 旧版 V1 凭据库（Argon2 v1 参数）必须仍能解锁
    #[test]
    fn v1_vault_still_loads() {
        use aes_gcm::aead::{Aead, KeyInit, Payload};
        let dir = tmp_dir("v1");
        let creds = Creds {
            api_key: "k1".into(),
            api_secret: "s1".into(),
        };
        let salt = [7u8; SALT_LEN];
        let key = kdf::derive_key("pw", &salt, 1).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key[..]).unwrap();
        let nonce = [3u8; NONCE_LEN];
        let pt = serde_json::to_vec(&creds).unwrap();
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &pt,
                    aad: MAGIC_V1,
                },
            )
            .unwrap();
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC_V1);
        buf.extend_from_slice(&salt);
        buf.extend_from_slice(&nonce);
        buf.extend_from_slice(&ct);
        std::fs::write(dir.join(VAULT_FILE), buf).unwrap();
        let got = load_creds(&dir, "pw").unwrap();
        assert_eq!(got.api_key, "k1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn roundtrip_and_wrong_password() {
        let dir = tmp_dir("rt");
        let creds = Creds {
            api_key: "key123".into(),
            api_secret: "secret456".into(),
        };
        assert!(!vault_exists(&dir));
        save_creds(&dir, "主密码!", &creds).unwrap();
        assert!(vault_exists(&dir));
        let got = load_creds(&dir, "主密码!").unwrap();
        assert_eq!(got.api_key, "key123");
        assert_eq!(got.api_secret, "secret456");

        match load_creds(&dir, "错密码") {
            Err(VaultError::BadPassword) => {}
            other => panic!("期望 BadPassword，得到 {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
