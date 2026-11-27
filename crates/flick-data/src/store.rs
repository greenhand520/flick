//! SQLite 存储实现：声明缓存、明文值、密文值、revision 与插件状态。
//!
//! 本模块是**物理层**：只做读写与事务，不做业务校验（那属于 `flick-settings`）。
//! 所有写操作在单个事务内完成（`BEGIN IMMEDIATE`），revision 用
//! `UPDATE ... revision = revision + 1` 原子自增。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use flick_domain::settings::{SettingDecl, SettingKind, SettingValue};
use rusqlite::{Connection, OptionalExtension, params};

use crate::crypto::{self, EncryptedSecret, SharedKeySource};
use crate::error::DataError;
use crate::paths::DataDirectories;
use crate::schema;

/// 从数据库读出的声明及其 schema 版本。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredDecl {
    /// 声明本身。
    pub decl: SettingDecl,
    /// 写入该声明时的 schema 版本。
    pub schema_version: i64,
    /// 声明状态：`active` / `orphaned`（manifest 已删）/ `retired`（被改名的旧 key）。
    pub state: String,
}

/// 一次声明注册带来的差异（供日志与诊断）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeclDiff {
    /// 新增的短 key。
    pub added: Vec<String>,
    /// 变为 orphaned 的短 key。
    pub removed: Vec<String>,
    /// 因 `renamed_from` 迁移的 `(旧 key, 新 key)`。
    pub renamed: Vec<(String, String)>,
}

impl DeclDiff {
    /// 是否没有任何变化。
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.renamed.is_empty()
    }
}

/// 插件元数据行。
#[derive(Debug, Clone, PartialEq)]
pub struct PluginMeta {
    /// 当前安装版本（给人看）。
    pub installed_version: String,
    /// 当前声明来自的插件版本。
    pub decl_version: String,
    /// 声明指纹。
    pub schema_fingerprint: String,
    /// `installed` / `uninstalled`。
    pub status: String,
    /// 进入 uninstalled 的时间（Unix 秒字符串）。
    pub uninstalled_at: Option<String>,
}

/// 存储管理器：持有 [`DataDirectories`]、SQLite 连接与主密钥来源。
pub struct StorageManager {
    dirs: DataDirectories,
    conn: Mutex<Connection>,
    keys: SharedKeySource,
}

impl StorageManager {
    /// 在给定目录上打开（必要时创建）`core.sqlite3` 并执行迁移。
    pub fn open(dirs: DataDirectories, keys: SharedKeySource) -> Result<Self, DataError> {
        dirs.ensure_config_root()?;
        let path = dirs.config_db();
        let conn = Connection::open(&path)?;
        schema::apply_pragmas(&conn)?;
        schema::migrate(&conn)?;
        Ok(Self {
            dirs,
            conn: Mutex::new(conn),
            keys,
        })
    }

    /// 按平台默认目录打开；主密钥优先取 `FLICK_MASTER_KEY`，否则不可用。
    pub fn open_default() -> Result<Self, DataError> {
        let dirs = DataDirectories::discover()?;
        Self::open(dirs, crate::crypto::default_master_key_source())
    }

    /// 物理目录句柄。
    pub fn directories(&self) -> &DataDirectories {
        &self.dirs
    }

