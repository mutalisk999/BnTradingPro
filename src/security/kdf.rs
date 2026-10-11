//! 主密码 → 256 位密钥派生（Argon2id，OWASP 推荐参数：m=19MiB, t=2, p=1）

use argon2::{Algorithm, Argon2, Params, Version};

pub const SALT_LEN: usize = 16;
pub const KEY_LEN: usize = 32;

pub fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], String> {
    if salt.len() != SALT_LEN {
        return Err("salt 长度非法".into());
    }
    let params = Params::new(19_456, 2, 1, Some(KEY_LEN)).map_err(|e| e.to_string())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| format!("密钥派生失败: {e}"))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_salt_sensitive() {
        let s1 = [1u8; 16];
        let s2 = [2u8; 16];
        let a = derive_key("correct horse", &s1).unwrap();
        let b = derive_key("correct horse", &s1).unwrap();
        let c = derive_key("correct horse", &s2).unwrap();
        let d = derive_key("wrong horse", &s1).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }
}
