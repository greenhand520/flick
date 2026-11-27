//! secret 值的 AEAD 加解密与主密钥来源。
//!
//! 算法：XChaCha20-Poly1305（24 字节随机 nonce，禁止复用）；
//! AAD = `flick-settings-v1|<namespace>|<key>|<revision>`，
//! 其中 `revision` 必须是写入后的新 revision（见设计文档 §10.2）。
//!
//! 主密钥**只**来自系统钥匙串（S2 实现）；S0 提供 [`MasterKeySource`] 抽象、
//! 环境变量实现（测试/开发）与不可用实现（生产默认，返回结构化错误而非回退明文）。

use std::sync::Arc;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;

use crate::error::DataError;

/// 当前 AEAD 算法版本，写入 `secret_values.crypto_version`。
pub const CRYPTO_VERSION: i32 = 1;

/// AAD 前缀，用于把本系统的密文与其它用途区分开。
pub const AAD_PREFIX: &str = "flick-settings-v1";

/// 主密钥材料；`Debug` 不输出密钥字节。
#[derive(Clone)]
pub struct MasterKey {
    /// 主密钥标识，用于轮换与审计。
    pub key_id: String,
    /// 32 字节对称密钥。
    pub bytes: [u8; 32],
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field("key_id", &self.key_id)
            .field("bytes", &"<redacted>")
            .finish()
    }
}

/// 主密钥来源抽象。
pub trait MasterKeySource: Send + Sync {
    /// 读取当前主密钥；不可用时返回 [`DataError::SecretStoreUnavailable`]。
    fn load(&self) -> Result<MasterKey, DataError>;

    /// 按 `key_id` 取历史主密钥（密钥轮换时用于解密旧数据）。
    ///
    /// 默认实现返回 `None`，表示该来源不支持历史密钥（轮换会失败并报错，
    /// 绝不会用错密钥写出损坏数据）。
    fn load_by_id(&self, _key_id: &str) -> Option<MasterKey> {
        None
    }
}

/// 不可用实现：生产环境默认值，直到 S2 接入系统钥匙串。
#[derive(Debug, Default)]
pub struct UnavailableMasterKeySource;

impl MasterKeySource for UnavailableMasterKeySource {
    fn load(&self) -> Result<MasterKey, DataError> {
        Err(DataError::SecretStoreUnavailable(
            "系统钥匙串后端尚未接入（S2）".to_string(),
        ))
    }
}

/// 环境变量实现：`FLICK_MASTER_KEY`（64 位十六进制）与 `FLICK_MASTER_KEY_ID`。
///
/// 仅用于测试与本地开发；生产不允许把密钥放在环境变量里。
#[derive(Debug, Default)]
pub struct EnvMasterKeySource;

impl MasterKeySource for EnvMasterKeySource {
    fn load(&self) -> Result<MasterKey, DataError> {
        let raw = std::env::var("FLICK_MASTER_KEY").map_err(|_| {
            DataError::SecretStoreUnavailable("未设置 FLICK_MASTER_KEY".to_string())
        })?;
        let bytes = hex_decode(&raw)?;
        if bytes.len() != 32 {
            return Err(DataError::Crypto("主密钥必须是 32 字节".to_string()));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        let key_id = std::env::var("FLICK_MASTER_KEY_ID").unwrap_or_else(|_| "env-1".to_string());
        Ok(MasterKey { key_id, bytes: key })
    }
}

/// 固定主密钥实现（测试专用）。
#[derive(Debug, Clone)]
pub struct FixedMasterKeySource(pub MasterKey);

impl MasterKeySource for FixedMasterKeySource {
    fn load(&self) -> Result<MasterKey, DataError> {
        Ok(self.0.clone())
    }
}

/// 密文行：nonce 与 ciphertext 分开存储。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedSecret {
    /// 加密使用的主密钥标识。
    pub key_id: String,
    /// 算法版本。
    pub crypto_version: i32,
    /// 24 字节随机 nonce。
    pub nonce: Vec<u8>,
    /// 密文（含认证标签）。
    pub ciphertext: Vec<u8>,
}

/// 构造 AAD 字节串。
pub fn aad(namespace: &str, key: &str, revision: u64) -> Vec<u8> {
    format!("{AAD_PREFIX}|{namespace}|{key}|{revision}").into_bytes()
}

/// 用主密钥加密明文。
pub fn encrypt(
    master: &MasterKey,
    namespace: &str,
    key: &str,
    revision: u64,
    plaintext: &[u8],
) -> Result<EncryptedSecret, DataError> {
    let cipher = XChaCha20Poly1305::new_from_slice(&master.bytes)
        .map_err(|err| DataError::Crypto(err.to_string()))?;
    let mut nonce_bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let aad = aad(namespace, key, revision);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|err| DataError::Crypto(err.to_string()))?;
    Ok(EncryptedSecret {
        key_id: master.key_id.clone(),
        crypto_version: CRYPTO_VERSION,
        nonce: nonce_bytes.to_vec(),
        ciphertext,
    })
}

/// 用主密钥解密密文行。
pub fn decrypt(
    master: &MasterKey,
    namespace: &str,
    key: &str,
    revision: u64,
    secret: &EncryptedSecret,
) -> Result<Vec<u8>, DataError> {
    let cipher = XChaCha20Poly1305::new_from_slice(&master.bytes)
        .map_err(|err| DataError::Crypto(err.to_string()))?;
    if secret.nonce.len() != 24 {
        return Err(DataError::Crypto("nonce 长度非法".to_string()));
    }
    let aad = aad(namespace, key, revision);
    cipher
        .decrypt(
            XNonce::from_slice(&secret.nonce),
            Payload {
                msg: &secret.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|err| DataError::Crypto(err.to_string()))
}

/// 共享的主密钥来源句柄。
pub type SharedKeySource = Arc<dyn MasterKeySource>;

/// 按环境选择默认主密钥来源。
///
/// 设置了 `FLICK_MASTER_KEY` 时用 [`EnvMasterKeySource`]（测试/开发）；
/// 否则用 [`UnavailableMasterKeySource`]，明文设置仍可用，secret 读写返回结构化错误。
pub fn default_master_key_source() -> SharedKeySource {
    if std::env::var_os("FLICK_MASTER_KEY").is_some() {
        Arc::new(EnvMasterKeySource)
    } else {
        Arc::new(UnavailableMasterKeySource)
    }
}

fn hex_decode(raw: &str) -> Result<Vec<u8>, DataError> {
    let trimmed = raw.trim();
    if trimmed.len() % 2 != 0 {
        return Err(DataError::Crypto("十六进制长度必须为偶数".to_string()));
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let hi = (bytes[index] as char)
            .to_digit(16)
            .ok_or_else(|| DataError::Crypto("十六进制字符非法".to_string()))?;
        let lo = (bytes[index + 1] as char)
            .to_digit(16)
            .ok_or_else(|| DataError::Crypto("十六进制字符非法".to_string()))?;
        out.push(((hi << 4) | lo) as u8);
        index += 2;
    }
    Ok(out)
}
