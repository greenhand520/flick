//! 设置服务的错误类型。

use flick_data::DataError;
use flick_domain::settings::SettingValueError;
use thiserror::Error;

/// `flick-settings` 的统一错误。
#[derive(Debug, Error)]
pub enum SettingsError {
    /// 命名空间未注册（没有对应 schema）。
    #[error("未知设置命名空间：{0}")]
    UnknownNamespace(String),
    /// 键没有在 schema 中声明。
    #[error("未知设置键：{namespace}.{key}")]
    UnknownKey {
        /// 命名空间。
        namespace: String,
        /// 短 key。
        key: String,
    },
    /// 值不符合声明类型（类型、范围或枚举取值不合法）。
    #[error("设置值不合法：{key}（{reason}）")]
    InvalidValue {
        /// 短 key。
        key: String,
        /// 具体原因。
        reason: String,
    },
    /// 声明本身不合法。
    #[error("设置声明不合法：{namespace}（{reason}）")]
    InvalidSchema {
        /// 命名空间。
        namespace: String,
        /// 具体原因。
        reason: String,
    },
    /// 存储中的值与当前声明不符（多为声明被不兼容地修改）。
    #[error("存储的设置值与声明不符：{key}（{reason}）")]
    CorruptValue {
        /// 短 key。
        key: String,
        /// 具体原因。
        reason: String,
    },
    /// schema 版本高于程序支持的版本。
    #[error("设置 schema 版本不受支持：{namespace} v{version}")]
    SchemaVersionUnsupported {
        /// 命名空间。
        namespace: String,
        /// 遇到的版本号。
        version: u32,
    },
    /// 拒绝写入（例如插件尝试写 `core` 命名空间）。
    #[error("不允许写入设置命名空间：{namespace}")]
    PermissionDenied {
        /// 命名空间。
        namespace: String,
    },
    /// 变更订阅通道已关闭。
    #[error("设置变更订阅通道已关闭")]
    SubscriptionClosed,
    /// 底层存储错误。
    #[error("设置存储错误：{0}")]
    Storage(#[from] DataError),
}

impl From<SettingValueError> for SettingsError {
    fn from(err: SettingValueError) -> Self {
        Self::InvalidSchema {
            namespace: String::new(),
            reason: err.to_string(),
        }
    }
}
