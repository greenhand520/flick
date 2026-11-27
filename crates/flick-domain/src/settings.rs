//! 设置系统的纯数据类型与纯校验逻辑。
//!
//! 本模块**只**包含跨层共享的纯数据与纯函数：不依赖 tokio、SQLite、XDG、
//! IPC 或 UI（见 `设置系统设计与约束.md` C1）。文件 I/O、持久化、并发与
//! 订阅一律放在 `flick-settings` / `flick-data`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 设置命名空间：Flick 自身（`core`）或某个插件（`plugin.<plugin_id>`）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SettingNamespace {
    /// Flick 自身设置，全限定前缀固定为 `core`。
    Core,
    /// 插件设置，附带插件 id（如 `flick.builtin.apps`）。
    Plugin(String),
}

impl SettingNamespace {
    /// `core` 命名空间的字符串标识。
    pub const CORE: &'static str = "core";

    /// 构造 Flick 自身命名空间。
    pub fn core() -> Self {
        Self::Core
    }

    /// 构造插件命名空间。
    pub fn plugin(id: impl Into<String>) -> Self {
        Self::Plugin(id.into())
    }

    /// 返回用于存储与拼键的字符串：`core` 或插件 id。
    pub fn as_str(&self) -> &str {
        match self {
            Self::Core => Self::CORE,
            Self::Plugin(id) => id.as_str(),
        }
    }

    /// 解析命名空间字符串；`core` 为自身，其余必须是合法插件 id。
    pub fn parse(input: &str) -> Result<Self, SettingValueError> {
        if input == Self::CORE {
            return Ok(Self::Core);
        }
        if !is_valid_plugin_id(input) {
            return Err(SettingValueError::InvalidNamespace {
                value: input.to_string(),
            });
        }
        Ok(Self::Plugin(input.to_string()))
    }

    /// 是否为 Flick 自身命名空间。
    pub fn is_core(&self) -> bool {
        matches!(self, Self::Core)
    }
}

/// 设置值的类型标签与约束；来自插件 manifest 或核心手写表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingKind {
    /// 布尔值。
    Bool,
    /// 整数，闭区间 `[min, max]`。
    Integer { min: i64, max: i64 },
    /// 字符串。
    String,
    /// 枚举，取值必须是 `variants` 之一。
    Enum { variants: Vec<String> },
    /// 文件路径，值用 `file://` URI 字符串表示。
    Path,
    /// 结构化 JSON 值。
    Json,
}

impl SettingKind {
    /// 存储层使用的稳定类型标签。
    pub fn type_tag(&self) -> &'static str {
        match self {
            Self::Bool => "Bool",
            Self::Integer { .. } => "Integer",
            Self::String => "String",
            Self::Enum { .. } => "Enum",
            Self::Path => "Path",
            Self::Json => "Json",
        }
    }

    /// 校验声明本身是否合法（与具体值无关）。
    ///
    /// 目前规则：`Enum` 的 variants 不能为空；`Integer` 必须 `min <= max`。
    pub fn validate_declaration(&self) -> Result<(), SettingValueError> {
        match self {
            Self::Enum { variants } if variants.is_empty() => Err(SettingValueError::EmptyVariants),
            Self::Integer { min, max } if min > max => Err(SettingValueError::InvalidRange {
                min: *min,
                max: *max,
            }),
            _ => Ok(()),
        }
    }

    /// 校验一个具体值是否符合本类型。
    pub fn validate(&self, value: &SettingValue) -> Result<(), SettingValueError> {
        match (self, value) {
            (Self::Bool, SettingValue::Bool(_)) => Ok(()),
            (Self::Integer { min, max }, SettingValue::Integer(v)) => {
                if v < min || v > max {
                    return Err(SettingValueError::IntegerOutOfRange {
                        value: *v,
                        min: *min,
                        max: *max,
                    });
                }
                Ok(())
            }
            (Self::String, SettingValue::String(_)) => Ok(()),
            (Self::Enum { variants }, SettingValue::Enum(v)) => {
                if variants.iter().any(|item| item == v) {
                    Ok(())
                } else {
                    Err(SettingValueError::UnknownVariant {
                        variant: v.clone(),
                    })
                }
            }
            (Self::Path, SettingValue::Path(_)) => Ok(()),
            (Self::Json, SettingValue::Json(_)) => Ok(()),
            (kind, value) => Err(SettingValueError::TypeMismatch {
                expected: kind.type_tag(),
                got: value.type_tag(),
            }),
        }
    }

    /// 声明未提供 `default` 时的回退值（见设计文档 §7.3）。
    pub fn fallback_value(&self) -> SettingValue {
        match self {
            Self::Bool => SettingValue::Bool(false),
            Self::Integer { min, .. } => SettingValue::Integer(*min),
            Self::String => SettingValue::String(String::new()),
            Self::Enum { variants } => {
                SettingValue::Enum(variants.first().cloned().unwrap_or_default())
            }
            Self::Path => SettingValue::Path(String::new()),
            Self::Json => SettingValue::Json(serde_json::Value::Null),
        }
    }
}

