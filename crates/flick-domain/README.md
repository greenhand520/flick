# flick-domain

Flick 运行时的核心领域类型 crate：定义 UI、CLI、Runtime、IPC 与插件层共享的、与框架无关的纯数据协议。

## 在 workspace 中的位置与依赖

`flick-domain` 位于依赖图最底层，是其它 crate 的公共词汇表；它只依赖序列化/UUID/错误基础设施，不依赖 Iced、GPUI、Portal、D-Bus、SQLite、XDG、systemd 或 Unix Socket，Tokio 也仅用于 `ActionContext` 中的 `CancellationToken`（见 [lib.rs:1](src/lib.rs#L1)）。

| 依赖 | 用途 |
| --- | --- |
| `serde` / `serde_json` | 所有协议类型的序列化；`ActionOutcome::Data` 的 `Value` 负载 |
| `uuid` | `InvocationId`、`SurfaceSessionId` 的底层标识 |
| `thiserror` | `ActionError`、`SettingValueError` 的错误派生 |
| `tokio-util` | `ActionContext::cancellation` 的 `CancellationToken` |
| `tokio` / `rmp-serde`（dev） | 异步测试与 MessagePack 往返测试 |

依赖声明见 [Cargo.toml:6](Cargo.toml#L6)。crate 常量 `APP_ID = "io.github.greenhand520.Flick"`（[lib.rs:24](src/lib.rs#L24)）同时用作 portal app id、`.desktop` 文件名与 GPUI 窗口 `app_id`，三者必须一致。

## 为什么设计这个 crate

项目的 workspace 责任边界把「ID、Action 协议、领域错误、查询与状态纯类型」划给本 crate，并明确它**不承担** Iced、Tokio 运行时、SQLite、D-Bus、Portal；核心 crate 也不依赖 Linux 类型、XDG、Portal、D-Bus、systemd 或 Unix Socket，以便未来在别的操作系统复用同一套核心语义。这一边界直接体现在 [lib.rs:1](src/lib.rs#L1) 的「不依赖什么」清单，以及 Shell 只能单向依赖领域协议的设计约束（`docs/2026-11-24-gpui-shell-design.md:14-18, 40-48`）。

单独拆出的理由是**同一套语义必须被 UI 外壳、CLI、Runtime 和 IPC 共同消费**：若把协议类型放进某个外壳或运行时，CLI/UI 就会反向依赖实现层，也难以在 GUI 之外单测。依赖方向固定为「shell / cli / daemon → application + runtime → domain」，可由各 crate 的 `Cargo.toml` 依赖声明验证，本 crate 因此是唯一的叶子。设置类型遵循同一条边界：`settings.rs` 只放跨层共享的纯数据与纯校验，`flick-settings` 负责文件 I/O 与持久化，crate 划分的是职责。

## 如何运行

本 crate **没有 `main`、没有进程、没有事件循环，也不起线程**：它是一个被其它 crate 在编译期链接的纯库。生命周期完全由调用方驱动——UI 外壳采集输入形成 `SearchInputSnapshot`（[input.rs:59](src/input.rs#L59)），同步解析为 `InputContext`（[input.rs:228](src/input.rs#L228)）；协调器/CLI 用 `ActionDescriptor`（[action.rs:14](src/action.rs#L14)）与 `ActionRequest`（[action.rs:209](src/action.rs#L209)）发起调用；Runtime 或插件返回 `ActionOutcome`（[action.rs:267](src/action.rs#L267)），再由 `HostToUi`（[ui.rs:94](src/ui.rs#L94)）或 CLI 输出呈现给用户。

所有协议类型都派生 serde，可经内存通道零拷贝传递、经 IPC 走 MessagePack 往返；本 crate 自身不发起 I/O（不查库、不读文件、不访问网络、`from_snapshot` 不做 `stat`）。唯一的异步边角是 `ActionContext`（[action.rs:255](src/action.rs#L255)）携带的 `CancellationToken`——它只是交给执行方协作式轮询的句柄，不在本 crate 内产生任务或线程。`APP_ID`（[lib.rs:24](src/lib.rs#L24)）是跨进程启动时注入的统一身份，用于 portal、`.desktop` 与窗口系统三方对齐。

## 内部技术与架构细节

**标识符与代次。** `InvocationId`（[id.rs:10](src/id.rs#L10)）与 `SurfaceSessionId`（[id.rs:34](src/id.rs#L34)）是 UUID newtype；`QueryGeneration`（[id.rs:58](src/id.rs#L58)）从 `INITIAL = 1` 起，`next()` 用 `wrapping_add` 前进，调用方靠它丢弃乱序返回的旧代次结果；`ActionId`（[id.rs:88](src/id.rs#L88)）内部为 `Arc<str>`，构造时拒绝空白字符串。

**动作描述与权限。** `ActionDescriptor` 的缺省值：version `"1.0.0"`、`search_invocable = false`、`supported_inputs` 为空表示不限制、`output_types` 默认为 `[Data]`、`invocation_policy` 默认为 `NonInteractive`；`file_extensions` 约定为小写且不带点。`effective_permissions`（[action.rs:60](src/action.rs#L60)）按「插件权限在前、动作权限在后」合并，以 O(n²) 比较保序去重。`ActionRequest::new_echo`（[action.rs:222](src/action.rs#L222)）使用 30 秒截止时间与 `Caller::ui("search-panel")`。

**输入分面解析。** `InputFacet`（[input.rs:182](src/input.rs#L182)）中 `PlainText` 永远存在，另可并存 Url、PathLike、MathExpression（`explicit` 标记是否以 `=` 显式声明）、File、ClipboardImage。`normalize_url` 只接受 http/https/ftp/mailto 且不含空白的文本；`detect_path_like` 识别 `file://`、`~`、`/`、`./`、`../` 和 Windows 盘符；数学表达式用受限字符集判定，避免把普通文本误判为公式。`to_action_input` 的规则是单文件归为 `PathCandidate`、多文件归为 `Files`。

**候选身份与去重键。** `RouteItemKey`（[route.rs:28](src/route.rs#L28)）取 `Action(ActionId)` 或 `Execution { parent_action_id, item_id }`，刻意**不含 `invocation_id`**，这样同一动作在一次会话的多次查询中可被累积层识别并去重；`key()`（[route.rs:133](src/route.rs#L133)）在 Execution 项缺失 `parent_action_id` 时回退到 `action_id`。

**结果与错误。** `ActionOutcome` 的七种形态为 Data / Notice / Error / Text / Url / File / OpenSurface；`open_surface`（[action.rs:298](src/action.rs#L298)）默认 `session_id = None`、`needs_search = false`，会话 id 由 Runtime 回填。`ActionError`（[error.rs:11](src/error.rs#L11)）共七类，`code()`（[error.rs:36](src/error.rs#L36)）给机器稳定码，`user_message()`（[error.rs:49](src/error.rs#L49)）不泄露内部上下文。

**UI / Surface 协议。** `UiIntent`（[ui.rs:19](src/ui.rs#L19)）的 `is_action_invocation()` 恒为 `false`，把「显示/隐藏面板」与「调用动作」严格分开。`SurfaceContent`（[surface.rs:17](src/surface.rs#L17)）带 `SURFACE_SCHEMA_VERSION = 1`（[surface.rs:9](src/surface.rs#L9)），缺省 schema 会回填；`SurfaceNode`（[surface.rs:29](src/surface.rs#L29)）以 `serde(tag = "kind")` + snake_case 编码节点树，交互节点的 id 在单个 content 内唯一。

**设置与偏好算法。** `SettingKind`（[settings.rs:64](src/settings.rs#L64)）有 Bool / Integer{min,max} / String / Enum{variants} / Path(file://) / Json 六类，声明期校验枚举非空、整数上下界有序；`SettingSchema::validate`（[settings.rs:250](src/settings.rs#L250)）要求插件短 key 不含 `.` 且不得冒用 `core` / `core.*` 前缀，`fingerprint` 用规范化 JSON 排序后做 FNV-1a64，并以 `fp1|` 开头；插件 id 规则为 `[A-Za-z0-9][A-Za-z0-9._-]*` 且拒绝 `.`、`..`、`core`。`UsageStat::decayed_score`（[usage.rs:13](src/usage.rs#L13)）以 30 天为半衰期，`frecency_boost` 上限 180 分。

## 公开 API 与核心类型

所有公开项在 [lib.rs:27](src/lib.rs#L27) 起统一重导出到 crate 根，因此常用路径是 `flick_domain::ActionId` 而非 `flick_domain::id::ActionId`。

### 动作与调用（[action.rs](src/action.rs)）

| 类型 | 位置 | 说明 |
| --- | --- | --- |
| `ActionDescriptor` | [action.rs:14](src/action.rs#L14) | 已注册动作的描述符；`new`（[L36](src/action.rs#L36)）填默认值，`effective_permissions`（[L60](src/action.rs#L60)）合并插件与动作权限并去重 |
| `ActionInput` / `ImageData` / `FileInput` | [action.rs:84](src/action.rs#L84) / [L131](src/action.rs#L131) / [L138](src/action.rs#L138) | 调用输入：`plain_text`、`clipboard_image`、`none` 三种构造器 |
| `InputType` / `OutputType` | [action.rs:153](src/action.rs#L153) / [L169](src/action.rs#L169) | 输入分类与结果类型，含 `ClipboardImage`、`Surface` |
| `ActionRequest` | [action.rs:209](src/action.rs#L209) | 一次调用请求；`new_echo`（[L222](src/action.rs#L222)）为 Phase 0 便捷构造 |
| `Deadline` / `ActionContext` | [action.rs:238](src/action.rs#L238) / [L255](src/action.rs#L255) | 截止时间与协作式取消上下文 |
| `ActionOutcome` | [action.rs:267](src/action.rs#L267) | 调用结果：`Data / Notice / Error / Text / Url / File / OpenSurface`；`open_surface`（[L298](src/action.rs#L298)）构造 Surface 结果 |

### 输入、路由与执行（[input.rs](src/input.rs)、[route.rs](src/route.rs)、[execution.rs](src/execution.rs)）

- `SearchInputSnapshot`（[input.rs:59](src/input.rs#L59)）是 UI 采集的快照；`InputContext::from_snapshot`（[input.rs:228](src/input.rs#L228)）用纯函数解析出可并存的 `InputFacet`（[input.rs:182](src/input.rs#L182)），不读网络、不查库、不 `stat`。
- `RouteItem`（[route.rs:55](src/route.rs#L55)）是 Host 返回的扁平候选；`key()`（[route.rs:133](src/route.rs#L133)）给出累积去重用的 `RouteItemKey`（[route.rs:28](src/route.rs#L28)）。Execution 项以「父 Action + item_id」为身份，刻意不含 `invocation_id`。
- `ExecutionItem`（[execution.rs:35](src/execution.rs#L35)）与 `ExecutionEventKind`（[execution.rs:27](src/execution.rs#L27)）描述搜索阶段执行项及其激活事件。

### 标识、调用方与错误

- `InvocationId`（[id.rs:10](src/id.rs#L10)）、`SurfaceSessionId`（[id.rs:34](src/id.rs#L34)）为 UUID newtype；`QueryGeneration`（[id.rs:58](src/id.rs#L58)）单调递增，用于丢弃过期结果；`ActionId`（[id.rs:88](src/id.rs#L88)）为非空 `Arc<str>`。
- `Caller` / `CallerKind`（[caller.rs:10](src/caller.rs#L10) / [caller.rs:36](src/caller.rs#L36)）记录调用方；`ActionSource`（[execution.rs:9](src/execution.rs#L9)）记录发起阶段。
- `ActionError`（[error.rs:11](src/error.rs#L11)）带稳定 `code()`（[error.rs:36](src/error.rs#L36)）与用户安全 `user_message()`（[error.rs:49](src/error.rs#L49)）。

### Surface、UI 协议、设置与插件元数据

- `SurfaceContent`（[surface.rs:17](src/surface.rs#L17)）+ `SurfaceNode`（[surface.rs:29](src/surface.rs#L29)）是数据驱动的节点树；`SURFACE_SCHEMA_VERSION = 1`（[surface.rs:9](src/surface.rs#L9)）。进程内零序列化，跨 IPC 走 MessagePack。
- `UiToHost`（[ui.rs:39](src/ui.rs#L39)）/ `HostToUi`（[ui.rs:94](src/ui.rs#L94)）为双向协议；`UiIntent`（[ui.rs:19](src/ui.rs#L19)）表示快捷键唤起等意图，**不是**动作调用；`RuntimeStatus`（[ui.rs:140](src/ui.rs#L140)）为诊断状态。
- 设置系统：`SettingNamespace`（[settings.rs:14](src/settings.rs#L14)）、`SettingKind`（[settings.rs:64](src/settings.rs#L64)）、`SettingValue`（[settings.rs:156](src/settings.rs#L156)）、`SettingDecl`（[settings.rs:211](src/settings.rs#L211)）、`SettingSchema`（[settings.rs:250](src/settings.rs#L250)）、`SettingChange`（[settings.rs:369](src/settings.rs#L369)）、`SettingSnapshot`（[settings.rs:378](src/settings.rs#L378)）、`SettingKey`（[settings.rs:396](src/settings.rs#L396)）、`SettingValueError`（[settings.rs:470](src/settings.rs#L470)）。本模块只做纯校验，文件 I/O 与持久化在 `flick-data`。
- 插件元数据 `PluginManifest`（[plugin.rs:124](src/plugin.rs#L124)）及 `PluginKind`、`Category`、`OperatingSystem`、`Architecture` 等集中在 [plugin.rs](src/plugin.rs)；`supports_platform`（[plugin.rs:156](src/plugin.rs#L156)）以「空列表 = 不限」判定平台。
- `UsageStat`（[usage.rs:13](src/usage.rs#L13)）与 `UsageStore`（[usage.rs:63](src/usage.rs#L63)）定义 frecency 公式与存储接口；`HostInfo`（[host.rs:9](src/host.rs#L9)）是适配器启动时注入的宿主信息。

## 模块与文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根：13 个 `pub mod` 与统一重导出 |
| [src/action.rs](src/action.rs) | 描述符、输入、请求、上下文、结果 |
| [src/input.rs](src/input.rs) | UI 输入快照与 facet 解析 |
| [src/route.rs](src/route.rs) | 路由候选与去重身份 |
| [src/execution.rs](src/execution.rs) | 执行项与调用来源 |
| [src/id.rs](src/id.rs) | 各类标识符 |
| [src/caller.rs](src/caller.rs) | 调用方标识 |
| [src/error.rs](src/error.rs) | 结构化错误模型 |
| [src/surface.rs](src/surface.rs) | 数据驱动 Surface 节点树 |
| [src/ui.rs](src/ui.rs) | UI ↔ Host 通信协议 |
| [src/settings.rs](src/settings.rs) | 设置类型与纯校验 |
| [src/plugin.rs](src/plugin.rs) | 插件元数据 |
| [src/usage.rs](src/usage.rs) | frecency 数据与存储 trait |
| [src/host.rs](src/host.rs) | 宿主 OS / 架构等信息 |
| [tests/domain_protocol.rs](tests/domain_protocol.rs) | 协议往返、标识符、意图边界 |
| [tests/execution_contract.rs](tests/execution_contract.rs) | 描述符默认值与执行项契约 |
| [tests/plugin_types.rs](tests/plugin_types.rs) | manifest / 动作类型序列化与平台匹配 |
| [tests/settings.rs](tests/settings.rs) | 设置校验、键解析、插件 id 规则 |
| [tests/surface.rs](tests/surface.rs) | Surface JSON / MessagePack 往返 |

## 测试与常用命令

```bash
cargo test -p flick-domain                       # 全部单元 + 集成测试
cargo test -p flick-domain --test domain_protocol
cargo test -p flick-domain --test settings
cargo clippy -p flick-domain --all-targets -- -D warnings
```

测试无需任何系统服务：只用 `serde_json` 与 dev 依赖 `rmp-serde` 做往返；`CancellationToken` 由 `tokio` 提供。

## 与其他 crate 的关系

- **被依赖**：几乎所有 crate——`flick-data`、`flick-application`、`flick-runtime`、`flick-plugin`、`flick-builtins`、`flick-ipc`、`flick-adapters-linux`、`flick-daemon`、`flick-cli`、`flick-ui`、`flick-shell-gpui`、`flick-testkit`。
- **依赖**：无 workspace 内 crate，是依赖图的叶子。
- `flick-plugin` 把本 crate 的类型再重导出给插件作者，使插件只需依赖 `flick-plugin`；设置数据的持久化在 `flick-data`，插件 manifest 的静态校验在 `flick-plugin`。
