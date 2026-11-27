# flick-runtime

Flick 的动作运行时：负责**注册动作与插件、按输入搜索路由、调用动作、管理超时与取消、维护 Surface 会话**。

本 crate 与框架无关，只依赖 `flick-domain`、`flick-plugin`、Tokio / tokio-util、async-trait、tracing、serde_json 与 `ib_pinyin`；不依赖 GPUI、Iced、Portal、D-Bus 或 Unix Socket。因此 daemon、CLI 和无头测试可以共用同一个运行时。插件契约（`Plugin` / `ActionHandler` / `PluginLifecycle` / `SurfaceHandler` 等）已抽到 `flick-plugin`，本 crate 在 [lib.rs:26](src/lib.rs#L26) 重导出，下游仍可从 `flick_runtime` 直接 import。

## 在架构中的位置

```
UI / CLI / 全局快捷键
    │  UiToHost（flick-domain::ui）
    ▼
flick-application :: ApplicationCoordinator ──► RuntimePort（窄接口）
    │                                              ▲
    │                                              │ 实现
    ▼                                              │
flick-daemon :: RuntimeAdapter ─────────────────────┘
    │
    ▼
flick-runtime（本 crate） ──► ActionHandler::invoke（flick-builtins / 各插件）
    │
    ▼
ActionOutcome（Data / Text / Url / File / OpenSurface / Error） ──► 回到 UI
```

协调器只依赖 `RuntimePort`（[crates/flick-application/src/ports.rs:19](../flick-application/src/ports.rs#L19)），真正的实现在 [crates/flick-daemon/src/bootstrap.rs:27](../flick-daemon/src/bootstrap.rs#L27) 的 `RuntimeAdapter`（实现块在 [bootstrap.rs:32](../flick-daemon/src/bootstrap.rs#L32)），它把调用一字不差地转发给 `ActionRuntime`。

## 为什么设计这个 crate

把「编排」从「UI 框架」和「动作实现」两侧剥离开，形成一条可替换的边界：

- 对上：只暴露窄接口（`route_input` / `search_invoke` / `invoke` / `execution_event` / `cancel` / Surface 事件），协调器不持有任何运行时内部状态；daemon、CLI、无头测试可以复用同一个实现。
- 对下：只依赖 `Plugin` / `ActionHandler` trait，不关心动作来自 `flick-builtins` 还是第三方插件；插件契约抽到 `flick-plugin` 后，本 crate 只负责注册、依赖解析、调度与会话管理。
- 把「并发与取消」这类最容易出错的逻辑集中在一处：搜索阶段要并行试探多个动作且不能产生副作用，执行阶段要处理用户取消与外部超时，Surface 要按会话路由事件——这些都在本 crate 内闭环，动作实现只需返回一个 `ActionOutcome`。
- 把「评分排序」做成纯函数模块 `search`，与状态化的注册表分离，便于独立测试排序规则，也便于插件用同一套规则扩充关键词。

## 如何运行

**启动装配**：宿主构造 `ActionRuntime`（[runtime.rs:165](src/runtime.rs#L165)）后，逐个 `register` / `register_plugin`（[runtime.rs:205](src/runtime.rs#L205)、[runtime.rs:214](src/runtime.rs#L214)），调用 [`resolve_dependencies()`](src/runtime.rs#L240) 得到不可用插件集合，然后 [`activate_all_plugins()`](src/runtime.rs#L223) 逐个激活生命周期——激活失败只写 `tracing::warn`，不阻断启动。内置插件通过 `flick_builtins::register_builtin` 完成同样的注册。

**一次请求的调用链**：

1. UI 的 `QueryChanged` → 协调器并行走 [`route_input()`](src/runtime.rs#L273) 与 [`search_invoke()`](src/runtime.rs#L287)。`route_input` 委托 `ActionRegistry::search_input()` → `search::score_action()` → `RouteItem`。
2. `search_invoke()` 用 `JoinSet` 并行调用所有 `search_invocable` 动作，请求来源标记为 `SearchRoute`，每个调用带 **150ms** 硬超时；新查询通过 `begin_search_generation()` 取消上一代；结果经 [`execution_route_items()`](src/runtime.rs#L644) 转成执行项，动作 ID 截断到 `MAX_ACTION_RESULTS = 100`。
3. UI 的 `Invoke` → [`invoke()`](src/runtime.rs#L472)：查注册表取 handler 与其所属插件 id，建立取消令牌（有父令牌则 `child_token()`），登记 `ActiveGuard`，`tokio::spawn` 执行 handler，然后 [`tokio::select!`](src/runtime.rs#L628) 三方竞速——handler 完成 / 请求截止时间 / 外部取消。
4. 若返回 `OpenSurface`，[`backfill_open_surface()`](src/runtime.rs#L669) 生成 `SurfaceSessionId`、按 manifest 回填 `needs_search`、调用 `on_open` 并登记会话；后续 [`surface_event()`](src/runtime.rs#L477) / [`surface_closed()`](src/runtime.rs#L503) 转发给插件。
5. [`cancel()`](src/runtime.rs#L523) 或超时只触发 `CancellationToken`；[`execution_event()`](src/runtime.rs#L436) 有固定 30s 超时，处理执行项的激活等交互。

**线程 / 阻塞模型**：核心是 `tokio::spawn` + `tokio::select!` 的异步编排；注册表读锁只在查找期间持有、不跨 `.await`；活动调用用 `tokio::sync::Mutex`，Surface 会话用 `RwLock`，宿主信息与插件目录在构造时注入为 `Arc`。超时 / 取消分支**不会** `abort` handler 任务——`JoinHandle` 被移入“handler 完成”分支，其它分支胜出时任务被 detach，直到 handler 自己观察到令牌才收敛，因此取消是**协作式**的。

## 内部技术与架构细节

- **核心状态**：[`ActionRuntime`](src/runtime.rs#L137) = `{ registry: Arc<RwLock<ActionRegistry>>, active: Arc<tokio::sync::Mutex<HashMap<InvocationId, ActiveEntry>>>, host_info: Arc<HostInfo>, surface_sessions: Arc<RwLock<HashMap<SurfaceSessionId, SurfaceSessionState>>>, search_cancel: std::sync::Mutex<Option<(QueryGeneration, CancellationToken)>>, settings/secrets: Arc<RwLock<Option<…>>>, paths: Arc<dyn PluginPaths> }`。[`InvocationCore`](src/runtime.rs#L159) 是可 `Clone` 的 `'static` 最小上下文，供 `JoinSet` 克隆使用。
- **注册表**：[`ActionRegistry`](src/registry.rs#L37) 的 `entries` 按动作 id 索引，每个条目都带 `plugin_id: String`；`manifests` 用 `BTreeMap` 保证依赖解析顺序确定，另有 `surface_handlers` / `lifecycles` / `dependency_status` / `unavailable` / `usage`。[`register_plugin()`](src/registry.rs#L96) 先经 `flick_plugin::validate_manifest` 全量校验并查 id 冲突，全部通过才写入（all-or-nothing）；[`register_action()`](src/registry.rs#L79) 供测试/嵌入场景把单个动作挂到插件下（必要时补最小 manifest）；[`resolve_dependencies()`](src/registry.rs#L139) 委托 `flick_plugin::resolve_dependencies`，移除不可用插件的条目 / handler / 生命周期但保留 manifest，返回全部错误、不短路、可重复调用。
- **输入归一化**：[`normalize_path_input()`](src/runtime.rs#L63) 在调用入口只做一次 stat，把 `PathCandidate` 升级为 `File`；`SearchRoute` 跳过这一 I/O。`uri_to_path()` / `percent_decode()` 处理 `file://` 与 `file://localhost` 前缀。[`supported_inputs == [None]`](src/runtime.rs#L472) 的动作不做输入归一化。
- **活动调用清理**：[`ActiveGuard`](src/runtime.rs#L99) 的 `Drop`（[runtime.rs:106](src/runtime.rs#L106)）先 `try_lock` 同步移除；锁被占用时退化到 `Handle::spawn` 异步清理，避免 Drop 中 `.await`。
- **搜索评分**：`search` 是私有纯函数模块。[`MatchTier`](src/search.rs#L13) 分档基础分为描述 100 / 子串 300 / 词首拼音 520 / 首字母缩写 550 / 词首字面 600 / 全等拼音 700 / 全等字面 800；[`score_action()`](src/search.rs#L87) 先 `select_input()` 判断输入兼容，再匹配动作字段与插件关键词；`quality_score()` 字段权重 Name 40 / Keyword 25 / Id 10 / Description 0 并计入紧凑度；`category_rank()` 给 System 1 / Apps 2 / builtin 3 / 其它 4 / PluginExpansion 7。[frecency 提升上限 180](src/registry.rs#L269)，排序键依次为 `score → category_rank → title_len → 原始顺序`，先把候选按 `(plugin_id, action_id)` 稳定预排以保证确定性，最终截断到 100 条。
- **结果归一化**：`execution_route_items()` 把数组展开为 `ExecutionItem`（score 0.0，title 依次回退 title / name / label），丢弃 `Error` / `OpenSurface` 并写 warn，`MAX_EXECUTION_ITEMS_PER_ACTION = 20`（[runtime.rs:18](src/runtime.rs#L18)）。
- **常量与句柄**：`SEARCH_INVOKE_TIMEOUT_MS = 150`（[runtime.rs:24](src/runtime.rs#L24)）；`MAX_ACTION_RESULTS = 100`（[registry.rs:16](src/registry.rs#L16)）。[`InvocationHandle`](src/invocation.rs#L13) 提供 `new` / `cancel`（[invocation.rs:41](src/invocation.rs#L41)，已取消返回 false）/ `into_result`（[invocation.rs:53](src/invocation.rs#L53)）/ `abort`（[invocation.rs:63](src/invocation.rs#L63)），当前是预留给外部调用方的跟踪工具，`invoke()` 内部不使用它。

## 公开 API / 核心类型

| 项目 | 位置 | 说明 |
| --- | --- | --- |
| `ActionRuntime` | [runtime.rs:136](src/runtime.rs#L136) | 运行时主体：注册表、活动调用、宿主信息、Surface 会话与搜索取消代次 |
| `ActionRuntime::new` | [runtime.rs:180](src/runtime.rs#L180) | 构造空运行时；必须注入 `host_info` 与 `paths`，单测可用 `default()` 的 Fake |
| `register_action` / `register_plugin` | [runtime.rs:269](src/runtime.rs#L269)、[runtime.rs:283](src/runtime.rs#L283) | 单动作挂到插件下 / 全量校验后注册插件（所有动作都必须有插件归属） |
| `resolve_dependencies` / `activate_all_plugins` | [runtime.rs:310](src/runtime.rs#L310)、[runtime.rs:292](src/runtime.rs#L292) | 解析插件依赖 / 逐个激活生命周期（失败只告警） |
| `route_input` / `route` | [runtime.rs:343](src/runtime.rs#L343)、[runtime.rs:335](src/runtime.rs#L335) | 新输入路由 / 旧关键词路由（兼容未迁移调用方） |
| `search_invoke` / `begin_search_generation` | [runtime.rs:357](src/runtime.rs#L357)、[runtime.rs:420](src/runtime.rs#L420) | 搜索阶段并行试探 / 取消上一代查询 |
| `invoke` / `cancel` | [runtime.rs:472](src/runtime.rs#L472)、[runtime.rs:523](src/runtime.rs#L523) | 执行动作（select! 竞速）/ 触发协作式取消 |
| `execution_event` | [runtime.rs:436](src/runtime.rs#L436) | 处理执行项事件（固定 30s 超时） |
| `surface_event` / `surface_closed` | [runtime.rs:477](src/runtime.rs#L477)、[runtime.rs:503](src/runtime.rs#L503) | Surface 事件转发与会话关闭 |
| `InvocationHandle` | [invocation.rs:13](src/invocation.rs#L13) | 跟踪运行中调用：取消 / 等待结果 / 强制 abort |
| `ActionRegistry` | [registry.rs:37](src/registry.rs#L37) | 注册表：动作 / 插件清单 / Surface handler / 生命周期 / 使用统计 |
| `register_plugin` / `resolve_dependencies` | [registry.rs:96](src/registry.rs#L96)、[registry.rs:139](src/registry.rs#L139) | 注册与依赖解析的实际实现 |
| `search_input` / `search` | [registry.rs:269](src/registry.rs#L269)、[registry.rs:219](src/registry.rs#L219) | 新输入评分排序 / 旧关键词召回 |
| `MAX_ACTION_RESULTS` | [registry.rs:16](src/registry.rs#L16) | 搜索结果上限 100 |
| `MAX_EXECUTION_ITEMS_PER_ACTION` | [runtime.rs:18](src/runtime.rs#L18) | 单动作展开执行项上限 20 |
| `SEARCH_INVOKE_TIMEOUT_MS` | [runtime.rs:23](src/runtime.rs#L23) | 搜索阶段调用超时 150ms |

`lib.rs` 还在 [lib.rs:26](src/lib.rs#L26) 从 `flick-plugin` 重导出 `ActionHandler` / `ActionSpec` / `DependencyResolution` / `Plugin` / `PluginLifecycle` / `PluginRegistrationError` / `PluginRuntimeError` / `PluginUiFactory` / `SurfaceError` / `SurfaceHandler` / `SurfaceSession` / `validate_manifest`，因此下游无需直接依赖 `flick-plugin`。

## 模块 / 文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根：模块声明与公共 API 重导出 |
| [src/invocation.rs](src/invocation.rs) | `InvocationHandle`：外部跟踪运行中调用的句柄 |
| [src/registry.rs](src/registry.rs) | `ActionRegistry`：注册、查找、搜索评分与排序（含单元测试） |
| [src/search.rs](src/search.rs) | 私有纯函数评分层：匹配档位、字段权重、分类排序 |
| [src/runtime.rs](src/runtime.rs) | `ActionRuntime`：路由 / 调用 / 取消 / Surface 会话编排（含单元测试） |
| [tests/runtime_contract.rs](tests/runtime_contract.rs) | 路由 + 执行、取消、未知动作等主干契约测试 |
| [tests/search_invoke_contract.rs](tests/search_invoke_contract.rs) | 搜索阶段执行与执行项展开测试 |
| [tests/surface_runtime.rs](tests/surface_runtime.rs) | Surface 会话回填与事件转发测试 |
| [tests/plugin_registry.rs](tests/plugin_registry.rs) | 注册校验错误测试 |
| [tests/lifecycle_contract.rs](tests/lifecycle_contract.rs) | 启动激活插件测试 |

## 测试与常用命令

```bash
cargo test -p flick-runtime              # 全部单元测试 + 契约测试
cargo test -p flick-runtime --test runtime_contract
cargo test -p flick-runtime --test search_invoke_contract
cargo test -p flick-runtime --test surface_runtime
cargo check -p flick-runtime
cargo doc -p flick-runtime --open
```

## 与其他 crate 的关系

- `flick-domain`：提供 `ActionDescriptor` / `ActionRequest` / `ActionContext` / `ActionOutcome` / `InputContext` / `RouteItem` / `PluginManifest` 等输入输出类型。
- `flick-plugin`：定义插件与动作契约（`Plugin` / `ActionHandler` / `PluginLifecycle` / `SurfaceHandler` / 校验函数），本 crate 在根上重导出。
- `flick-application` / `flick-daemon`：协调器依赖 `RuntimePort` 窄接口，daemon 的 `RuntimeAdapter` 转发到 `ActionRuntime`。
- `flick-builtins` / 第三方插件：作为 `Plugin` 实现注册进同一个注册表，内置插件没有特权通道。
