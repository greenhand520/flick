//! 设置服务：schema 注册、读写、revision 与变更订阅。
//!
//! 本层负责**业务校验**（键是否存在、值是否符合声明），物理读写委托给
//! [`flick_data::StorageManager`]。所有方法都是同步的（SQLite 是阻塞 IO），
//! 调用方（daemon / IPC）应放到 blocking 线程池执行。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use flick_data::{DeclDiff, StorageManager};
use flick_domain::settings::{
    SettingChange, SettingDecl, SettingKey, SettingNamespace, SettingSchema, SettingSnapshot,
    SettingValue,
};
use tokio::sync::broadcast;

use crate::error::SettingsError;

/// 变更广播通道容量。
const CHANGE_CHANNEL_CAPACITY: usize = 256;

/// 一次 schema 注册的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct RegisterOutcome {
    /// 命名空间。
    pub namespace: String,
    /// 注册后的 revision。
    pub revision: u64,
    /// 指纹是否发生变化（false 表示声明缓存被跳过）。
    pub changed: bool,
    /// 声明差异（仅 `changed == true` 时有内容）。
    pub diff: DeclDiff,
}

/// 设置服务。
pub struct SettingsService {
    storage: Arc<StorageManager>,
    schemas: RwLock<HashMap<String, SettingSchema>>,
    changes: broadcast::Sender<SettingChange>,
}

impl SettingsService {
    /// 用给定存储构造服务。
    pub fn new(storage: Arc<StorageManager>) -> Self {
        let (changes, _receiver) = broadcast::channel(CHANGE_CHANNEL_CAPACITY);
        Self {
            storage,
            schemas: RwLock::new(HashMap::new()),
            changes,
        }
    }

    /// 底层存储句柄（供 daemon 做维护任务）。
    pub fn storage(&self) -> &Arc<StorageManager> {
        &self.storage
    }

    /// 注册一个 schema。
    ///
    /// 指纹未变时跳过声明写入（避免每次启动都做一次全表 upsert）。
    /// 插件命名空间会同时更新 `plugins` 表的版本与指纹。
    pub fn register_schema(
        &self,
        schema: SettingSchema,
        installed_version: &str,
        decl_version: &str,
    ) -> Result<RegisterOutcome, SettingsError> {
        schema.validate().map_err(|err| SettingsError::InvalidSchema {
            namespace: schema.namespace.as_str().to_string(),
            reason: err.to_string(),
        })?;
        let namespace = schema.namespace.as_str().to_string();
        let fingerprint = schema.fingerprint();

        let previous = match &schema.namespace {
            SettingNamespace::Plugin(_) => self.storage.plugin_meta(&namespace)?,
            SettingNamespace::Core => None,
        };
        let changed = match &previous {
            Some(meta) => meta.schema_fingerprint != fingerprint,
            None => true,
        };

        let diff = if changed {
            self.storage
                .replace_decls(&namespace, &schema.declarations, schema.schema_version)?
        } else {
            DeclDiff::default()
        };

        if matches!(schema.namespace, SettingNamespace::Plugin(_)) {
            self.storage.upsert_plugin(
                &namespace,
                installed_version,
                decl_version,
                &fingerprint,
            )?;
        }

        if changed {
            tracing::info!(
                namespace = %namespace,
                added = diff.added.len(),
                removed = diff.removed.len(),
                renamed = diff.renamed.len(),
                "设置声明已更新"
            );
        }

        self.schemas
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(namespace.clone(), schema);

        let revision = self.storage.revision(&namespace)?;
        Ok(RegisterOutcome {
            namespace,
            revision,
            changed,
            diff,
        })
    }

