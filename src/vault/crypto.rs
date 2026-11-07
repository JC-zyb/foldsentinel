use anyhow::{Result, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

const MAX_PASSWORD_BYTES: usize = 1024;
const KEY_BYTES: usize = 32;
const ARGON2_MEMORY_KIB: u32 = 19_456;
const ARGON2_ITERATIONS: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;

pub(super) fn validate_password(password: &str) -> Result<()> {
    ensure!(!password.is_empty(), "密码不能为空");
    ensure!(
        password.len() <= MAX_PASSWORD_BYTES,
        "密码最多 1024 个 UTF-8 字节"
    );
    Ok(())
}

pub(super) fn derive_key(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; KEY_BYTES]>> {
    let mut key = Zeroizing::new([0u8; KEY_BYTES]);
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(KEY_BYTES),
    )
    .map_err(|e| anyhow::anyhow!("KDF 参数错误: {e}"))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| anyhow::anyhow!("密钥派生失败: {e}"))?;
    Ok(key)
}