    /// 数据库文件路径。
    pub fn config_db_path(&self) -> PathBuf {
        self.dirs.config_db()
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // 互斥锁被毒化时仍取回内部数据，避免把一次 panic 扩大成永久不可用。
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ── 插件元数据 ────────────────────────────────────────────────

    /// 写入/更新插件元数据（安装或重新注册时调用）。
    pub fn upsert_plugin(
        &self,
        plugin_id: &str,
        installed_version: &str,
        decl_version: &str,
        schema_fingerprint: &str,
    ) -> Result<(), DataError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO plugins
                (plugin_id, installed_version, decl_version, schema_fingerprint, status, uninstalled_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'installed', NULL, ?5)
             ON CONFLICT(plugin_id) DO UPDATE SET
                installed_version = excluded.installed_version,
                decl_version = excluded.decl_version,
                schema_fingerprint = excluded.schema_fingerprint,
                status = 'installed',
                uninstalled_at = NULL,
                updated_at = excluded.updated_at",
            params![plugin_id, installed_version, decl_version, schema_fingerprint, now_text()],
        )?;
        Ok(())
    }

    /// 读取插件元数据。
    pub fn plugin_meta(&self, plugin_id: &str) -> Result<Option<PluginMeta>, DataError> {
        let conn = self.conn();
        let meta = conn
            .query_row(
                "SELECT installed_version, decl_version, schema_fingerprint, status, uninstalled_at
                 FROM plugins WHERE plugin_id = ?1",
                params![plugin_id],
                |row| {
                    Ok(PluginMeta {
                        installed_version: row.get(0)?,
                        decl_version: row.get(1)?,
                        schema_fingerprint: row.get(2)?,
                        status: row.get(3)?,
                        uninstalled_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(meta)
    }

    /// 标记插件为已卸载（软删除，数据保留）。
    pub fn mark_uninstalled(&self, plugin_id: &str) -> Result<(), DataError> {
        let conn = self.conn();
        conn.execute(
            "UPDATE plugins SET status = 'uninstalled', uninstalled_at = ?2, updated_at = ?2
             WHERE plugin_id = ?1",
            params![plugin_id, now_text()],
        )?;
        Ok(())
    }

    /// 清理软卸载超过 `days` 天的插件数据：删声明/值/密文行、删插件行、删数据目录。
    ///
    /// 返回被清理的插件 id 列表。
    pub fn purge_uninstalled_older_than(&self, days: u64) -> Result<Vec<String>, DataError> {
        let threshold = now_secs().saturating_sub(days.saturating_mul(86_400));
        let victims: Vec<String> = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT plugin_id FROM plugins
                 WHERE status = 'uninstalled' AND uninstalled_at IS NOT NULL
                   AND CAST(uninstalled_at AS INTEGER) <= ?1",
            )?;
            let rows = stmt.query_map(params![threshold as i64], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for plugin_id in &victims {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            for table in [
                "setting_decls",
                "setting_values",
                "secret_values",
                "namespaces",
            ] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE namespace = ?1"),
                    params![plugin_id],
                )?;
            }
            tx.execute(
                "DELETE FROM plugins WHERE plugin_id = ?1",
                params![plugin_id],
            )?;
            tx.commit()?;
            // 目录删除是幂等的；失败不阻断清理流程。
            let dir = self.dirs.data_root().join("plugins").join(plugin_id);
            let _ = std::fs::remove_dir_all(dir);
            let cache = self.dirs.cache_root().join("plugins").join(plugin_id);
            let _ = std::fs::remove_dir_all(cache);
        }
        Ok(victims)
    }

    // ── 声明缓存 ────────────────────────────────────────────────

    /// 读取某命名空间下的全部声明（含 orphaned / retired，供诊断）。
    pub fn load_decls(&self, namespace: &str) -> Result<Vec<StoredDecl>, DataError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT key, renamed_from, label, description, type_tag, min_value, max_value,
                    variants_json, default_tag, default_text, secret, schema_version, state
             FROM setting_decls WHERE namespace = ?1 ORDER BY key",
        )?;
        let rows = stmt.query_map(params![namespace], row_to_decl)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 按 `schema_fingerprint` 之外的信息做声明增量更新。
    ///
    /// 调用方负责先比对指纹；本方法只负责“把当前声明写进缓存”：
    /// 新增/更新声明行、把消失的 key 标记 orphaned、处理 `renamed_from` 值迁移。
    pub fn replace_decls(
        &self,
        namespace: &str,
        decls: &[SettingDecl],
        schema_version: u32,
    ) -> Result<DeclDiff, DataError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut diff = DeclDiff::default();
        let mut renamed_olds: Vec<String> = Vec::new();

        let existing: Vec<(String, String)> = {
            let mut stmt =
                tx.prepare("SELECT key, state FROM setting_decls WHERE namespace = ?1")?;
            let rows = stmt.query_map(params![namespace], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let old_keys: Vec<String> = existing
            .iter()
            .filter(|(_, state)| state != "retired")
            .map(|(key, _)| key.clone())
            .collect();

        for decl in decls {
            if !old_keys.iter().any(|key| key == &decl.key) {
                diff.added.push(decl.key.clone());
            }
            // renamed_from：把旧 key 的值迁到新 key，并把旧声明标记 retired。
            if let Some(old) = &decl.renamed_from {
                if old != &decl.key && old_keys.iter().any(|key| key == old) {
                    move_value_rows(&tx, namespace, old, &decl.key)?;
                    tx.execute(
                        "UPDATE setting_decls SET state = 'retired' WHERE namespace = ?1 AND key = ?2",
                        params![namespace, old],
                    )?;
                    diff.renamed.push((old.clone(), decl.key.clone()));
                    renamed_olds.push(old.clone());
                }
            }
            upsert_decl(&tx, namespace, decl, schema_version)?;
        }

        for old in &old_keys {
            if renamed_olds.iter().any(|renamed| renamed == old) {
                continue;
            }
            if !decls.iter().any(|decl| &decl.key == old) {
                tx.execute(
                    "UPDATE setting_decls SET state = 'orphaned' WHERE namespace = ?1 AND key = ?2",
                    params![namespace, old],
                )?;
                diff.removed.push(old.clone());
            }
        }

        tx.execute(
            "INSERT INTO namespaces (namespace, schema_version, revision, updated_at)
             VALUES (?1, ?2, 0, ?3)
             ON CONFLICT(namespace) DO UPDATE SET schema_version = excluded.schema_version,
                                                 updated_at = excluded.updated_at",
            params![namespace, schema_version as i64, now_text()],
        )?;
        tx.commit()?;
        Ok(diff)
    }

    // ── 值与密文 ────────────────────────────────────────────────

    /// 当前 namespace 的 revision（不存在为 0）。
    pub fn revision(&self, namespace: &str) -> Result<u64, DataError> {
        let conn = self.conn();
        let value: Option<i64> = conn
            .query_row(
                "SELECT revision FROM namespaces WHERE namespace = ?1",
                params![namespace],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.unwrap_or(0).max(0) as u64)
    }

    /// 读取一个值；secret 项从密文表解密。
    pub fn get_value(
        &self,
        namespace: &str,
        key: &str,
        kind: &SettingKind,
        secret: bool,
    ) -> Result<Option<SettingValue>, DataError> {
        if secret {
            return self.read_secret_value(namespace, key, kind);
        }
        let conn = self.conn();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT type_tag, value_text FROM setting_values WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(tag, text)| decode_value(kind, &tag, &text))
            .transpose()
    }

    /// 列出某命名空间下已设置的明文值与可解密 secret。
    pub fn list_values(
        &self,
        namespace: &str,
        decls: &[SettingDecl],
    ) -> Result<BTreeMap<String, SettingValue>, DataError> {
        let mut out = BTreeMap::new();
        for decl in decls {
            if let Some(value) = self.get_value(namespace, &decl.key, &decl.kind, decl.secret)? {
                out.insert(decl.key.clone(), value);
            }
        }
        Ok(out)
    }

    /// 写入一个值；secret 项加密后入 `secret_values`，`setting_values` 不落行。
    ///
    /// 返回递增后的新 revision。
    pub fn set_value(
        &self,
        namespace: &str,
        key: &str,
        kind: &SettingKind,
        value: &SettingValue,
        secret: bool,
    ) -> Result<u64, DataError> {
        kind.validate(value)
            .map_err(|err| DataError::InvalidStored(err.to_string()))?;
        let (type_tag, text) = encode_value(value)?;

        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let new_revision = bump_revision(&tx, namespace)?;
        if secret {
            let plaintext = text.as_bytes();
            let sealed = self.seal(namespace, key, new_revision, plaintext)?;
            tx.execute(
                "INSERT INTO secret_values
                    (namespace, key, key_id, crypto_version, nonce, ciphertext, revision, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(namespace, key) DO UPDATE SET
                    key_id = excluded.key_id, crypto_version = excluded.crypto_version,
                    nonce = excluded.nonce, ciphertext = excluded.ciphertext,
                    revision = excluded.revision, updated_at = excluded.updated_at",
                params![
                    namespace,
                    key,
                    sealed.key_id,
                    sealed.crypto_version,
                    sealed.nonce,
                    sealed.ciphertext,
                    new_revision as i64,
                    now_text()
                ],
            )?;
            tx.execute(
                "DELETE FROM setting_values WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
            )?;
        } else {
            tx.execute(
                "INSERT INTO setting_values (namespace, key, type_tag, value_text, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(namespace, key) DO UPDATE SET
                    type_tag = excluded.type_tag, value_text = excluded.value_text,
                    updated_at = excluded.updated_at",
                params![namespace, key, type_tag, text, now_text()],
            )?;
            tx.execute(
                "DELETE FROM secret_values WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
            )?;
        }
        tx.commit()?;
        Ok(new_revision)
    }

    /// 删除单个键的值（明文与密文都删），返回新 revision。
    pub fn reset_key(&self, namespace: &str, key: &str) -> Result<u64, DataError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM setting_values WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
        )?;
        tx.execute(
            "DELETE FROM secret_values WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
        )?;
        let new_revision = bump_revision(&tx, namespace)?;
        tx.commit()?;
        Ok(new_revision)
    }

    /// 删除整个命名空间的值（保留声明），返回新 revision。
    pub fn reset_namespace(&self, namespace: &str) -> Result<u64, DataError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM setting_values WHERE namespace = ?1",
            params![namespace],
        )?;
        tx.execute(
            "DELETE FROM secret_values WHERE namespace = ?1",
            params![namespace],
        )?;
        let new_revision = bump_revision(&tx, namespace)?;
        tx.commit()?;
        Ok(new_revision)
    }

    /// 用当前主密钥重加密全部 secret 行（换密钥时使用），返回处理行数。
    pub fn rotate_master_key(&self) -> Result<usize, DataError> {
        let mut conn = self.conn();
        let rows: Vec<(String, String, i64, Vec<u8>, Vec<u8>)> = {
            let mut stmt = conn
                .prepare("SELECT namespace, key, revision, nonce, ciphertext FROM secret_values")?;
            let iter = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })?;
            iter.collect::<Result<Vec<_>, _>>()?
        };
        let current = self.keys.load()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut count = 0usize;
        for (namespace, key, revision, nonce, ciphertext) in rows {
            let revision = revision.max(0) as u64;
            // 读出该行原本的 key_id，才能选对解密用的历史主密钥。
            let key_id: String = tx.query_row(
                "SELECT key_id FROM secret_values WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
                |row| row.get(0),
            )?;
            let old_master = if key_id == current.key_id {
                current.clone()
            } else {
                self.keys.load_by_id(&key_id).ok_or_else(|| {
                    DataError::SecretStoreUnavailable(format!("缺少历史主密钥 {key_id}，无法轮换"))
                })?
            };
            let secret = EncryptedSecret {
                key_id: key_id.clone(),
                crypto_version: 0,
                nonce,
                ciphertext,
            };
            let plaintext = crypto::decrypt(
                &old_master,
                namespace.as_str(),
                key.as_str(),
                revision,
                &secret,
            )?;
            let sealed = self.seal(&namespace, &key, revision, &plaintext)?;
            tx.execute(
                "UPDATE secret_values SET key_id = ?3, crypto_version = ?4, nonce = ?5,
                        ciphertext = ?6, updated_at = ?7
                 WHERE namespace = ?1 AND key = ?2",
                params![
                    namespace,
                    key,
                    sealed.key_id,
                    sealed.crypto_version,
                    sealed.nonce,
                    sealed.ciphertext,
                    now_text()
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    fn read_secret_value(
        &self,
        namespace: &str,
        key: &str,
        kind: &SettingKind,
    ) -> Result<Option<SettingValue>, DataError> {
        let conn = self.conn();
        let row: Option<(String, i32, Vec<u8>, Vec<u8>, i64)> = conn
            .query_row(
                "SELECT key_id, crypto_version, nonce, ciphertext, revision
                 FROM secret_values WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((key_id, crypto_version, nonce, ciphertext, revision)) = row else {
            return Ok(None);
        };
        let secret = EncryptedSecret {
            key_id,
            crypto_version,
            nonce,
            ciphertext,
        };
        let plaintext = self.open_secret(namespace, key, revision.max(0) as u64, &secret)?;
        let text = String::from_utf8(plaintext)
            .map_err(|_| DataError::InvalidStored("secret 明文不是 UTF-8".to_string()))?;
        decode_value(kind, kind.type_tag(), &text).map(Some)
    }

    fn seal(
        &self,
        namespace: &str,
        key: &str,
        revision: u64,
        plaintext: &[u8],
    ) -> Result<EncryptedSecret, DataError> {
        let master = self.keys.load()?;
        crypto::encrypt(&master, namespace, key, revision, plaintext)
    }

    fn open_secret(
        &self,
        namespace: &str,
        key: &str,
        revision: u64,
        secret: &EncryptedSecret,
    ) -> Result<Vec<u8>, DataError> {
        let master = self.keys.load()?;
        crypto::decrypt(&master, namespace, key, revision, secret)
    }
}

/// 事务内原子自增 revision 并返回新值。
fn bump_revision(tx: &rusqlite::Transaction<'_>, namespace: &str) -> Result<u64, DataError> {
    tx.execute(
        "INSERT INTO namespaces (namespace, schema_version, revision, updated_at)
         VALUES (?1, 0, 1, ?2)
         ON CONFLICT(namespace) DO UPDATE SET revision = revision + 1,
                                             updated_at = excluded.updated_at",
        params![namespace, now_text()],
    )?;
    let revision: i64 = tx.query_row(
        "SELECT revision FROM namespaces WHERE namespace = ?1",
        params![namespace],
        |row| row.get(0),
    )?;
    Ok(revision.max(0) as u64)
}

/// 把 `setting_values` / `secret_values` 中旧 key 的行迁移到新 key。
fn move_value_rows(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    old_key: &str,
    new_key: &str,
) -> Result<(), DataError> {
    tx.execute(
        "DELETE FROM setting_values WHERE namespace = ?1 AND key = ?2",
        params![namespace, new_key],
    )?;
    tx.execute(
        "UPDATE setting_values SET key = ?3 WHERE namespace = ?1 AND key = ?2",
        params![namespace, old_key, new_key],
    )?;
    tx.execute(
        "DELETE FROM secret_values WHERE namespace = ?1 AND key = ?2",
        params![namespace, new_key],
    )?;
    tx.execute(
        "UPDATE secret_values SET key = ?3 WHERE namespace = ?1 AND key = ?2",
        params![namespace, old_key, new_key],
    )?;
    Ok(())
}

/// 插入或更新一行声明。
fn upsert_decl(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    decl: &SettingDecl,
    schema_version: u32,
) -> Result<(), DataError> {
    let (min, max, variants) = match &decl.kind {
        SettingKind::Integer { min, max } => (Some(*min), Some(*max), None),
        SettingKind::Enum { variants } => (
            None,
            None,
            Some(serde_json::to_string(variants).unwrap_or_else(|_| "[]".to_string())),
        ),
        _ => (None, None, None),
    };
    let (default_tag, default_text) = match &decl.default {
        Some(value) => {
            let (tag, text) = encode_value(value)?;
            (Some(tag), Some(text))
        }
        None => (None, None),
    };
    tx.execute(
        "INSERT INTO setting_decls
            (namespace, key, label, description, type_tag, min_value, max_value, variants_json,
             default_tag, default_text, secret, schema_version, renamed_from, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'active')
         ON CONFLICT(namespace, key) DO UPDATE SET
            label = excluded.label, description = excluded.description,
            type_tag = excluded.type_tag, min_value = excluded.min_value,
            max_value = excluded.max_value, variants_json = excluded.variants_json,
            default_tag = excluded.default_tag, default_text = excluded.default_text,
            secret = excluded.secret, schema_version = excluded.schema_version,
            renamed_from = excluded.renamed_from, state = 'active'",
        params![
            namespace,
            decl.key,
            decl.label,
            decl.description,
            decl.kind.type_tag(),
            min,
            max,
            variants,
            default_tag,
            default_text,
            decl.secret as i64,
            schema_version as i64,
            decl.renamed_from,
        ],
    )?;
    Ok(())
}

/// 行 → [`StoredDecl`]。
fn row_to_decl(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredDecl> {
    let type_tag: String = row.get(4)?;
    let min: Option<i64> = row.get(5)?;
    let max: Option<i64> = row.get(6)?;
    let variants: Option<String> = row.get(7)?;
    let default_tag: Option<String> = row.get(8)?;
    let default_text: Option<String> = row.get(9)?;
    let kind = kind_from_row(&type_tag, min, max, variants.as_deref()).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let default = match (default_tag, default_text) {
        (Some(tag), Some(text)) => Some(decode_value(&kind, &tag, &text).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?),
        _ => None,
    };
    Ok(StoredDecl {
        decl: SettingDecl {
            key: row.get(0)?,
            renamed_from: row.get(1)?,
            label: row.get(2)?,
            description: row.get(3)?,
            kind,
            default,
            secret: row.get::<_, i64>(10)? != 0,
        },
        schema_version: row.get(11)?,
        state: row.get(12)?,
    })
}

/// 由存储列重建 [`SettingKind`]。
fn kind_from_row(
    type_tag: &str,
    min: Option<i64>,
    max: Option<i64>,
    variants: Option<&str>,
) -> Result<SettingKind, DataError> {
    match type_tag {
        "Bool" => Ok(SettingKind::Bool),
        "Integer" => Ok(SettingKind::Integer {
            min: min.unwrap_or(i64::MIN),
            max: max.unwrap_or(i64::MAX),
        }),
        "String" => Ok(SettingKind::String),
        "Enum" => Ok(SettingKind::Enum {
            variants: variants
                .map(|raw| serde_json::from_str(raw).unwrap_or_default())
                .unwrap_or_default(),
        }),
        "Path" => Ok(SettingKind::Path),
        "Json" => Ok(SettingKind::Json),
        other => Err(DataError::InvalidStored(format!("未知类型标签 {other}"))),
    }
}

/// 值 → `(type_tag, 文本)`。
fn encode_value(value: &SettingValue) -> Result<(String, String), DataError> {
    let tag = value.type_tag().to_string();
    let text = match value {
        SettingValue::Bool(v) => v.to_string(),
        SettingValue::Integer(v) => v.to_string(),
        SettingValue::String(v) | SettingValue::Enum(v) | SettingValue::Path(v) => v.clone(),
        SettingValue::Json(v) => {
            serde_json::to_string(v).map_err(|err| DataError::InvalidStored(err.to_string()))?
        }
    };
    Ok((tag, text))
}

/// `(type_tag, 文本)` → 值。
fn decode_value(kind: &SettingKind, tag: &str, text: &str) -> Result<SettingValue, DataError> {
    if tag != kind.type_tag() {
        return Err(DataError::InvalidStored(format!(
            "存储类型 {tag} 与声明类型 {} 不符",
            kind.type_tag()
        )));
    }
    match kind {
        SettingKind::Bool => match text {
            "true" => Ok(SettingValue::Bool(true)),
            "false" => Ok(SettingValue::Bool(false)),
            other => Err(DataError::InvalidStored(format!("非法布尔值 {other}"))),
        },
        SettingKind::Integer { .. } => text
            .parse::<i64>()
            .map(SettingValue::Integer)
            .map_err(|_| DataError::InvalidStored(format!("非法整数 {text}"))),
        SettingKind::String => Ok(SettingValue::String(text.to_string())),
        SettingKind::Enum { .. } => Ok(SettingValue::Enum(text.to_string())),
        SettingKind::Path => Ok(SettingValue::Path(text.to_string())),
        SettingKind::Json => serde_json::from_str(text)
            .map(SettingValue::Json)
            .map_err(|err| DataError::InvalidStored(err.to_string())),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn now_text() -> String {
    now_secs().to_string()
}
