//! `flick-data` — 物理存储设施。
//!
//! 职责（见 `设置系统设计与约束.md` §6.2）：
//! - 四个逻辑根（config/data/cache/runtime）的解析与每插件目录创建（[`paths`]）；
//! - 权威数据库 `core.sqlite3` 的连接、PRAGMA 与迁移（[`schema`]）；
//! - 五张共享表的读写与 revision 原子自增（[`store`]）；
//! - secret 值的 AEAD 加密与主密钥来源抽象（[`crypto`]）。
//!
//! 本 crate 只提供设施，不调用 `flick-settings`，也不做业务校验。

pub mod crypto;
pub mod error;
pub mod paths;
pub mod schema;
pub mod store;

pub use crypto::{
    AAD_PREFIX, CRYPTO_VERSION, EncryptedSecret, EnvMasterKeySource, FixedMasterKeySource,
    MasterKey, MasterKeySource, SharedKeySource, UnavailableMasterKeySource,
};
pub use error::DataError;
pub use paths::DataDirectories;
pub use schema::SCHEMA_VERSION;
pub use store::{DeclDiff, PluginMeta, StorageManager, StoredDecl};