/// 设置的具体值；与 [`SettingKind`] 一一对应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SettingValue {
    /// 布尔值。
    Bool(bool),
    /// 整数。
    Integer(i64),
    /// 字符串。
    String(String),
    /// 枚举取值（必须命中声明的 variants）。
    Enum(String),
    /// 文件路径，`file://` URI 字符串。
    Path(String),
    /// 结构化 JSON 值。
    Json(serde_json::Value),
}

impl SettingValue {
    /// 与 [`SettingKind::type_tag`] 对应的类型标签。
    pub fn type_tag(&self) -> &'static str {
        match self {
            Self::Bool(_) => "Bool",
            Self::Integer(_) => "Integer",
            Self::String(_) => "String",
            Self::Enum(_) => "Enum",
            Self::Path(_) => "Path",
            Self::Json(_) => "Json",
        }
    }

    /// 取布尔值；类型不符返回 `None`。
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// 取整数值；类型不符返回 `None`。
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(v) => Some(*v),
            _ => None,
        }
    }

    /// 以字符串形式取值（String / Enum / Path）；JSON 与整数返回 `None`。
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(v) | Self::Enum(v) | Self::Path(v) => Some(v.as_str()),
            _ => None,
        }
    }
}

/// 一条设置声明：插件或核心在注册期提供的元数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingDecl {
    /// 命名空间内的短 key（插件设置不得含 `.`、不得以 `core` 开头）。
    pub key: String,
    /// key 改名时的旧 key；注册期据此把旧值迁到新 key。
    pub renamed_from: Option<String>,
    /// 展示用标题。
    pub label: String,
    /// 展示用描述；空串表示无描述。
    pub description: String,
    /// 值的类型与约束。
    pub kind: SettingKind,
    /// 声明默认值；缺省时按 [`SettingKind::fallback_value`] 回退。
    pub default: Option<SettingValue>,
    /// 是否敏感值（密文存储，绝不进明文表/日志/IPC）。
    pub secret: bool,
}

impl SettingDecl {
    /// 校验单条声明：key 规则、kind 合法性、default 与 kind 一致。
    ///
    /// 是否允许 `.`/`core` 前缀由命名空间决定，见 [`SettingSchema::validate`]。
    pub fn validate(&self) -> Result<(), SettingValueError> {
        if self.key.is_empty() {
            return Err(SettingValueError::EmptyKey);
        }
        self.kind.validate_declaration()?;
        if let Some(default) = &self.default {
            self.kind
                .validate(default)
                .map_err(|_| SettingValueError::DefaultKindMismatch {
                    key: self.key.clone(),
                })?;
        }
        Ok(())
    }
}

/// 一个命名空间的设置声明集合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingSchema {
    /// 声明所属命名空间。
    pub namespace: SettingNamespace,
    /// 该命名空间下的全部声明。
    pub declarations: Vec<SettingDecl>,
    /// schema 版本，必须单调不减。
    pub schema_version: u32,
}