    /// 已注册的命名空间列表（排序后返回，便于诊断输出稳定）。
    pub fn registered_namespaces(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .schemas
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// 取某命名空间的 schema 副本。
    pub fn schema(&self, namespace: &str) -> Option<SettingSchema> {
        self.schemas
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(namespace)
            .cloned()
    }

    /// 读取整个命名空间的已设置值。
    pub fn list(&self, namespace: &str) -> Result<SettingSnapshot, SettingsError> {
        let parsed = namespace_from_str(namespace)
            .ok_or_else(|| SettingsError::UnknownNamespace(namespace.to_string()))?;
        let schema = self.require_schema(namespace)?;
        let values = self.storage.list_values(namespace, &schema.declarations)?;
        let revision = self.storage.revision(namespace)?;
        Ok(SettingSnapshot {
            namespace: parsed,
            revision,
            values,
        })
    }

    /// 读取单个值（未设置时返回 `None`）。
    pub fn get(&self, key: &SettingKey) -> Result<Option<SettingValue>, SettingsError> {
        let decl = self.declaration(key)?;
        Ok(self.storage.get_value(
            key.namespace.as_str(),
            &key.key,
            &decl.kind,
            decl.secret,
        )?)
    }

    /// 读取单个值并应用「存储值 → 声明默认 → 类型回退」的解析顺序。
    pub fn resolved(&self, key: &SettingKey) -> Result<SettingValue, SettingsError> {
        let decl = self.declaration(key)?;
        if let Some(value) = self.storage.get_value(
            key.namespace.as_str(),
            &key.key,
            &decl.kind,
            decl.secret,
        )? {
            return Ok(value);
        }
        Ok(decl
            .default
            .clone()
            .unwrap_or_else(|| decl.kind.fallback_value()))
    }

    /// 写入一个值，返回新 revision。
    pub fn set(&self, key: &SettingKey, value: SettingValue) -> Result<u64, SettingsError> {
        let decl = self.declaration(key)?;
        decl.kind
            .validate(&value)
            .map_err(|err| SettingsError::InvalidValue {
                key: key.key.clone(),
                reason: err.to_string(),
            })?;
        let revision = self.storage.set_value(
            key.namespace.as_str(),
            &key.key,
            &decl.kind,
            &value,
            decl.secret,
        )?;
        self.publish(&key.namespace, revision);
        Ok(revision)
    }

    /// 删除单个键，返回新 revision。
    pub fn reset_key(&self, key: &SettingKey) -> Result<u64, SettingsError> {
        self.declaration(key)?;
        let revision = self.storage.reset_key(key.namespace.as_str(), &key.key)?;
        self.publish(&key.namespace, revision);
        Ok(revision)
    }

    /// 删除整个命名空间的值（保留声明），返回新 revision。
    pub fn reset_namespace(&self, namespace: &str) -> Result<u64, SettingsError> {
        self.require_schema(namespace)?;
        let parsed = namespace_from_str(namespace)
            .ok_or_else(|| SettingsError::UnknownNamespace(namespace.to_string()))?;
        let revision = self.storage.reset_namespace(namespace)?;
        self.publish(&parsed, revision);
        Ok(revision)
    }

    /// 订阅变更通知。
    pub fn subscribe(&self) -> broadcast::Receiver<SettingChange> {
        self.changes.subscribe()
    }

    fn publish(&self, namespace: &SettingNamespace, revision: u64) {
        // 没有订阅者时 send 会失败，这属于正常情况，忽略即可。
        let _ = self.changes.send(SettingChange {
            namespace: namespace.clone(),
            revision,
        });
    }

    fn require_schema(&self, namespace: &str) -> Result<SettingSchema, SettingsError> {
        self.schema(namespace)
            .ok_or_else(|| SettingsError::UnknownNamespace(namespace.to_string()))
    }

    fn declaration(&self, key: &SettingKey) -> Result<SettingDecl, SettingsError> {
        let namespace = key.namespace.as_str().to_string();
        let schema = self.require_schema(&namespace)?;
        schema
            .declaration(&key.key)
            .cloned()
            .ok_or(SettingsError::UnknownKey {
                namespace,
                key: key.key.clone(),
            })
    }
}

/// 把命名空间字符串（`core` / `plugin.<id>`）解析为 [`SettingNamespace`]。
fn namespace_from_str(namespace: &str) -> Option<SettingNamespace> {
    SettingNamespace::parse(namespace).ok()
}
