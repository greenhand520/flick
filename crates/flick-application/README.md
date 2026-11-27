# flick-application

Flick 的应用层 crate：以 `ApplicationCoordinator` 为枢纽，把 UI 事件与快捷键意图桥接到 Runtime，并持有与 GUI 无关的搜索面板状态机。

## 在 workspace 中的位置与依赖

`flick-application` 处于 UI/CLI 外壳与 Runtime 之间，是路由/调用逻辑的唯一所有者；UI 外壳与 CLI 只是转发消息的薄封装（见 [lib.rs:7](src/lib.rs#L7)）。它只依赖领域类型与两个基础设施 crate，通过窄端口 trait 与具体实现解耦，因此本身不需要 tokio 之外的运行时知识。

| 依赖 | 用途 |
| --- | --- |
| `flick-domain`（path） | 全部协议类型：`UiToHost`、`HostToUi`、`RouteItem`、`ActionOutcome` 等 |
| `tokio` | 用 `tokio::spawn` 把搜索阶段执行放到后台任务 |
| `async-trait` | `RuntimePort` 的异步 trait |

依赖声明见 [Cargo.toml:6](Cargo.toml#L6)。架构速览见 [lib.rs:7](src/lib.rs#L7)：`UI ──► ApplicationCoordinator ──► Runtime`，回程经 `HostEventSink ──► UI`。

## 为什么设计这个 crate

GPUI Shell 设计规格规定「UI 与实现严格分离」：Shell 只做输入采集、渲染与意图翻译，不承载 Action 执行、路由或持久化，并且只能单向依赖 `flick-domain`，禁止依赖 `flick-runtime` / `flick-application` 的实现（`docs/2026-11-24-gpui-shell-design.md:14-18, 40-48`）。为此需要一个不绑定到任何窗口框架的用例层：项目把「用例、协调器接口、Port trait、入口编排」划归本 crate，明确它不承担具体 Linux API 与 GUI widget。

拆出的另一个收益是**状态机可脱离 GUI 复用**：`presenter.rs` 只依赖 `flick-domain`，不引入 gpui 或窗口类型，因此 `PanelState` 能在无 GUI 环境单测，也可被其它外壳复用；Shell 设计也要求新外壳沿用同一套与框架无关的状态机（`docs/2026-11-24-gpui-shell-design.md:64-71`）。两阶段结果链路的设计又把职责切得更清楚：`route_input` 是纯内存微秒级计算、`search_invoke` 可耗至 150ms，首屏不能被慢插件拖住；而代次取消放在 Runtime 内部，协调器只传 `generation`、不持有令牌注册表，避免 `RuntimePort` 泄漏实现细节（`docs/superpowers/specs/2026-11-27-execution-pipeline-design.md:27-43`）。

## 如何运行

本 crate 是库，没有 `main`、不占进程也不自带事件循环。组合根（典型是 `flick-daemon`）构造 `ApplicationCoordinator::new(Arc<dyn RuntimePort>, Arc<dyn HostEventSink>)`（[coordinator.rs:86](src/coordinator.rs#L86)），并在自己的异步事件循环中 await `handle_ui(event: UiToHost)`（[coordinator.rs:111](src/coordinator.rs#L111)）；快捷键路径调用同步的 `handle_intent(UiIntent)`（[coordinator.rs:265](src/coordinator.rs#L265)）。`PanelState::update` / `update_at`（[presenter.rs:184](src/presenter.rs#L184) / [presenter.rs:189](src/presenter.rs#L189)）则由 UI 外壳直接驱动，与协调器互不持有。

一次 `QueryChanged` 的调用链是：`QueryTracker::set_current`（[query.rs:33](src/query.rs#L33)）登记新代次 → `SearchInputSnapshot::to_action_input` 与 `InputContext::from_snapshot` 组装输入 → `await runtime.route_input(...)` 得到候选 → 回送 `HostToUi::Routes` 形成首屏 → `tokio::spawn` 后台任务调用 `runtime.search_invoke(generation, input)`，完成后再异步补发 `HostToUi::ExecutionResults`。这样首屏只等纯内存的路由，慢插件的子项稍后浮现。

`Invoke` 分支先由 `RouteSnapshot::contains`（[coordinator.rs:68](src/coordinator.rs#L68)）校验 item_id 与代次，校验失败回送 `UnknownAction` 错误，避免 UI 永远停在「调用中」；通过后构造 `Caller::ui("search-panel")`、`ActionSource::Ui`、30 秒 deadline 的 `ActionRequest` 交给 `runtime.invoke`，成功才通过 `record_usage_on_success`（[coordinator.rs:286](src/coordinator.rs#L286)）累加 frecency（错误结果不计）。`ExecutionActivated` 丢弃旧代次后转交 `runtime.execution_event`；`Cancel`、`SearchPanelClosed`、`SurfaceEvent`、`SurfaceClosed` 分别取消调用、清空快照、转交 Surface 事件或通知 Runtime。

线程与共享状态：`RuntimePort` / `HostEventSink` 都是 `Send + Sync` trait 对象（[ports.rs:19](src/ports.rs#L19)、[ports.rs:116](src/ports.rs#L116)）；协调器用 `Mutex<RouteSnapshot>` 保护候选并集、`AtomicBool` 标记面板可见性（`toggle_search_panel` 用 `fetch_xor` 翻转，[coordinator.rs:296](src/coordinator.rs#L296)），`QueryTracker` 用 `AtomicU64` 配合 `SeqCst` 读写代次（0 表示无查询）。`new` 会在进程内创建 `InMemoryUsageStore` 并通过 `set_usage_store` 把同一份存储注入 Runtime。

## 内部技术与架构细节

**窄端口。** `RuntimePort`（[ports.rs:19](src/ports.rs#L19)）暴露 `route`（[L21](src/ports.rs#L21)）、`route_input`（[L24](src/ports.rs#L24)）、`search_invoke`（[L49](src/ports.rs#L49)）、`invoke`（[L75](src/ports.rs#L75)）、`cancel`（[L81](src/ports.rs#L81)）、`surface_event`（[L87](src/ports.rs#L87)）、`execution_event`（[L65](src/ports.rs#L65)）等方法；其中 `route_input`、`search_invoke`、`execution_event`、`dependency_status`（[L100](src/ports.rs#L100)）、`registered_plugin_ids`（[L107](src/ports.rs#L107)）都有默认实现，测试替身只需实现真正关心的方法。`route_input` 的默认实现按 `InputFacet` 折算 `InputType` 再回退到 `route`。`HostEventSink::send`（[ports.rs:118](src/ports.rs#L118)）是回程唯一出口。

**候选快照与代次。** `RouteSnapshot`（[coordinator.rs:42](src/coordinator.rs#L42)）保存本会话已发出的候选并集与 `latest_generation`；`record` 按 `(item_id, action_id)` 去重，`contains` 拒绝未来代次与未知候选，`clear` 在面板关闭时重置。`QueryTracker`（[query.rs:18](src/query.rs#L18)）的 `is_current`（[L38](src/query.rs#L38)）让乱序完成的旧批次结果被丢弃。

**累积与会话。** `merge_batch`（[accumulate.rs:19](src/accumulate.rs#L19)）按 `RouteItemKey` 去重、旧项保留原位、同 key 保留高分，超过 `MAX_ACCUMULATED_RESULTS = 200`（[L9](src/accumulate.rs#L9)）时用 `score.total_cmp` 稳定排序截断并恢复相对顺序。`merge_execution_batch`（[L59](src/accumulate.rs#L59)）把执行项插到父 Action 之后，继承父分；同 key 时后到者更新内容但分数钉在父分。`SearchSession`（[accumulate.rs:112](src/accumulate.rs#L112)）含 `accumulated / manual / kind / active`，`note_kind` 在种类或模式变化时清空，非手动输入批次直接替换，执行项合并则不受 `manual` 影响。

**面板状态机。** `PanelState`（[presenter.rs:112](src/presenter.rs#L112)）是无窗口的纯状态，`update_at` 把 `PanelEvent`（[L21](src/presenter.rs#L21)）归约为 `PanelAction`（[L62](src/presenter.rs#L62)），后者只有 `SendToHost(UiToHost)` 与 `StartDebounce { revision }` 两种输出。`SelectionState`（[L68](src/presenter.rs#L68)）管理选中项；`PANEL_RESUME_WINDOW = 10s`（[L17](src/presenter.rs#L17)）让关闭后 10 秒内的重新打开复用会话，超时则 `reset_expired_session`；`on_hide` 递增 `debounce_revision` 使迟到的防抖结果无效，并发出 `SearchPanelClosed` 与 `SurfaceClosed`。

**frecency。** `InMemoryUsageStore`（[usage.rs:12](src/usage.rs#L12)）用 `Mutex<HashMap<ActionId, UsageStat>>` 在进程内保存使用统计，退出即丢失；`frecency_boost` 对从未使用的 Action 返回 0。

## 公开 API 与核心类型

重导出见 [lib.rs:25](src/lib.rs#L25)。

### 协调器（[coordinator.rs](src/coordinator.rs)）

- `ApplicationCoordinator`（[coordinator.rs:25](src/coordinator.rs#L25)）持有 `Arc<dyn RuntimePort>`、`Arc<dyn HostEventSink>`、`QueryTracker`、`RouteSnapshot` 与 frecency 存储。`new`（[L86](src/coordinator.rs#L86)）把同一份 `UsageStore` 注入 Runtime。
- `handle_ui`（[coordinator.rs:111](src/coordinator.rs#L111)）分派 `UiToHost`：`QueryChanged` 先同步路由出首屏，再 `tokio::spawn` 异步补发执行项；`Invoke` 先做候选身份校验，再构造 `ActionRequest` 调用；`SurfaceEvent` / `SurfaceClosed` 转交 Runtime。
- `handle_intent`（[coordinator.rs:265](src/coordinator.rs#L265)）处理快捷键唤起等意图——只发 `HostToUi`，**不**调用 `runtime.invoke`；`toggle_search_panel`（[L296](src/coordinator.rs#L296)）翻转可见状态。
- 内部 `RouteSnapshot`（[coordinator.rs:42](src/coordinator.rs#L42)）保存本会话已发出的候选并集，`contains`（[L68](src/coordinator.rs#L68)）拒绝陈旧代次与未知 item_id 的回传调用。

### 窄端口（[ports.rs](src/ports.rs)）

| trait | 位置 | 关键方法 |
| --- | --- | --- |
| `RuntimePort` | [ports.rs:19](src/ports.rs#L19) | `route`（[L21](src/ports.rs#L21)）、`route_input`（[L24](src/ports.rs#L24)）、`search_invoke`（[L49](src/ports.rs#L49)）、`invoke`（[L75](src/ports.rs#L75)）、`cancel`（[L81](src/ports.rs#L81)）、`surface_event`（[L87](src/ports.rs#L87)）、`execution_event`（[L65](src/ports.rs#L65)） |
| `HostEventSink` | [ports.rs:116](src/ports.rs#L116) | `send(&self, event: HostToUi)`（[L118](src/ports.rs#L118)） |

`route_input`、`search_invoke`、`execution_event`、`dependency_status`、`registered_plugin_ids` 均有默认实现，便于测试替身逐步迁移。

### 面板状态机与累积（[presenter.rs](src/presenter.rs)、[accumulate.rs](src/accumulate.rs)）

- `PanelState`（[presenter.rs:112](src/presenter.rs#L112)）是与原生窗口无关的逻辑状态；`update` / `update_at`（[L184](src/presenter.rs#L184) / [L189](src/presenter.rs#L189)）把 `PanelEvent`（[L21](src/presenter.rs#L21)）变成 `PanelAction`（[L62](src/presenter.rs#L62)）。`PANEL_RESUME_WINDOW = 10s`（[L17](src/presenter.rs#L17)）让隐藏后短暂保留会话。
- `SelectionState`（[presenter.rs:68](src/presenter.rs#L68)）管理列表选中项。
- `merge_batch`（[accumulate.rs:19](src/accumulate.rs#L19)）按 `RouteItemKey` 去重、保留旧项位置；`merge_execution_batch`（[L59](src/accumulate.rs#L59)）把执行项插到父项之后并继承父分。`MAX_ACCUMULATED_RESULTS = 200`（[L9](src/accumulate.rs#L9)）。
- `SearchSession`（[accumulate.rs:112](src/accumulate.rs#L112)）对齐面板可见性；只有手动输入的批次才累积。

### 辅助类型

- `QueryTracker`（[query.rs:18](src/query.rs#L18)）用原子代次丢弃乱序结果；`set_current` / `is_current` 见 [L33](src/query.rs#L33)、[L38](src/query.rs#L38)。
- `InMemoryUsageStore`（[usage.rs:12](src/usage.rs#L12)）实现 `UsageStore`，仅在进程内保存 frecency；`new` / `stat` 见 [L19](src/usage.rs#L19)、[L24](src/usage.rs#L24)。

## 模块与文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根：模块声明与重导出 |
| [src/coordinator.rs](src/coordinator.rs) | `ApplicationCoordinator`：UI / 意图分派与候选校验 |
| [src/ports.rs](src/ports.rs) | `RuntimePort` / `HostEventSink` 窄端口 |
| [src/presenter.rs](src/presenter.rs) | 面板状态机 `PanelState` / `PanelEvent` / `PanelAction` |
| [src/accumulate.rs](src/accumulate.rs) | 结果累积与会话 |
| [src/query.rs](src/query.rs) | 查询代次跟踪 |
| [src/usage.rs](src/usage.rs) | 内存 frecency 存储 |
| [tests/coordinator_flow.rs](tests/coordinator_flow.rs) | 伪造端口驱动的端到端流程测试 |

## 测试与常用命令

```bash
cargo test -p flick-application              # 单元 + 集成测试
cargo test -p flick-application --test coordinator_flow
cargo clippy -p flick-application --all-targets -- -D warnings
```

集成测试用伪造的 `RuntimePort` / `HostEventSink`，不需要真实 Runtime、GUI 或系统服务。

## 与其他 crate 的关系

- **依赖**：`flick-domain`（领域类型）；`tokio`、`async-trait`（基础设施）。
- **被依赖**：`flick-daemon`（其 `RuntimeAdapter` 实现 `RuntimePort`）、`flick-shell-gpui`、`flick-adapters-linux`、`flick-testkit`。
- `flick-runtime` 实现真正的路由/调用能力但不直接依赖本 crate；二者通过 `RuntimePort` 解耦，胶水在 `flick-daemon`。
