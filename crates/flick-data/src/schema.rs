//! 数据库 schema 与迁移。
//!
//! 使用 `PRAGMA user_version` 记录 schema 版本；每次 `migrate` 只做向前迁移。

use rusqlite::Connection;

use crate::error::DataError;

/// 当前程序支持的数据库 schema 版本。
pub const SCHEMA_VERSION: i64 = 1;

/// 打开连接后需要统一设置的 PRAGMA（见设计文档 §7.2 / §10.1）。
pub fn apply_pragmas(conn: &Connection) -> Result<(), DataError> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

/// 建表语句（version 1）。
const MIGRATION_V1: &str = "
CREATE TABLE IF NOT EXISTS plugins (
    plugin_id          TEXT PRIMARY KEY,
    installed_version  TEXT NOT NULL DEFAULT '',
    decl_version       TEXT NOT NULL DEFAULT '',
    schema_fingerprint TEXT NOT NULL DEFAULT '',
    status             TEXT NOT NULL DEFAULT 'installed',
    uninstalled_at     TEXT,
    updated_at         TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS setting_decls (
    namespace      TEXT NOT NULL,
    key            TEXT NOT NULL,
    label          TEXT NOT NULL DEFAULT '',
    description    TEXT NOT NULL DEFAULT '',
    type_tag       TEXT NOT NULL,
    min_value      INTEGER,
    max_value      INTEGER,
    variants_json  TEXT,
    default_tag    TEXT,
    default_text   TEXT,
    secret         INTEGER NOT NULL DEFAULT 0,
    schema_version INTEGER NOT NULL DEFAULT 0,
    renamed_from   TEXT,
    state          TEXT NOT NULL DEFAULT 'active',
    PRIMARY KEY (namespace, key)
);

CREATE TABLE IF NOT EXISTS setting_values (
    namespace  TEXT NOT NULL,
    key        TEXT NOT NULL,
    type_tag   TEXT NOT NULL,
    value_text TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (namespace, key)
);

CREATE TABLE IF NOT EXISTS secret_values (
    namespace      TEXT NOT NULL,
    key            TEXT NOT NULL,
    key_id         TEXT NOT NULL,
    crypto_version INTEGER NOT NULL,
    nonce          BLOB NOT NULL,
    ciphertext     BLOB NOT NULL,
    revision       INTEGER NOT NULL,
    updated_at     TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (namespace, key)
);

CREATE TABLE IF NOT EXISTS namespaces (
    namespace      TEXT PRIMARY KEY,
    schema_version INTEGER NOT NULL DEFAULT 0,
    revision       INTEGER NOT NULL DEFAULT 0,
    updated_at     TEXT NOT NULL DEFAULT ''
);
";

/// 迁移数据库到 [`SCHEMA_VERSION`]。
pub fn migrate(conn: &Connection) -> Result<(), DataError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(DataError::UnsupportedSchema(current));
    }
    if current < 1 {
        conn.execute_batch(MIGRATION_V1)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    }
    Ok(())
}
