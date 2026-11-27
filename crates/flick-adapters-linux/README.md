# flick-adapters-linux

Flick 的 Linux 平台适配器 crate：封装 xdg-desktop-portal 的 GlobalShortcuts、Portal 应用身份注册，以及会话诊断与快捷键状态管理。

## 在 workspace 中的位置与依赖

本 crate 把 Linux 桌面特有的 D-Bus / Portal / Wayland 细节隔离在适配器层，供守护进程等上层调用。

| 依赖 | 用途 |
| --- | --- |
| `flick-domain`（path） | `HostInfo`、`UiIntent`、`APP_ID` 等领域类型 |
| `flick-application`（path） | 应用层抽象 |
| `ashpd`（features `global_shortcuts`）/ `zbus` 5.18 | Portal GlobalShortcuts 与 D-Bus 会话总线 |
| `resvg` | 把内嵌 SVG 光栅化为 hicolor 图标 |
| `tokio` / `futures-util` / `async-trait` | 异步运行时、事件流、异步 trait |
| `serde` / `serde_json` / `thiserror` / `tracing` | 序列化、错误、日志 |

依赖声明见 [Cargo.toml](Cargo.toml)。

## 为什么设计这个 crate

从依赖关系看，`flick-daemon` 需要接入真实 Linux 桌面（Portal 全局快捷键、会话探测、主机信息），但桌面生态的 D-Bus 会话、`ashpd`/`zbus` 版本与 GNOME/Mutter 的 app-id 要求都是易变且高度平台相关的细节。把这些细节收敛进独立 crate，上层只面对 `PortalShortcutAdapter`、`UiIntent`、`ShortcutState` 这些稳定抽象，不必接触 ashpd 的 ResponseError、D-Bus 对象路径或图标光栅化。方向也是单向的：本 crate 依赖 `flick-domain` / `flick-application`，daemon 依赖本 crate，本 crate 不反向依赖 daemon；`AshpdPortalClient` 通过 `PortalClient` trait（[portal_client.rs:69](src/portal_client.rs#L69)）注入，便于用假客户端测试状态机。设计文档 `docs/flick-service.md` 中「Portal app id 注册与自动绑定」「桌面快捷键回退」两节，对应的正是本 crate `portal_identity.rs` 与 `portal.rs` 承担的那条平台边界。

## 如何运行

本 crate 是库，没有入口；它的生命周期由 `flick-daemon` 驱动，围绕一个 Portal GlobalShortcuts 会话展开：

- **装配**：daemon 在 `flick-daemon/src/main.rs:172` 用 `PortalShortcutAdapter::new_with_client` 构造适配器，注入真实的 `AshpdPortalClient`（`flick-daemon/src/main.rs:77-78`）。`new()`（[portal.rs:128](src/portal.rs#L128)）只是占位客户端，从不模拟成功绑定。
- **初始化**：`initialize()`（[portal.rs:182](src/portal.rs#L182)）依次 `ensure_running` → `ensure_event_consumer` → `open_session_and_list`；只开会话并列出已有绑定，**不打开授权 UI、不自动绑定**。
- **配置快捷键**：`configure_search_shortcut()`（[portal.rs:190](src/portal.rs#L190)）是唯一会调用 Portal `BindShortcuts` 的路径——若本会话已尝试过绑定或会话不活跃，就先开新会话，再在等待 Portal 前把 `bind_attempted_in_current_session` 置位，回来后做代次校验，防止旧任务覆盖新状态。用户取消（`UserCancelled`）→ `Denied`，其它错误 → 记录到会话。
- **事件循环**：`AshpdPortalClient` 订阅 Portal 的 session 事件，`handle_portal_event_inner`（[portal.rs:551](src/portal.rs#L551)）处理：`Activated` 且 `shortcut_id == show-search` → `intent_sink.send_intent(UiIntent::ToggleSearchPanel)`；`SessionClosed` / `TransportError` → `begin_recovery` 进入退避恢复。
- **重连**：`reconnect()`（[portal.rs:297](src/portal.rs#L297)）只重新开会话并列绑定，**绝不绑定**——这是 Phase 0 不变量：每个 Portal 会话最多尝试一次绑定，只有显式配置命令才绑定，自动恢复不开授权 UI。
- **关停**：`shutdown()`（[portal.rs:315](src/portal.rs#L315)）先广播关闭并 cancel，再取 `operation_lock`，abort + join 事件/恢复任务，最后 `close_session`，幂等。

**Phase 0 不变量**由双代次计数器（`PortalInner` 的 `session_generation` / `recovery_generation`，[portal.rs:78](src/portal.rs#L78)，`wrapping_add`）守护：任何超时/慢响应回来时先用代次校验，过期的任务不能发布 `Bound` 或覆盖实时状态。

## 关键常量与桌面集成点

| 项 | 值 / 说明 | 出处 |
| --- | --- | --- |
| 默认应用 ID | `io.github.greenhand520.Flick`（来自 `flick_domain::APP_ID`） | [portal_identity.rs:26](src/portal_identity.rs#L26)、`flick-domain/src/lib.rs:24` |
| 图标尺寸 | `[16, 22, 24, 32, 48, 64, 96, 128, 256, 512]` | [portal_identity.rs:44](src/portal_identity.rs#L44) |
| 搜索快捷键 ID | `show-search` | [session.rs:78](src/session.rs#L78) |
| 默认触发键 | `Super+Space` | [session.rs:86](src/session.rs#L86) |
| 重连退避 | `[2s, 5s, 15s, 60s]`，上限 300s | [shortcut_state.rs:52](src/shortcut_state.rs#L52) |
| Portal D-Bus 名 | `org.freedesktop.portal.Desktop` / `org.freedesktop.portal.Session` | [portal_client.rs:529](src/portal_client.rs#L529) |
| 注册接口 | `org.freedesktop.host.portal.Registry.Register` | [portal_identity.rs:4](src/portal_identity.rs#L4) |

身份文件写到 `$XDG_DATA_HOME/icons/hicolor/<N>x<N>/apps/<app_id>.png`（`resvg` 光栅化内嵌 SVG）与 `$XDG_DATA_HOME/applications/<app_id>.desktop`（`Exec=<current_exe> --foreground`，见 [portal_identity.rs:94](src/portal_identity.rs#L94)）。注册时把 `Connection already associated with an application ID` 视为成功（[portal_identity.rs:140](src/portal_identity.rs#L140)）。诊断通过环境变量 `XDG_SESSION_TYPE`、`WAYLAND_DISPLAY`、`XDG_RUNTIME_DIR`、`DBUS_SESSION_BUS_ADDRESS`、`XDG_CURRENT_DESKTOP` 判定会话（[diagnostics.rs:39](src/diagnostics.rs#L39)）；窗口管理器由 `XDG_CURRENT_DESKTOP` + `XDG_SESSION_DESKTOP` 识别（GNOME→Mutter、KDE/Plasma→KWin、Hyprland、Sway、Weston，见 [host_info.rs:54](src/host_info.rs#L54)）。

> 关键实现约束：`AshpdPortalClient::connection()`（[portal_client.rs:173](src/portal_client.rs#L173)）使用**专用 session-bus 连接**而非 ashpd 全局连接，避免 gpui 先污染全局连接导致 GNOME 报 `org.freedesktop.portal.Error.NotAllowed: An app id is required`（[portal_client.rs:109](src/portal_client.rs#L109)）。`PortalSessionId`（[portal_client.rs:30](src/portal_client.rs#L30)）是独立的 `u64`，刻意不与 D-Bus 对象路径绑定。

## 公开 API 与核心类型

### Portal 适配器（[portal.rs](src/portal.rs)）

- `IntentSink`（[portal.rs:24](src/portal.rs#L24)）：`fn send_intent(&self, intent: UiIntent)`，把 Portal 激活转成 UI 意图。
- `PortalShortcutAdapter`（[portal.rs:68](src/portal.rs#L68)）：`new()`（占位客户端，[portal.rs:128](src/portal.rs#L128)）、`new_with_client()`（[portal.rs:133](src/portal.rs#L133)）、`initialize()`（[portal.rs:182](src/portal.rs#L182)）、`configure_search_shortcut(preferred_trigger)`（[portal.rs:190](src/portal.rs#L190)）、`reconnect()`（[portal.rs:297](src/portal.rs#L297)）、`diagnostics()`（[portal.rs:306](src/portal.rs#L306)）、`shutdown()`（[portal.rs:315](src/portal.rs#L315)）。

### Portal 客户端（[portal_client.rs](src/portal_client.rs)）

- `PortalClient` trait（[portal_client.rs:69](src/portal_client.rs#L69)）：`open_session_and_list`、`activate_session_events`、`bind_show_search`、`close_session`、`take_event_receiver`。
- `AshpdPortalClient`（[portal_client.rs:101](src/portal_client.rs#L101)）：真实 GlobalShortcuts 客户端；`PortalEvent`（[portal_client.rs:48](src/portal_client.rs#L48)）含 `Activated { session_id, shortcut_id, activation_token }` / `SessionClosed { session_id }` / `TransportError { session_id, message }`。`PortalSessionId`（[portal_client.rs:30](src/portal_client.rs#L30)）、`PortalSession { id, bindings }`（[portal_client.rs:41](src/portal_client.rs#L41)）。

### 会话与状态（[session.rs](src/session.rs)、[shortcut_state.rs](src/shortcut_state.rs)）

- `ShortcutBinding { id, description, preferred_trigger, actual_trigger }`（[session.rs:8](src/session.rs#L8)）、`ShortcutActivation`（[session.rs:17](src/session.rs#L17)）、`ShortcutError`（[session.rs:24](src/session.rs#L24)，含 `Unavailable` / `UserCancelled` / `RequestEnded` / `SessionError` / `Internal`）。
- `ShortcutState`（[shortcut_state.rs:8](src/shortcut_state.rs#L8)）：7 个变体 `Unconfigured` / `Configuring` / `Bound` / `Denied` / `Recovering { attempt }` / `Error { operation, message }` / `Unavailable { reason }`；`Denied`（用户取消）与 `Error`（操作失败）是两类不同的运维状态。
- `PortalSnapshot`（[shortcut_state.rs:35](src/shortcut_state.rs#L35)）、`PortalDiagnosticError`（[shortcut_state.rs:27](src/shortcut_state.rs#L27)）、`RecoveryBackoff`（[shortcut_state.rs:44](src/shortcut_state.rs#L44)，`new()` 见 [shortcut_state.rs:50](src/shortcut_state.rs#L50)）。

### 诊断与身份（[diagnostics.rs](src/diagnostics.rs)、[host_info.rs](src/host_info.rs)、[portal_identity.rs](src/portal_identity.rs)）

- `SessionDiagnostics` / `SessionType` / `probe_session()`（[diagnostics.rs:11](src/diagnostics.rs#L11)、[diagnostics.rs:39](src/diagnostics.rs#L39)）。
- `collect_host_info()`（[host_info.rs:10](src/host_info.rs#L10)）、`detect_window_manager()`（[host_info.rs:54](src/host_info.rs#L54)）。
- `ensure_icons` / `ensure_desktop_entry` / `register_portal_app_id_with_connection`（[portal_identity.rs:50](src/portal_identity.rs#L50)、[portal_identity.rs:94](src/portal_identity.rs#L94)、[portal_identity.rs:169](src/portal_identity.rs#L169)）。
- `FLICK_ICON_SVG`（[flick_icon.rs:11](src/flick_icon.rs#L11)）：内嵌 1024×1024 矢量图标。

## 模块与文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根：8 个模块声明 |
| [src/portal.rs](src/portal.rs) | 适配器主体：状态机、事件处理、退避恢复 |
| [src/portal_client.rs](src/portal_client.rs) | ashpd/zbus Portal 客户端实现 |
| [src/portal_identity.rs](src/portal_identity.rs) | 应用 ID、图标与 desktop 文件、Portal 注册 |
| [src/session.rs](src/session.rs) | 快捷键绑定/激活/错误类型 |
| [src/shortcut_state.rs](src/shortcut_state.rs) | 状态枚举、快照、退避策略 |
| [src/diagnostics.rs](src/diagnostics.rs) | 会话环境探测 |
| [src/host_info.rs](src/host_info.rs) | 主机与窗口管理器信息 |
| [src/flick_icon.rs](src/flick_icon.rs) | 内嵌矢量图标 |
| [tests/portal_state_machine.rs](tests/portal_state_machine.rs) | 15 个状态机测试 |
| [tests/session_diagnostics.rs](tests/session_diagnostics.rs) | 会话探测测试 |
| [examples/portal_probe.rs](examples/portal_probe.rs) | 只读诊断探针示例 |

## 测试与常用命令

```bash
cargo test -p flick-adapters-linux                                # 全部测试
cargo test -p flick-adapters-linux --test portal_state_machine     # 状态机测试（15 个）
cargo test -p flick-adapters-linux --test session_diagnostics      # 会话探测测试
cargo run -p flick-adapters-linux --example portal_probe           # 只读 Portal 诊断探针
cargo clippy -p flick-adapters-linux --all-targets -- -D warnings
```

`portal_probe` 只打开并列出 GlobalShortcuts 会话，不调用 `BindShortcuts`、不转发 UI 意图（[examples/portal_probe.rs](examples/portal_probe.rs)），适合在真实桌面上排查 Portal 授权问题。

## 与其他 crate 的关系

- **上游**：`flick-domain`（`APP_ID`、`HostInfo`、`UiIntent`、`SettingValue`）、`flick-application`（应用层抽象）。
- **下游**：`flick-daemon` 用 `PortalShortcutAdapter` 与 `register_portal_app_id_with_connection` 接入真实桌面；上层通过 `UiIntent` 接收搜索面板切换。
- 与 `flick-ipc` 无直接依赖：`ShortcutsConfigure` / `ShortcutsReconnect` 命令由 daemon 转成对本 crate 的 `configure_search_shortcut` / `reconnect` 调用。
