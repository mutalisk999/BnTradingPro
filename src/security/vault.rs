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

pub const VAULT_FILE: &str = "vault.bin";
const MAGIC: &[u8; 5] = b"BTPV1";
const NONCE_LEN: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Creds {
    pub api_key: String,
    pub api_secret: String,
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
    let key = kdf::derive_key(password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    let nonce = {
        let mut n = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut n);
        n
    };
    let plaintext = serde_json::to_vec(creds).map_err(|e| e.to_string())?;
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: MAGIC,
            },
        )
        .map_err(|e| format!("加密失败: {e}"))?;

    let mut buf = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ct.len());
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&salt);
    buf.extend_from_slice(&nonce);
    buf.extend_from_slice(&ct);

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(VAULT_FILE);
    // 先写临时文件再原子替换，避免写入中断留下损坏的凭据库
    let tmp = dir.join(format!("{VAULT_FILE}.tmp"));
    std::fs::write(&tmp, buf).map_err(|e| format!("写入凭据文件失败({}): {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("替换凭据文件失败({}): {e}", path.display()))?;
    Ok(())
}

/// 用主密码解锁凭据
pub fn load_creds(dir: &Path, password: &str) -> Result<Creds, VaultError> {
    let path = dir.join(VAULT_FILE);
    let buf = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => VaultError::NotFound,
        _ => VaultError::Io(e),
    })?;
    if buf.len() < MAGIC.len() + SALT_LEN + NONCE_LEN || &buf[..MAGIC.len()] != MAGIC {
        return Err(VaultError::Corrupt);
    }
    let mut off = MAGIC.len();
    let salt = &buf[off..off + SALT_LEN];
    off += SALT_LEN;
    let nonce = &buf[off..off + NONCE_LEN];
    off += NONCE_LEN;
    let ct = &buf[off..];

    let key = kdf::derive_key(password, salt).map_err(|_| VaultError::BadPassword)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| VaultError::Corrupt)?;
    let pt = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: MAGIC,
            },
        )
        .map_err(|_| VaultError::BadPassword)?;
    serde_json::from_slice(&pt).map_err(|_| VaultError::Corrupt)
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
