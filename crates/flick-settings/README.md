# flick-settings

Flick 的**设置业务层**：在 `flick-data` 的存储能力之上提供 schema 注册、默认值解析、值校验、revision 递增与变更订阅。

声明类型（`SettingDecl` / `SettingKind` / `SettingValue` 等）定义在 `flick-domain::settings`；物理读写（SQLite、五张共享表、AEAD 密文、四逻辑根）在 `flick-data`。本 crate 只负责“业务规则”，不持有连接字符串、不自己建表、不定义声明类型。

## 在 workspace 中的位置

```
flick-domain::settings（纯类型）
        │
        ▼
flick-data::StorageManager（SQL / 密文 / 路径）
        │
        ▼
flick-settings（本 crate：schema、校验、默认值、revision、订阅）
        │
        ▼
daemon / IPC → UI（core.theme、core.search.result_limit、plugin.<id>.<key>）
```

依赖：`flick-domain`、`flick-data`、`tokio`（仅用 `broadcast`）、`serde`、`serde_json`、`thiserror`、`tracing`；开发依赖 `tempfile` 与 `rusqlite`。

**注意**：所有公开方法都是同步的——SQLite 是阻塞 I/O，调用方（daemon / IPC）应放到 blocking 线程池执行，不要在 Tokio 异步任务里直接调用。

## 为什么设计这个 crate

把「设置」切成三层，每层只做一件事，本 crate 占据中间的**业务规则边界**：

- `flick-domain::settings` 提供**纯类型**（声明、值、命名空间），`SettingKind::validate()` 定义单值的合法区间 / 枚举集合，无 I/O。
- `flick-data` 提供**物理设施**：数据库、五张表、事务、revision 自增、AEAD 加解密；它不知道“哪个键有默认值”“注册时指纹是否变化”。
- 本 crate 把两者拼起来：维护进程内 schema 表、决定写入前是否需要校验、决定读值时如何回退到默认值、决定变更如何广播、决定插件 schema 何时需要重写声明。
- 这样 IPC / daemon 只需面对一个 `SettingsService`，底层换数据库或加密方案不影响业务代码；插件作者只提交 manifest 中的声明，由本层统一校验与落库。

## 如何运行

**装配顺序**：宿主先用 `flick_data` 打开 `StorageManager`，再 `SettingsService::new(storage)`，然后调用 `register_schema()` 把 core schema 与各插件 schema 注册进来，最后接到 IPC / daemon 的 blocking 线程池上。