impl SettingSchema {
    /// 校验整个 schema：命名空间、短 key 规则、短 key 唯一、逐条声明。
    pub fn validate(&self) -> Result<(), SettingValueError> {
        if let SettingNamespace::Plugin(id) = &self.namespace {
            if !is_valid_plugin_id(id) {
                return Err(SettingValueError::InvalidNamespace { value: id.clone() });
            }
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.declarations.len());
        for decl in &self.declarations {
            self.validate_key(&decl.key)?;
            if seen.contains(&decl.key.as_str()) {
                return Err(SettingValueError::DuplicateKey {
                    key: decl.key.clone(),
                });
            }
            seen.push(decl.key.as_str());
            decl.validate()?;
        }
        Ok(())
    }

    /// 校验短 key：插件命名空间下不得含 `.`、不得以 `core` 开头。
    fn validate_key(&self, key: &str) -> Result<(), SettingValueError> {
        if key.is_empty() {
            return Err(SettingValueError::EmptyKey);
        }
        if let SettingNamespace::Plugin(_) = self.namespace {
            if key.contains('.') {
                return Err(SettingValueError::InvalidKey {
                    key: key.to_string(),
                    reason: "短 key 不得包含 '.'".to_string(),
                });
            }
            if key == SettingNamespace::CORE || key.starts_with("core.") {
                return Err(SettingValueError::ReservedPrefix {
                    key: key.to_string(),
                });
            }
        }
        Ok(())
    }

    /// 按短 key 查找声明。
    pub fn declaration(&self, key: &str) -> Option<&SettingDecl> {
        self.declarations.iter().find(|decl| decl.key == key)
    }

    /// 计算 schema 指纹。
    ///
    /// 规范化序列化（字段按键名排序、无浮点、声明按 key 排序）后做 FNV-1a
    /// 64 位哈希，并带 `fp1|` 算法版本前缀；Rust 结构体字段顺序变化不会
    /// 影响结果（见设计文档 §10.1）。
    pub fn fingerprint(&self) -> String {
        let mut decls: Vec<String> = self
            .declarations
            .iter()
            .map(|decl| {
                let kind = match &decl.kind {
                    SettingKind::Bool => "{\"tag\":\"Bool\"}".to_string(),
                    SettingKind::Integer { min, max } => {
                        format!("{{\"tag\":\"Integer\",\"min\":{min},\"max\":{max}}}")
                    }
                    SettingKind::String => "{\"tag\":\"String\"}".to_string(),
                    SettingKind::Enum { variants } => {
                        let list = variants
                            .iter()
                            .map(|item| serde_json::to_string(item).unwrap_or_default())
                            .collect::<Vec<_>>()
                            .join(",");
                        format!("{{\"tag\":\"Enum\",\"variants\":[{list}]}}")
                    }
                    SettingKind::Path => "{\"tag\":\"Path\"}".to_string(),
                    SettingKind::Json => "{\"tag\":\"Json\"}".to_string(),
                };
                let default = decl
                    .default
                    .as_ref()
                    .map(|value| serde_json::to_string(value).unwrap_or_default())
                    .unwrap_or_else(|| "null".to_string());
                let renamed = decl
                    .renamed_from
                    .as_ref()
                    .map(|old| serde_json::to_string(old).unwrap_or_default())
                    .unwrap_or_else(|| "null".to_string());
                format!(
                    "{{\"key\":{},\"renamed_from\":{},\"label\":{},\"description\":{},\"kind\":{},\"default\":{},\"secret\":{}}}",
                    serde_json::to_string(&decl.key).unwrap_or_default(),
                    renamed,
                    serde_json::to_string(&decl.label).unwrap_or_default(),
                    serde_json::to_string(&decl.description).unwrap_or_default(),
                    kind,
                    default,
                    decl.secret
                )
            })
            .collect();
        decls.sort();
        let payload = format!(
            "{{\"namespace\":{},\"schema_version\":{},\"declarations\":[{}]}}",
            serde_json::to_string(self.namespace.as_str()).unwrap_or_default(),
            self.schema_version,
            decls.join(",")
        );
        format!("fp1|{:016x}", fnv1a64(payload.as_bytes()))
    }
}

