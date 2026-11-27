# flick-data

Flick 的**物理存储设施**：解析四个逻辑根目录、管理 `core.sqlite3` 的连接与迁移、读写五张共享表并以事务原子自增 revision、为 secret 值提供 AEAD 加解密与主密钥来源抽象。

> 本 crate 只提供设施，不调用 `flick-settings`，也不做业务校验。业务级校验（键是否存在、值是否符合声明）由上层负责；物理层只做读写与事务。

## 在 workspace 中的位置 / 依赖关系

本 crate 位于 `crates/flick-data`，是 workspace 中较底层的基础设施，只依赖 `flick-domain` 与若干外部库，供上层业务库与宿主装配层调用。

- **直接依赖**：`flick-domain`（`settings::{SettingDecl, SettingKind, SettingValue}`、`is_valid_plugin_id`）；外部库 `rusqlite`、`directories`、`chacha20poly1305`、`rand`、`serde_json`、`thiserror`。
- **开发依赖**：`tempfile`（临时目录）。
- **反向依赖**：`flick-settings`（业务层，`crates/flick-settings/Cargo.toml` 中以 path 依赖本 crate）。

```
配置/数据/缓存/运行时根  ──► paths.rs
                                  │
core.sqlite3 ──► schema.rs（PRAGMA / 迁移）
                                  │
                                  ▼
                           store.rs（StorageManager）
                             ├─ 声明缓存 / 明文值 / revision
                             └─ secret 值 ──► crypto.rs（AEAD + 主密钥）
错误统一为 error.rs::DataError
```

## 为什么设计这个 crate

把「与业务无关的物理细节」从设置业务层剥离出来，形成一条清晰边界：

- **路径边界**：四个 XDG 逻辑根、每插件目录与符号链接防护统一在 `paths.rs` 解析。只有这个模块允许拼这些路径，上层不自己拼字符串（避免目录被插件指向他人、避免到处重复 `0700` 约定）。
- **数据库边界**：SQLite 连接、PRAGMA、`user_version` 向前迁移与五张表的 DDL 集中在 `schema.rs` / `store.rs`，业务层不需要知道表结构与 SQL。
- **加密边界**：secret 的 AEAD、nonce、AAD 与主密钥来源抽象集中在 `crypto.rs`；`StorageManager` 负责在写入事务中调用它，业务层只传 `decl.secret` 标志。
- 这样 `flick-settings` 可以只做 schema 注册 / 指纹短路 / 校验 / 默认值解析，宿主也能单独做迁移、密钥轮换和软删除清理，而不把 SQL 或加密知识泄漏到上层。

## 如何运行