**注册链**：[`register_schema(namespace, plugin_id, version, schema)`](src/service.rs#L62) 先校验声明（键合法、默认值合法、无重复），读取 `flick-data` 中该插件的 `schema_fingerprint`：若与当前声明指纹一致，则**跳过** `replace_decls()`，只更新 `plugins` 表的版本字段；指纹变化时才写入声明、返回 `RegisterOutcome { namespace, revision, changed, diff }`（含 `DeclDiff`）。注册结果会 `tracing::info` 记录新增 / 改名 / 孤儿声明。

**读写链**：[`get()`](src/service.rs#L161) 直接读存储值；[`resolved()`](src/service.rs#L172) 按「存储值 → `decl.default` → `kind.fallback_value()`」三级回退，因此新键在未写入时也能拿到合理值。[`set()`](src/service.rs#L189) 先 `require_schema` + `kind.validate`，再调用 `storage.set_value()`，由底层在事务里做 secret 分流与 revision 自增，最后 [`publish()`](src/service.rs#L231) 向广播通道发 `SettingChange { namespace, revision }`。[`reset_key()`](src/service.rs#L209) / [`reset_namespace()`](src/service.rs#L217) 只删值、保留声明。

**线程 / 阻塞模型**：`SettingsService` 自身不持有 Tokio runtime；公开方法全部同步阻塞（SQLite + `std::sync::RwLock` 保护 schema 表）。`broadcast::Sender` 仅在 `tokio` 的 broadcast 通道意义上使用，订阅者通过 [`subscribe()`](src/service.rs#L227) 拿到 receiver。发送时若无人订阅，`publish()` 忽略 `SendError`，不影响写操作。

## 内部技术与架构细节

- **进程内 schema 表**：`SettingsService` 持有 `schemas: RwLock<HashMap<String, SettingSchema>>`，键为 namespace 字符串。`require_schema()` 在每次读写前查表，未注册的命名空间返回 `SettingsError::UnknownNamespace`，未注册的键返回 `UnknownKey { namespace, key }`（[service.rs:239](src/service.rs#L239)、[service.rs:244](src/service.rs#L244)）。
- **指纹短路**：注册时用 schema 自身计算的 `schema_fingerprint` 与 `flick-data` 中 `plugins` 表的记录比对，相同则不触发 `replace_decls()`，避免每次启动都重写声明、把 revision 无谓抬高；`changed` 字段反映是否真的重写。
- **变更通道**：`CHANGE_CHANNEL_CAPACITY = 256`（[service.rs:20](src/service.rs#L20)）。广播是有界 `broadcast` 通道，慢订阅者会收到 `Lagged`；`publish()` 只关心发送失败（无人订阅），不阻塞写入。
- **校验发生在写前**：`set()` 先经 `declaration()` 找到 `SettingDecl`，再 `kind.validate()`；底层 `flick-data` 还会再做一次物理层校验，两道防线的分工是业务层负责“键是否存在、值是否合法”，物理层负责“类型标签是否匹配”。`decl.secret` 决定值落到明文表还是密文表，业务层不接触密钥。
- **错误边界**：[`SettingsError`](src/error.rs#L9) 统一 `UnknownNamespace` / `UnknownKey { namespace, key }` / `InvalidValue { key, reason }` / `InvalidSchema { namespace, reason }` / `CorruptValue` / `SchemaVersionUnsupported { namespace, version }` / `PermissionDenied { namespace }` / `SubscriptionClosed` / `Storage(#[from] DataError)`；底层 `SettingValueError` 经 `From` 转成 `InvalidSchema`。
- **core schema**：[`core_schema()`](src/core_schema.rs#L11) 返回 `CORE_SCHEMA_VERSION = 1` 的 `SettingSchema`，含 [`theme.mode`](src/core_schema.rs#L15)（枚举 `dark` / `light`，默认 `dark`）与 [`search.result_limit`](src/core_schema.rs#L26)（整数 1–100，默认 20）。

## 公开 API 与核心类型

| 类型 / 函数 | 位置 | 说明 |
| --- | --- | --- |
| `SettingsService` | [service.rs:36](src/service.rs#L36) | 服务主体，持有 `Arc<StorageManager>`、内存 schema 表与广播通道 |
| `SettingsService::new` | [service.rs:44](src/service.rs#L44) | 用存储句柄构造服务 |
| `register_schema` | [service.rs:62](src/service.rs#L62) | 校验并注册 schema；指纹未变则跳过声明写入；插件同时更新 `plugins` 表 |
| `registered_namespaces` / `schema` | [service.rs:125](src/service.rs#L125)、[service.rs:138](src/service.rs#L138) | 已注册命名空间与 schema 查询 |
| `list` | [service.rs:147](src/service.rs#L147) | 读取一个 namespace 的 `SettingSnapshot`（含 revision） |
| `get` / `resolved` | [service.rs:161](src/service.rs#L161)、[service.rs:172](src/service.rs#L172) | 直接读值 / 按“存储值 → 声明默认 → 类型回退”解析 |
| `set` / `reset_key` / `reset_namespace` | [service.rs:189](src/service.rs#L189)、[service.rs:209](src/service.rs#L209)、[service.rs:217](src/service.rs#L217) | 写入、删单键、删整个 namespace 的值（保留声明）；均返回新 revision |
| `subscribe` | [service.rs:227](src/service.rs#L227) | 订阅 `SettingChange { namespace, revision }`（容量 256） |
| `RegisterOutcome` | [service.rs:24](src/service.rs#L24) | 注册结果：revision、指纹是否变化、`DeclDiff` |
| `SettingsError` | [error.rs:9](src/error.rs#L9) | 统一错误：未知命名空间/键、非法值/声明、版本不支持、权限拒绝、存储错误等 |
| `core_schema()` / `CORE_SCHEMA_VERSION` | [core_schema.rs:8](src/core_schema.rs#L8)、[core_schema.rs:11](src/core_schema.rs#L11) | 宿主默认 schema（当前版本 1） |

## 文件清单

| 文件 | 行数 | 职责 |
| --- | ---: | --- |
| `src/lib.rs` | 12 | 模块与根重导出 |
| `src/service.rs` | 260 | 设置服务：注册、校验、读写、revision、订阅 |
| `src/error.rs` | 74 | `SettingsError` 及从底层错误的转换 |
| `src/core_schema.rs` | 38 | Flick 自身默认设置 schema |
| `tests/service.rs` | 256 | 十个契约测试 |

## 测试与常用命令

```bash
cargo test -p flick-settings
cargo test -p flick-settings --test service
cargo check -p flick-settings
```

`tests/service.rs` 使用 `tempfile` 建临时仓库，不触碰真实 `$XDG_CONFIG_HOME/flick`。

## 与其他 crate 的关系

- **上游**：`flick-domain` 提供纯类型与 `SettingKind::validate()`；`flick-data` 提供 `StorageManager` 与 `DeclDiff`。
- **下游**：daemon 装配 `SettingsService`（`StorageManager → SettingsService → IPC`），插件 schema 由各自 manifest 提供后经 `register_schema()` 注册。
