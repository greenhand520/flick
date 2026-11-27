# flick-plugin

Flick 的插件契约 crate：定义插件作者要实现的 trait，以及 manifest 静态校验与依赖解析的纯函数。

## 在 workspace 中的位置与依赖

`flick-plugin` 只含「插件作者需要实现什么」与纯校验/解析，不依赖 tokio，也不持有任何注册表状态（见 [lib.rs:1](src/lib.rs#L1)）。它依赖领域类型，并把关键领域类型再转出，使插件作者通常只需依赖本 crate。

| 依赖 | 用途 |
| --- | --- |
| `flick-domain`（path） | 插件元数据与动作协议的领域类型 |
| `async-trait` | 让异步 trait 保持对象安全（`Arc<dyn ActionHandler>`） |
| `serde_json`（dev） | 测试中构造 `ActionOutcome::Data` 的 `Value` |

依赖声明见 [Cargo.toml:6](Cargo.toml#L6)。本 crate 没有 `tests/` 目录：测试以内联 `#[cfg(test)]` 形式放在 [dependency.rs:97](src/dependency.rs#L97) 与 [manifest.rs:84](src/manifest.rs#L84)。

## 为什么设计这个 crate

插件契约原本在 `flick-runtime` 中（`plugin.rs` 117 行 + `handler.rs` 38 行），二者只依赖 `async-trait` 与 `flick-domain`；但 `flick-runtime` 的 `Cargo.toml` 要求 `tokio = { features = ["full"] }`，插件作者为了写一个插件就被迫把整条运行时拖进依赖树（`docs/superpowers/specs/2026-11-27-plugin-dependency-design.md:30-52`）。拆出本 crate 的收益是：插件作者只需 `flick-plugin` + `flick-domain`；依赖解析成为可直接单测的纯函数（`resolve_dependencies(&BTreeMap<..>)`，无需构造 `ActionRegistry`）；并由领域类型重导出兜住现有内置插件，下游无需改动。

边界由设计文档明确为「契约 + 纯函数」，**不碰注册表**：`ActionRegistry` 仍留在 `flick-runtime`，负责存储、搜索以及把解析结果应用到 entries（同上 `:54-55`）。依赖检查之所以拆成注册期静态校验、全部注册完成后统一解析两阶段，是因为注册顺序不应由依赖拓扑决定——依赖只影响可用性，不影响注册顺序（同上 `:57-69`）。

## 如何运行

本 crate 是纯库，没有 `main`、没有进程、没有事件循环，也不创建线程。生命周期分两条线：

- **注册期（同步）**：宿主 `flick-runtime` 拿到插件的 `PluginManifest` 与 `Vec<ActionSpec>` 后调用 `validate_manifest`（[manifest.rs:15](src/manifest.rs#L15)）做静态校验；所有插件注册完成后再调用 `resolve_dependencies`（[dependency.rs:33](src/dependency.rs#L33)）统一解析可用性。两者都是同步纯函数，不访问注册表状态。
- **运行期（由宿主驱动）**：插件作者实现 `Plugin`（[contract.rs:102](src/contract.rs#L102)）并在 `actions()` 中交付 `ActionSpec`；Runtime 在 tokio 任务里调用 `ActionHandler::invoke`（[contract.rs:31](src/contract.rs#L31)），并用 `tokio::select!` 让它与截止时间/取消令牌竞速，因此处理器应在长任务中协作式检查 `ActionContext` 的取消令牌（见 [contract.rs:15](src/contract.rs#L15)）。搜索阶段执行项走 `execution_event`（[contract.rs:38](src/contract.rs#L38)）；Surface 会话依次触发 `on_open` / `on_event`（[contract.rs:64](src/contract.rs#L64)）/ `on_close`；带生命周期的插件还会收到 `activate` / `deactivate`。本 crate 自身不产生这些任务——异步只是 `async_trait` 生成的 boxed future，调度权在 Runtime。

## 内部技术与架构细节

**契约面。** `ActionHandler: Send + Sync` 的 `invoke(request, ctx) -> Result<ActionOutcome, ActionError>` 是最小执行单元，`execution_event` 默认返回 `Ok(None)`；`ActionSpec`（[contract.rs:50](src/contract.rs#L50)）= `ActionDescriptor` + `Arc<dyn ActionHandler>`，因此 handler 必须对象安全。`PluginUiFactory`（[contract.rs:56](src/contract.rs#L56)）目前是空 trait 占位，仅用于稳定签名。`PluginLifecycle::idle_timeout()` 返回 `Option<Duration>`，缺省视为 Resident（常驻，不因空闲卸载）。

**错误模型。** `PluginRegistrationError`（[contract.rs:119](src/contract.rs#L119)）有 8 个变体：`InvalidManifest { reason }`、`NoActions`、`IdCollision`、`NamespaceViolation`、`DuplicateSettingKey`、`SurfaceIdCollision`、`DependencyMissing`；`PluginRuntimeError`（[contract.rs:143](src/contract.rs#L143)）有 9 个变体：`NotActivated`、`ActivationFailed`、`DeactivationFailed`、`ResourceUnavailable`、`UnsupportedInput`、`DependencyUnavailable`、`HandlerPanicked`、`DeadlineExceeded`、`Cancelled`。

**manifest 校验顺序。** `validate_manifest` 依次检查：id 非空（trim）、author 非空（trim）、setting key 唯一（`HashSet`）、surface id 唯一、动作列表非空、每个动作 id 必须以 `<plugin_id>.` 开头、不得自依赖、依赖不得重复声明。`ActionId` 全局冲突需要注册表状态，交给 `flick-runtime` 的 `ActionRegistry`。

**依赖解析算法。** `DependencyResolution`（[dependency.rs:15](src/dependency.rs#L15)）含 `statuses: HashMap<String, Vec<DependencyStatus>>`、`unavailable: HashSet<String>`、`errors: Vec<PluginRegistrationError>`；其中 `errors` 是**每个不可用插件一条** `DependencyMissing`，而不是每个缺失依赖一条。`resolve_dependencies` 用不动点传播：`max_rounds = manifests.len() + 1`（[dependency.rs:38](src/dependency.rs#L38)），`has_broken_required`（[L85](src/dependency.rs#L85)）判定 Required 依赖缺失或自身不可用会使本插件连锁不可用；Optional 缺失只把该项 `available` 置为 false；环不会报错，因为依赖只决定可用性、不决定加载顺序。入参用 `BTreeMap` 而非 `HashMap` 是为了迭代顺序确定——测试断言与 `flick doctor` 输出依赖此性质（[dependency.rs:26](src/dependency.rs#L26)）。

## 公开 API 与核心类型

重导出见 [lib.rs:10](src/lib.rs#L10)（契约 trait）、[lib.rs:14](src/lib.rs#L14)（依赖解析）、[lib.rs:15](src/lib.rs#L15)（manifest 校验）与 [lib.rs:20](src/lib.rs#L20)（领域类型转出）。

### 插件契约（[contract.rs](src/contract.rs)）

| trait / 类型 | 位置 | 说明 |
| --- | --- | --- |
| `ActionHandler` | [contract.rs:25](src/contract.rs#L25) | `invoke`（[L31](src/contract.rs#L31)）执行动作并返回 `Result<ActionOutcome, ActionError>`；`execution_event`（[L38](src/contract.rs#L38)）默认 no-op |
| `ActionSpec` | [contract.rs:50](src/contract.rs#L50) | 一个动作 = `ActionDescriptor` + `Arc<dyn ActionHandler>` |
| `SurfaceHandler` | [contract.rs:62](src/contract.rs#L62) | `on_event`（[L64](src/contract.rs#L64)）处理 Surface 事件；`on_open` / `on_close` 为可选钩子 |
| `SurfaceSession` / `SurfaceError` | [contract.rs:79](src/contract.rs#L79) / [L87](src/contract.rs#L87) | Surface 会话元数据与可展示错误 |
| `PluginLifecycle` | [contract.rs:93](src/contract.rs#L93) | `activate` / `deactivate`；缺省视为 Resident（常驻） |
| `Plugin` | [contract.rs:102](src/contract.rs#L102) | `manifest()` + `actions()`，可选的 `lifecycle` / `ui` / `surface_handler` |
| `PluginUiFactory` | [contract.rs:56](src/contract.rs#L56) | UI 工厂占位 trait，保证签名稳定 |
| `PluginRegistrationError` / `PluginRuntimeError` | [contract.rs:119](src/contract.rs#L119) / [L143](src/contract.rs#L143) | 注册期错误（manifest 非法、无动作、id 冲突、命名空间违规、依赖缺失等）与运行期错误 |

`ActionHandler` 使用 `async_trait` 保持对象安全；Runtime 用 `tokio::select!` 让处理器与截止时间/取消竞速，因此处理器应在长任务中协作式检查 `ActionContext` 的取消令牌（见 [contract.rs:15](src/contract.rs#L15)）。

### manifest 校验（[manifest.rs](src/manifest.rs)）

`validate_manifest(&PluginManifest, &[ActionSpec]) -> Result<(), PluginRegistrationError>`（[manifest.rs:15](src/manifest.rs#L15)）覆盖不依赖其它插件信息的规则：空 id、空 author、setting key 重复、surface id 重复、无动作、命名空间前缀、自依赖、依赖重复声明。`ActionId` 冲突需要注册表状态，仍交给 `flick-runtime` 的 `ActionRegistry`。

### 依赖解析（[dependency.rs](src/dependency.rs)）

- `resolve_dependencies(&BTreeMap<String, PluginManifest>) -> DependencyResolution`（[dependency.rs:33](src/dependency.rs#L33)）：`Required` 依赖缺失或自身不可用则本插件不可用；`Optional` 缺失只标记 `available == false`；环不报错，因为依赖只决定可用性、不决定加载顺序。
- `DependencyResolution`（[dependency.rs:15](src/dependency.rs#L15)）：`statuses`（逐条依赖状态）、`unavailable`（不可用插件集合）、`errors`（全部缺失错误，不短路）。
- 入参用 `BTreeMap` 而非 `HashMap` 是为了迭代顺序确定——测试断言与 `flick doctor` 输出依赖此性质。

## 模块与文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根：私有模块与转发重导出 |
| [src/contract.rs](src/contract.rs) | 插件作者要实现的 trait 与错误类型 |
| [src/manifest.rs](src/manifest.rs) | manifest 静态校验（内联测试） |
| [src/dependency.rs](src/dependency.rs) | 依赖解析纯函数（内联测试） |
| [Cargo.toml](Cargo.toml) | 包与依赖声明 |

## 测试与常用命令

```bash
cargo test -p flick-plugin                       # 全部内联单元测试
cargo test -p flick-plugin manifest              # 只看 manifest 校验
cargo test -p flick-plugin dependency            # 只看依赖解析
cargo clippy -p flick-plugin --all-targets -- -D warnings
```

所有测试都是纯函数级，无需 tokio、注册表或文件系统。

## 与其他 crate 的关系

- **依赖**：`flick-domain`（领域类型）；`async-trait`（异步 trait）。
- **被依赖**：`flick-runtime`——它在注册插件时调用 `validate_manifest` 与 `resolve_dependencies`，并调用 `Plugin` / `ActionHandler` / `SurfaceHandler`。
- `flick-builtins` 等插件实现本 crate 的 `Plugin` trait；由于领域类型已在 [lib.rs:20](src/lib.rs#L20) 转出，插件作者只需依赖 `flick-plugin`。
- 注册表（`ActionRegistry`）与 tokio 运行时的状态在 `flick-runtime`，本 crate 刻意不持有状态。
