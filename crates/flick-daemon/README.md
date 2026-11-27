# flick-daemon

Flick 的常驻后台进程：启动动作运行时、注册内置动作与插件、绑定全局快捷键、监听 Unix Socket 上的 IPC 请求，并在主线程把搜索面板 / 独立窗口（GPUI Shell）拉起为 UI。

它把「宿主能力」（`ActionRuntime`、Portal、快捷键、剪贴板、设置）与「UI 呈现」解耦：在生产拓扑中 daemon 是唯一持有 `ActionRuntime` 与 `PortalShortcutAdapter` 的进程，UI 只通过两条无界通道收发消息。同一份代码既能跑图形会话，也能跑无头（`--headless`）模式供 CI / 端到端测试使用。

## 为什么设计这个 crate

- **三类输入在此汇合**：IPC 客户端（`flick-cli` 等）、Portal 全局快捷键、GPUI UI 的意图，最终都交给同一个 `ApplicationCoordinator` → `ActionRuntime`。把这层收敛放进独立进程，UI 就不必知道运行时、Portal 或设置存储的存在，只面对 `UiToHost` / `HostToUi` 两条消息通道。
- **线程 / 进程模型决定拆分**：GPUI 必须在主线程跑且需要显式退出模式，而 `ActionRuntime`、Portal D-Bus 会话、IPC 服务和定期清理都是异步或阻塞工作。daemon 把它们放进独立 Tokio 后台线程（`catch_unwind` 包裹），主线程只负责 UI；任一端退出都能通过 `DaemonLifecycle` 关停令牌把另一端一起收掉。
- **UI 后端可替换的边界**：`RuntimePort` / `HostEventSink` / `IntentSink` 这三个端口由 daemon 用 `RuntimeAdapter` / `ChannelEventSink` / `ChannelIntentSink` 实现。UI 只通过 `UiEvent` / `HostToUi` 与这条边界对话，因此 daemon 可以独立升级宿主能力而不动 UI。
- **无头模式复用同一套组装**：`--headless` 跳过 GUI 与 Portal，但仍走 `HeadlessRuntime` + 协调器 + IPC + 设置 + 留存清理；端到端测试因此能在没有图形会话的机器上覆盖真实 socket 路径。

## 在 workspace 中的位置

```
全局快捷键(Portal) ──► intent 通道 ──┐
                                       ▼
IPC 客户端(flick-cli) ─► flick-daemon ─► ApplicationCoordinator ─► RuntimePort ─► ActionRuntime
   (Unix Socket)          │    ▲                                      ▲            (flick-runtime)
                          │    │ UiToHost / HostToUi（mpsc 通道）       │ 实现
                          ▼    ▼                                      │
                   flick-shell-gpui（GPUI 主线程）              RuntimeAdapter(flick-daemon)
```

依赖（见 [Cargo.toml](Cargo.toml)）：`flick-domain`、`flick-application`、`flick-runtime`、`flick-builtins`、`flick-settings`、`flick-data`、`flick-ipc`、`flick-adapters-linux`、`flick-shell-gpui`，外加 `tokio` / `tokio-util` / `clap` / `tracing` / `serde` / `serde_json` / `async-trait` / `thiserror`。它位于依赖图顶层，**没有任何 crate 在 Cargo 层反向依赖它**——`flick-cli` 只是在运行时通过 Unix Socket 与它通信的对等 IPC 客户端。

## 如何运行