/// 设置变更通知：订阅者据此知道「哪个命名空间变到了哪个 revision」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingChange {
    /// 发生变化的命名空间。
    pub namespace: SettingNamespace,
    /// 变更后的 namespace 级 revision。
    pub revision: u64,
}

/// 一次读取得到的一致快照：namespace、revision 与值集合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingSnapshot {
    /// 快照所属命名空间。
    pub namespace: SettingNamespace,
    /// 快照对应的 namespace 级 revision。
    pub revision: u64,
    /// 短 key → 值；未设置的项不在其中（读时回退声明默认值）。
    pub values: BTreeMap<String, SettingValue>,
}

impl SettingSnapshot {
    /// 按短 key 取值。
    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.values.get(key)
    }
}

/// 全限定设置键：命名空间 + 短 key。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SettingKey {
    /// 所属命名空间。
    pub namespace: SettingNamespace,
    /// 命名空间内的短 key（如 `max_recent`）。
    pub key: String,
}

impl SettingKey {
    /// 构造键。
    pub fn new(namespace: SettingNamespace, key: impl Into<String>) -> Self {
        Self {
            namespace,
            key: key.into(),
        }
    }

    /// 拼接全限定键：`plugin.<plugin_id>.<key>` / `core.<key>`。
    pub fn qualified(&self) -> String {
        match &self.namespace {
            SettingNamespace::Core => {
                format!("{}.{}", SettingNamespace::CORE, self.key)
            }
            SettingNamespace::Plugin(id) => format!("plugin.{id}.{}", self.key),
        }
    }

    /// 解析全限定键。
    ///
    /// - `core.<key>`：`<key>` 可含 `.`（如 `shortcut.show_search`）。
    /// - `plugin.<plugin_id>.<key>`：插件 id 可含 `.`，故按**最后一个** `.`
    ///   切分；短 key 不允许再含 `.`。
    pub fn parse(input: &str) -> Result<Self, SettingValueError> {
        if let Some(rest) = input.strip_prefix("core.") {
            if rest.is_empty() {
                return Err(SettingValueError::EmptyKey);
            }
            // core 的 key 可含 `.` 分段，但每段必须非空
            if rest.starts_with('.') || rest.ends_with('.') || rest.contains("..") {
                return Err(SettingValueError::InvalidKey {
                    key: input.to_string(),
                    reason: "key 分段为空".to_string(),
                });
            }
            return Ok(Self::new(SettingNamespace::Core, rest));
        }
        if let Some(rest) = input.strip_prefix("plugin.") {
            let Some((plugin_id, key)) = rest.rsplit_once('.') else {
                return Err(SettingValueError::InvalidKey {
                    key: input.to_string(),
                    reason: "缺少短 key 段".to_string(),
                });
            };
            if !is_valid_plugin_id(plugin_id) {
                return Err(SettingValueError::InvalidNamespace {
                    value: plugin_id.to_string(),
                });
            }
            if key.is_empty() || key.contains('.') {
                return Err(SettingValueError::InvalidKey {
                    key: input.to_string(),
                    reason: "短 key 为空或包含 '.'".to_string(),
                });
            }
            return Ok(Self::new(SettingNamespace::Plugin(plugin_id.to_string()), key));
        }
        Err(SettingValueError::InvalidKey {
            key: input.to_string(),
            reason: "未知命名空间前缀（应为 core. 或 plugin.）".to_string(),
        })
    }
}

