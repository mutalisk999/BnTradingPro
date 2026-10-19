//! 主密码 → 256 位密钥派生（Argon2id）
//!
//! - V1：m=19MiB, t=2, p=1（OWASP 基线，用于解密旧版凭据库）
//! - V2：m=64MiB, t=3, p=1（当前保存格式，提高离线爆破成本）

use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

pub const SALT_LEN: usize = 16;
pub const KEY_LEN: usize = 32;

/// 派生 AES-256 密钥；Zeroizing 包裹，用完即从内存抹除。
/// `version` 为凭据库格式版本（1/2）
pub fn derive_key(
    password: &str,
    salt: &[u8],
    version: u8,
) -> Result<Zeroizing<[u8; KEY_LEN]>, String> {
    if salt.len() != SALT_LEN {
        return Err(crate::i18n::tr("salt 长度非法").into());
    }
    // (m_KiB, t, p)：V2 显著提高离线爆破成本，解锁耗时约 200ms 可接受
    let (m_kib, t, p) = match version {
        1 => (19_456, 2, 1),
        _ => (65_536, 3, 1),
    };
    let params = Params::new(m_kib, t, p, Some(KEY_LEN)).map_err(|e| e.to_string())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|e| format!("{}{e}", crate::i18n::tr("密钥派生失败: ")))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_salt_sensitive() {
        let s1 = [1u8; 16];
        let s2 = [2u8; 16];
        let a = derive_key("correct horse", &s1, 2).unwrap();
        let b = derive_key("correct horse", &s1, 2).unwrap();
        let c = derive_key("correct horse", &s2, 2).unwrap();
        let d = derive_key("wrong horse", &s1, 2).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn versions_differ() {
        let salt = [9u8; 16];
        let v1 = derive_key("pw", &salt, 1).unwrap();
        let v2 = derive_key("pw", &salt, 2).unwrap();
        assert_ne!(v1, v2);
    }
}