入口是 [main.rs:29](src/main.rs#L29) 的 `main()`：初始化 `tracing_subscriber`（[main.rs:30](src/main.rs#L30)）→ `Args::parse`（[main.rs:37](src/main.rs#L37)）→ 解析 socket 路径（[main.rs:38](src/main.rs#L38)，`socket_path()` 委托 [lifecycle.rs:50](src/lifecycle.rs#L50)）→ 按 `--headless` 分派 [main.rs:53](src/main.rs#L53) 或 [main.rs:62](src/main.rs#L62)。

图形路径 [run_graphical:73](src/main.rs#L73) 的实际顺序：

1. 检查 Wayland 会话（[main.rs:110](src/main.rs#L110)，否则返回 `UnsupportedSession ... Use --headless.`）。
2. 建两条**无界 mpsc** 通道：`HostToUi`（daemon→UI）与 `UiToHost`（UI→daemon），见 [main.rs:119](src/main.rs#L119)；随后建 `DaemonLifecycle`（[main.rs:121](src/main.rs#L121)），并用 `Arc<Mutex<Option<String>>>` / `AtomicBool` 记录后台 panic（[main.rs:123](src/main.rs#L123)）。
3. 后台线程 [main.rs:134](src/main.rs#L134) 用 `catch_unwind` 包裹并自建 Tokio Runtime（[main.rs:135](src/main.rs#L135)–[main.rs:136](src/main.rs#L136)）：先解析数据目录与宿主信息，再 `ActionRuntime::new(host_info, paths)` → `register_builtin` → `activate_all_plugins`（[main.rs:162](src/main.rs#L162)）；`host_info` 与插件目录在构造时注入，不再事后 setter。
4. 用 `ChannelEventSink` / `RuntimeAdapter` / `ApplicationCoordinator::new` 组装协调器（[main.rs:148](src/main.rs#L148)–[main.rs:154](src/main.rs#L154)），并 tokio spawn 消费 `ui_cmd_rx` 的 `ui_worker`（[main.rs:158](src/main.rs#L158)）。
5. 建 intent 通道 + `PortalShortcutAdapter`（`ChannelIntentSink` + `AshpdPortalClient`，[main.rs:171](src/main.rs#L171)–[main.rs:177](src/main.rs#L177)）；`portal.initialize()` 成功后 `spawn_startup_auto_bind`（[main.rs:185](src/main.rs#L185)，受 `FLICK_DISABLE_AUTO_BIND` 控制），失败时限次重试 10 次 × 3s（[main.rs:197](src/main.rs#L197)，受 `FLICK_DISABLE_PORTAL_RETRY` 控制）。
6. tokio spawn `intent_worker`（[main.rs:229](src/main.rs#L229)）→ `init_settings`（[main.rs:241](src/main.rs#L241)）→ `create_ipc_dispatcher` + `bind_and_run_ipc`（[main.rs:242](src/main.rs#L242)–[main.rs:252](src/main.rs#L252)），绑定失败以 `ShutdownReason::StartupFailed` 关停。
7. 主线程加载初始主题（[main.rs:320](src/main.rs#L320)）后进入 `flick_shell_gpui::run_with_theme(ui_cmd_tx, host_event_rx, initial_theme)`（[main.rs:321](src/main.rs#L321)）——**不是** `run()`。UI 退出即 `request_shutdown(ShutdownReason::ShellExited)`（[main.rs:322](src/main.rs#L322)），随后 join 后台线程（[main.rs:323](src/main.rs#L323)）。
8. SIGTERM / Ctrl-C 的 `tokio::select!` 在 [main.rs:268](src/main.rs#L268)：发 `HostToUi::Shutdown`（[main.rs:299](src/main.rs#L299)）→ `portal.shutdown()`（[main.rs:300](src/main.rs#L300)）→ `bootstrap::shutdown`（[main.rs:302](src/main.rs#L302)）→ join worker；IPC `Shutdown` 走同一令牌。后台线程 panic 时也会请求关停并清理 socket（[main.rs:309](src/main.rs#L309)–[main.rs:326](src/main.rs#L326)）。

无头路径 [run_headless:595](src/bootstrap.rs#L595) 跳过 UI 与 Portal，只有 `HeadlessRuntime`（[headless.rs:10](src/headless.rs#L10)）+ 协调器 + IPC 服务 + 设置 + 留存清理，便于端到端测试。

## 内部技术与架构细节

- **关停状态机**：`DaemonLifecycle`（[lifecycle.rs:14](src/lifecycle.rs#L14)）= `CancellationToken` + `OnceLock<ShutdownReason>`；首次 `request_shutdown()`（[lifecycle.rs:25](src/lifecycle.rs#L25)）写入原因并 `token.cancel()`，因此 `Signal` / `Ipc` / `ShellExited` / `StartupFailed`（[lifecycle.rs:7](src/lifecycle.rs#L7)）中**第一个原因胜出**。IPC `Shutdown` 特意在回包后才请求关停（`after_response`，[bootstrap.rs:357](src/bootstrap.rs#L357)），避免客户端收不到响应。
- **通道拓扑**：`UiToHost` 由 UI 写、`ui_worker` 读，转成协调器调用；`HostToUi` 由 `ChannelEventSink`（[bootstrap.rs:86](src/bootstrap.rs#L86)）写、GPUI 主线程的运行事件泵读。Portal 意图走 `ChannelIntentSink`（[bootstrap.rs:96](src/bootstrap.rs#L96)）→ intent 通道 → `intent_worker`。两条通道都是 `tokio::sync::mpsc::unbounded_channel`。
- **IPC 分发**：`DaemonIpcDispatcher::dispatch`（[bootstrap.rs:122](src/bootstrap.rs#L122)）处理 Toggle / Trigger / Status / Doctor / ShortcutsConfigure / ShortcutsReconnect / Shutdown / Show / Hide / SettingsList / SettingsGet / SettingsSet / SettingsReset；`Trigger` 有 30s `deadline` 并把 `ActionOutcome` 映射成 IPC 结果。`doctor` 用 `plugin_dependency_report`（[bootstrap.rs:369](src/bootstrap.rs#L369)）返回 `BTreeMap`，保证输出顺序稳定。
- **后台维护**：`spawn_retention_worker`（[bootstrap.rs:513](src/bootstrap.rs#L513)）用 24 小时 `interval` + `spawn_blocking` 清理过期数据（`UNINSTALLED_RETENTION_DAYS = 30`），并随关停令牌退出。`init_settings`（[bootstrap.rs:855](src/bootstrap.rs#L855)）优先使用 `FLICK_MASTER_KEY` 环境变量，缺失时降级为 `UnavailableMasterKeySource` 而不是启动失败；它只取运行时中的插件清单并交给 [`build_settings_at`](src/bootstrap.rs#L876) 构造服务，**不注入运行时**；调用点需显式调用 [`wire_settings`](src/bootstrap.rs#L922) 完成注入。
- **主题与设置持久化**：`load_initial_theme` / `initial_theme_at`（[bootstrap.rs:788](src/bootstrap.rs#L788) / [bootstrap.rs:802](src/bootstrap.rs#L802)）读取 UI 主题配置；`migrate_legacy_theme` 把旧缓存迁移到 `shell-gpui.toml.migrated`。这些工作放在 daemon 而非 UI，确保无头会话也能一致处理。
- **资源清理语义**：`shutdown`（[bootstrap.rs:578](src/bootstrap.rs#L578)）只 `remove_file(socket_path)`，不会删除父目录；socket 路径由 `--socket` 或 `$XDG_RUNTIME_DIR/flick/flick.sock` 决定，systemd 用户单元见 [resources/flick.service](resources/flick.service)。

## 公开 API / 核心类型

| 类型 / 函数 | 位置 | 作用 |
| --- | --- | --- |
| `DaemonLifecycle` | [lifecycle.rs:14](src/lifecycle.rs#L14) | `CancellationToken` + `OnceLock<ShutdownReason>`，第一次关停原因胜出 |
| `ShutdownReason` | [lifecycle.rs:7](src/lifecycle.rs#L7) | `Signal` / `Ipc` / `ShellExited` / `StartupFailed` |
| `resolve_socket_path` | [lifecycle.rs:50](src/lifecycle.rs#L50) | socket 路径：`--socket` 优先，否则 `$XDG_RUNTIME_DIR/flick/flick.sock` |
| `RuntimeAdapter` | [bootstrap.rs:27](src/bootstrap.rs#L27) | 实现 `RuntimePort`，把调用逐字转发给 `ActionRuntime` |
| `ChannelEventSink` / `ChannelIntentSink` | [bootstrap.rs:86](src/bootstrap.rs#L86) / [bootstrap.rs:96](src/bootstrap.rs#L96) | 把宿主事件 / 快捷键意图写入通道 |
| `DaemonIpcDispatcher` | [bootstrap.rs:108](src/bootstrap.rs#L108) | 实现 `IpcDispatcher`，处理 Toggle / Trigger / Status / Doctor / Shutdown 等 |
| `create_ipc_dispatcher` | [bootstrap.rs:531](src/bootstrap.rs#L531) | 组装分发器（可注入 portal 快照，无头时为 `None`） |
| `DaemonHandle` / `bind_and_run_ipc` / `shutdown` | [bootstrap.rs:553](src/bootstrap.rs#L553) / [bootstrap.rs:559](src/bootstrap.rs#L559) / [bootstrap.rs:578](src/bootstrap.rs#L578) | IPC 服务句柄、绑定运行、关停时只删 socket 不删父目录 |
| `HeadlessRuntime` | [headless.rs:10](src/headless.rs#L10) | 无 UI 的运行时，注册内置动作并激活插件 |

IPC 命令在 [bootstrap.rs:122](src/bootstrap.rs#L122) 的 `dispatch` 中分发；`Shutdown` 在 [bootstrap.rs:357](src/bootstrap.rs#L357) 的 `after_response` 里回包后才请求关停，`doctor` 会合并会话 / Portal / IPC 事实并报插件依赖状态（[bootstrap.rs:369](src/bootstrap.rs#L369)）。

## 模块 / 文件清单

| 文件 | 职责 |
| --- | --- |
| [src/main.rs](src/main.rs) | CLI 参数、图形/无头分派、线程与通道编排、信号关停 |
| [src/bootstrap.rs](src/bootstrap.rs) | 适配器、IPC 分发器、socket 绑定与清理、主题/设置初始化、无头运行 |
| [src/lifecycle.rs](src/lifecycle.rs) | 关停令牌与原因、socket 路径解析 |
| [src/headless.rs](src/headless.rs) | `HeadlessRuntime` 构造 |
| [resources/flick.service](resources/flick.service) | systemd 用户单元：`flick-daemon --foreground`、`Restart=on-failure` |
| [tests/bootstrap_contract.rs](tests/bootstrap_contract.rs) | 适配器/协调器/关停契约测试（用 `#[path]` 直接引入源码单元） |
| [tests/headless_e2e.rs](tests/headless_e2e.rs) | 真实 Unix Socket 上的端到端测试 |

## 测试与常用命令

```bash
cargo test -p flick-daemon                          # 全部单测 + 契约测试
cargo test -p flick-daemon --test bootstrap_contract # 适配器/协调器契约
cargo test -p flick-daemon --test headless_e2e       # 真实 socket e2e（需 Unix Socket）
cargo run -p flick-daemon -- --foreground            # 图形模式
cargo run -p flick-daemon -- --headless              # 无头模式
cargo run -p flick-daemon -- --socket /tmp/flick.sock # 指定 socket
```

环境变量：`FLICK_DISABLE_AUTO_BIND=1` 关闭启动自动绑定快捷键，`FLICK_DISABLE_PORTAL_RETRY=1` 关闭 Portal 初始化失败重试，`FLICK_MASTER_KEY` 提供设置加密主密钥。

## 与其他 crate 的关系

- `flick-application`：用其 `ApplicationCoordinator` 与 `HostEventSink` / `IntentSink` 端口；`flick-runtime`：真正执行动作。
- `flick-adapters-linux`：`PortalShortcutAdapter` + `AshpdPortalClient` 提供 Linux 全局快捷键与 Portal 快照。
- `flick-shell-gpui`：daemon 在主线程调用其 `run_with_theme(host_tx, host_rx, initial_theme)` 起 UI，是 daemon 唯一的 UI 后端。
- `flick-ipc`：Unix Socket 服务端与 `IpcDispatcher` 契约；`flick-cli` 是通过该 socket 通信的运行时客户端，Cargo 层并不依赖本 crate。
- `flick-builtins`：`register_builtin` 注册内置动作（如 echo、状态查询）。
- `flick-settings` / `flick-data`：提供设置存储与数据目录 / 留存清理能力。