/// 设置相关校验错误；全部为纯数据错误，可安全序列化后跨层传递。
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum SettingValueError {
    /// 短 key 为空。
    #[error("setting key must not be empty")]
    EmptyKey,
    /// 短 key 格式非法。
    #[error("invalid setting key '{key}': {reason}")]
    InvalidKey { key: String, reason: String },
    /// 使用了保留前缀 `core`。
    #[error("setting key '{key}' uses reserved prefix 'core'")]
    ReservedPrefix { key: String },
    /// 插件 id 非法（字符集或保留名不满足要求）。
    #[error("invalid plugin id '{value}'")]
    InvalidNamespace { value: String },
    /// 值类型与声明不符。
    #[error("type mismatch: expected {expected}, got {got}")]
    TypeMismatch {
        expected: &'static str,
        got: &'static str,
    },
    /// 整数超出声明区间。
    #[error("integer {value} out of range [{min}, {max}]")]
    IntegerOutOfRange { value: i64, min: i64, max: i64 },
    /// 枚举取值不在 variants 内。
    #[error("unknown enum variant '{variant}'")]
    UnknownVariant { variant: String },
    /// 枚举声明没有 variants。
    #[error("enum setting must declare at least one variant")]
    EmptyVariants,
    /// 整数声明区间非法。
    #[error("invalid integer range: min {min} > max {max}")]
    InvalidRange { min: i64, max: i64 },
    /// 同一 namespace 内短 key 重复。
    #[error("duplicate setting key '{key}' in namespace")]
    DuplicateKey { key: String },
    /// 声明默认值与其声明类型不符。
    #[error("default value of '{key}' does not match its declared kind")]
    DefaultKindMismatch { key: String },
}

/// 校验插件 id：仅允许 `[A-Za-z0-9][A-Za-z0-9._-]*`，且拒绝保留名。
///
/// 该规则同时用于路径安全（目录名直接取 id）与命名空间解析。
pub fn is_valid_plugin_id(id: &str) -> bool {
    if id.is_empty() || id == "." || id == ".." || id == SettingNamespace::CORE {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
}

/// FNV-1a 64 位哈希；仅用于本地指纹，不作密码学用途。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int_decl(key: &str) -> SettingDecl {
        SettingDecl {
            key: key.to_string(),
            renamed_from: None,
            label: key.to_string(),
            description: String::new(),
            kind: SettingKind::Integer { min: 1, max: 100 },
            default: Some(SettingValue::Integer(20)),
            secret: false,
        }
    }

    #[test]
    fn integer_validation_rejects_out_of_range() {
        let kind = SettingKind::Integer { min: 1, max: 10 };
        assert_eq!(kind.validate(&SettingValue::Integer(5)), Ok(()));
        assert_eq!(
            kind.validate(&SettingValue::Integer(11)),
            Err(SettingValueError::IntegerOutOfRange {
                value: 11,
                min: 1,
                max: 10
            })
        );
        assert_eq!(
            kind.validate(&SettingValue::String("5".into())),
            Err(SettingValueError::TypeMismatch {
                expected: "Integer",
                got: "String"
            })
        );
    }

    #[test]
    fn fingerprint_is_stable_and_order_independent() {
        let a = SettingSchema {
            namespace: SettingNamespace::plugin("flick.test.a"),
            declarations: vec![int_decl("b"), int_decl("a")],
            schema_version: 1,
        };
        let mut b = a.clone();
        b.declarations.reverse();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert!(a.fingerprint().starts_with("fp1|"));
        let mut c = a.clone();
        c.declarations[0].secret = true;
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    #[test]
    fn plugin_schema_rejects_dotted_and_core_keys() {
        let mut decl = int_decl("max.recent");
        let schema = SettingSchema {
            namespace: SettingNamespace::plugin("flick.test.a"),
            declarations: vec![decl.clone()],
            schema_version: 1,
        };
        assert!(matches!(
            schema.validate(),
            Err(SettingValueError::InvalidKey { .. })
        ));
        decl.key = "core_limit".to_string();
        let schema = SettingSchema {
            namespace: SettingNamespace::plugin("flick.test.a"),
            declarations: vec![decl],
            schema_version: 1,
        };
        assert!(schema.validate().is_ok());
    }
}
