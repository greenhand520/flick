//! `flick-data` 的错误类型。

use thiserror::Error;

/// 物理存储层错误。
///
/// 该类型只描述存储设施自身的问题；业务级校验错误由 `flick-settings`
/// 负责映射为面向用户的错误。
#[derive(Debug, Error)]
pub enum DataError {
    /// SQLite 返回的错误。
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// 文件系统错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 插件 id 不合法（会直接用作目录名）。
    #[error("invalid plugin id '{0}'")]
    InvalidPluginId(String),

    /// 目标路径是符号链接，拒绝使用（防止插件把目录指向他人）。
    #[error("path '{path}' is a symlink")]
    Symlink { path: String },

    /// 无法确定跨平台项目目录。
    #[error("cannot determine project directories")]
    NoProjectDirs,

    /// 系统钥匙串不可用或主密钥缺失。
    #[error("secret store unavailable: {0}")]
    SecretStoreUnavailable(String),

    /// AEAD 加解密失败。
    #[error("crypto failure: {0}")]
    Crypto(String),

    /// 数据库中的值与声明类型不符或无法解析。
    #[error("invalid stored value: {0}")]
    InvalidStored(String),

    /// 数据库 schema 版本高于本程序支持的范围。
    #[error("unsupported database schema version {0}")]
    UnsupportedSchema(i64),
}