**打开数据库**：上层调用 [`StorageManager::open(dirs, keys)`](src/store.rs#L73)（或 [`open_default()`](src/store.rs#L87)）。`open` 依次执行 `ensure_config_root()` → `Connection::open(<config>/core.sqlite3)` → [`apply_pragmas()`](src/schema.rs#L13) → [`migrate()`](src/schema.rs#L83)。迁移读取 `PRAGMA user_version`：高于 `SCHEMA_VERSION`（当前 1）报 `UnsupportedSchema`，低于 1 则执行 `MIGRATION_V1` 建表并写回版本号。`open_default` 按平台发现目录，若设置了 `FLICK_MASTER_KEY` 则用 `EnvMasterKeySource`，否则用 `UnavailableMasterKeySource`。

**写入事务**：所有写操作在单个 `BEGIN IMMEDIATE` 事务内完成。以 [`set_value()`](src/store.rs#L349) 为例：先 `kind.validate` + 编码，然后 [`bump_revision()`](src/store.rs#L565) 用 `INSERT … ON CONFLICT DO UPDATE revision = revision + 1` 原子递增，再按 `secret` 写表——secret 写入 `secret_values` 并删除 `setting_values` 对应行，非 secret 反之；最后提交并返回新 revision。`replace_decls` 也在事务内完成新增 / `renamed_from` 迁移 / `orphaned` 标记。`conn()` 在被毒化的 mutex 上取回内部数据，避免一次 panic 让存储永久不可用。

**secret 生命周期**：写入时用新 revision 构造 AAD 加密；读取时 [`read_secret_value`](src/store.rs#L503) 用当前主密钥解密；[`rotate_master_key()`](src/store.rs#L437) 逐行读出原 `key_id`，用 `load_by_id` 取历史密钥解密后再用当前密钥重加密。没有可用主密钥时返回 `DataError::SecretStoreUnavailable`，绝不回退明文。

**线程 / 阻塞模型**：`StorageManager` 用 `std::sync::Mutex<Connection>` 串行化访问；所有方法是同步阻塞 I/O，调用方（daemon / IPC）应放到 blocking 线程池，不要在 Tokio 异步任务中直接调用。

## 内部技术与架构细节

- **四个逻辑根**：[`DataDirectories`](src/paths.rs#L19) = `{config, data, cache, runtime}`。[`discover()`](src/paths.rs#L28) 用 `directories::ProjectDirs::from("", "", "flick")` 解析前三个；[`runtime_root()`](src/paths.rs#L142) 优先 `XDG_RUNTIME_DIR/flick`，否则 `temp_dir()/flick`。[`config_db()`](src/paths.rs#L76) = `<config>/core.sqlite3`；[`plugin_dir()`](src/paths.rs#L87) = `<data>/plugins/<id>`，unix 下 `0700` 且拒绝符号链接目标（[`ensure_dir()`](src/paths.rs#L121) 返回 `DataError::Symlink`）；`validate_plugin_id()` 复用 `flick_domain::settings::is_valid_plugin_id` 并额外拒绝 `/` 与 `\`。
- **PRAGMA 与五张表**：[`apply_pragmas()`](src/schema.rs#L13) 设置 `journal_mode = WAL`、`busy_timeout = 5000`、`foreign_keys = ON`、`synchronous = NORMAL`。[`MIGRATION_V1`](src/schema.rs#L24) 建五张表：`plugins`（版本 / 指纹 / 状态 / 卸载时间）、`setting_decls`（声明、类型与默认、`renamed_from`、`state`）、`setting_values`（明文）、`secret_values`（`key_id` / `crypto_version` / `nonce` / `ciphertext` / `revision`）、`namespaces`（`schema_version` / `revision`）。
- **声明状态机**：`StoredDecl.state` 为 `active` / `orphaned`（manifest 已删该 key）/ `retired`（被 `renamed_from` 迁移的旧 key）。[`replace_decls()`](src/store.rs#L226) 在一个事务里算出 `DeclDiff { added, removed, renamed }`：`renamed_from` 命中时调用 `move_value_rows` 把值从旧 key 迁到新 key、旧声明标记 `retired`；消失的 key 标记 `orphaned`；最后 upsert `namespaces` 的 `schema_version`。
- **类型标签与校验**：`upsert_decl` 把 `SettingKind` 映射为 `type_tag` / `min` / `max` / `variants_json` / 默认值；`kind_from_row` 识别 `Bool` / `Integer` / `String` / `Enum` / `Path` / `Json` 六种标签；`decode_value` 要求存储标签与声明标签一致，否则 `DataError::InvalidStored`。`set_value` 也调用 `kind.validate` 做最后一道物理层防线。
- **AEAD**：算法为 `XChaCha20-Poly1305`（类型 `XChaCha20Poly1305`），24 字节随机 nonce 禁止复用；`CRYPTO_VERSION = 1`（[crypto.rs:19](src/crypto.rs#L19)）。AAD = `flick-settings-v1|<namespace>|<key>|<revision>`，其中 `revision` 是**写入后的新值**（[crypto.rs:4](src/crypto.rs#L4)）。[`MasterKey`](src/crypto.rs#L26) 含 `key_id` 与 32 字节 bytes，自定义 `Debug` 会把字节打印为 `<redacted>`；[`MasterKeySource`](src/crypto.rs#L43) 的 `load_by_id` 默认返回 `None`（不支持历史密钥时轮换直接报错，不会用错密钥写坏数据）；[`EnvMasterKeySource`](src/crypto.rs#L72) 读 `FLICK_MASTER_KEY`（64 位十六进制）与可选 `FLICK_MASTER_KEY_ID`（默认 `env-1`）。
- **清理**：[`purge_uninstalled_older_than(days)`](src/store.rs#L175) 先按时间戳挑出软卸载超期的插件，事务内删四张表中的命名空间行与 `plugins` 行，再幂等删除 data / cache 插件目录（删目录失败不阻断）。错误统一为 [`DataError`](src/error.rs#L10)：`Sqlite` / `Io` / `InvalidPluginId` / `Symlink` / `NoProjectDirs` / `SecretStoreUnavailable` / `Crypto` / `InvalidStored` / `UnsupportedSchema`。

## 公开 API / 核心类型

`lib.rs` 重导出了全部对外类型；模块与关键入口如下。

| 项目 | 位置 | 说明 |
| --- | --- | --- |
| `DataDirectories` | [paths.rs:19](src/paths.rs#L19) | 四个逻辑根；`discover()` / `with_roots()` / `plugin_dir()` |
| `DataDirectories::discover()` | [paths.rs:28](src/paths.rs#L28) | 按平台约定发现四个根（不创建目录） |
| `DataDirectories::config_db()` | [paths.rs:76](src/paths.rs#L76) | 权威配置数据库路径 `<config>/core.sqlite3` |
| `DataDirectories::plugin_dir()` | [paths.rs:87](src/paths.rs#L87) | 插件数据目录 `<data>/plugins/<id>`，0700 且拒绝符号链接 |
| `StorageManager` | [store.rs:65](src/store.rs#L65) | 持有目录、SQLite 连接（`Mutex`）与主密钥来源 |
| `StorageManager::open()` | [store.rs:73](src/store.rs#L73) | 在给定目录打开/创建数据库并执行迁移 |
| `StorageManager::open_default()` | [store.rs:87](src/store.rs#L87) | 按平台默认目录打开；主密钥优先取 `FLICK_MASTER_KEY` |
| `replace_decls()` | [store.rs:226](src/store.rs#L226) | 声明增量更新，返回 `DeclDiff`，事务内处理 renamed / orphaned |
| `get_value()` / `set_value()` | [store.rs:309](src/store.rs#L309) / [store.rs:349](src/store.rs#L349) | 读写值；secret 项自动加解密并避开明文表 |
| `revision()` | [store.rs:296](src/store.rs#L296) | 读取命名空间 revision（不存在为 0） |
| `rotate_master_key()` | [store.rs:437](src/store.rs#L437) | 用当前主密钥重加密全部 secret 行 |
| `purge_uninstalled_older_than()` | [store.rs:175](src/store.rs#L175) | 清理软删除超过保留期的插件数据与目录 |
| `MasterKey` / `MasterKeySource` | [crypto.rs:26](src/crypto.rs#L26) / [crypto.rs:43](src/crypto.rs#L43) | 主密钥材料与来源抽象（支持历史密钥 `load_by_id`） |
| `EnvMasterKeySource` | [crypto.rs:72](src/crypto.rs#L72) | `FLICK_MASTER_KEY`（64 位十六进制）+ `FLICK_MASTER_KEY_ID` |
| `UnavailableMasterKeySource` | [crypto.rs:58](src/crypto.rs#L58) | 生产默认：不可用返回结构化错误，绝不回退明文 |
| `EncryptedSecret` | [crypto.rs:102](src/crypto.rs#L102) | 密文行：`key_id` / `crypto_version` / `nonce` / `ciphertext` |
| `SCHEMA_VERSION` | [schema.rs:10](src/schema.rs#L10) | 当前数据库 schema 版本（1） |
| `DataError` | [error.rs:10](src/error.rs#L10) | 物理层错误枚举（`Sqlite`/`Io`/`Crypto`/`UnsupportedSchema` 等） |

## 模块 / 文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根；模块声明与全部重导出 |
| [src/paths.rs](src/paths.rs) | 四根发现、插件目录创建、id 校验、符号链接防护 |
| [src/schema.rs](src/schema.rs) | PRAGMA、五张表建表语句与向前迁移（`PRAGMA user_version`） |
| [src/store.rs](src/store.rs) | `StorageManager`：插件元数据、声明缓存、明文/密文值、revision、轮换与清理 |
| [src/crypto.rs](src/crypto.rs) | AEAD 加解密、主密钥来源、`SharedKeySource`、hex 解码 |
| [src/error.rs](src/error.rs) | `DataError` 错误枚举 |
| [tests/paths.rs](tests/paths.rs) | 路径与目录安全测试（4 例） |
| [tests/store.rs](tests/store.rs) | 读写、revision、密文、迁移、轮换与清理测试（10 例） |

## 测试与常用命令

```bash
cargo test -p flick-data                 # 14 例：4 路径 + 10 存储
cargo test -p flick-data --test paths
cargo test -p flick-data --test store
cargo check -p flick-data
cargo doc -p flick-data --open
```

测试用 `DataDirectories::with_roots()` + `tempfile::tempdir()` 隔离目录，用 `FixedMasterKeySource` 注入固定密钥；`unavailable_key_source_refuses_secret_io` 验证无密钥时返回 `SecretStoreUnavailable` 而非明文回退。

## 与其他 crate 的关系

- `flick-domain`：提供设置声明/值类型与插件 id 校验规则，被 `paths.rs`、`store.rs` 复用。
- `flick-settings`（业务层）：本 crate 为其提供底层设施；指纹比对与业务校验在那一层完成，`replace_decls()` 只按调用方给出的声明写缓存。
- 宿主装配层：通过 `StorageManager::open_default()` 或 `open()` 打开数据库；生产需接入系统钥匙串实现 `MasterKeySource`（当前 `UnavailableMasterKeySource` 为占位）。
- 测试：使用 `FixedMasterKeySource` / `EnvMasterKeySource` 可注入密钥，避免依赖真实钥匙串。
