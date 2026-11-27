# flick-cli

Flick 的命令行客户端 crate：同时提供可复用的 `flick_cli` 库与 `flick` 可执行文件，把命令经 Unix socket IPC 发给守护进程并格式化输出。

## 在 workspace 中的位置与依赖

`flick` 是面向用户/脚本的入口；`flick_cli` 库封装了 socket 路径解析、请求构造、连接与结果格式化，便于测试与复用。

| 依赖 | 用途 |
| --- | --- |
| `flick-ipc`（path） | `IpcCommand` / `IpcRequestEnvelope` / `UnixSocketClient` / 错误码 |
| `flick-domain`（path） | 领域类型（如 `SettingValue`） |
| `clap` 4（derive） | 命令行参数与子命令解析 |
| `serde` / `serde_json` / `tokio` / `async-trait` / `thiserror` | 序列化、异步、错误 |

依赖与 targets 见 [Cargo.toml](Cargo.toml)。

## 为什么设计这个 crate

从依赖关系看，`flick-cli` 与 `flick-daemon` 是两个独立可执行文件，二者只通过 `flick-ipc` 的协议通信，没有代码依赖。把客户端拆成「可复用库 + 薄可执行文件」两层：`flick_cli` 库把 socket 路径解析、请求构造、连接与结果/错误格式化集中起来，`flick` bin 只负责 clap 解析和一次调用链（外加 `toggle` 的自动拉起）。这样协议处理可被单测和真实 IPC 往返测试覆盖，而不必启动真实守护进程；同时命令行入口保持很薄，便于脚本与桌面自定义快捷键调用（`docs/flick-service.md` 描述的 `flick toggle` 正是发给唯一 daemon 的 IPC 回退方案，不是第二个启动器）。

## 如何运行

入口是 `#[tokio::main] fn main()`（[main.rs:106](src/main.rs#L106)），流程为：解析 CLI → 解析 socket 路径 → 把 `Command` 映射成 `IpcCommand` → `build_request` → `try_connect_and_send` → `format_result` 并以退出码结束。

- **路径解析**：`--socket <PATH>` 优先，否则 `default_socket_path()`（[lib.rs:31](src/lib.rs#L31)）取 `$XDG_RUNTIME_DIR/flick/flick.sock`；`XDG_RUNTIME_DIR` 缺失时打印错误并以退出码 2 结束（[main.rs:108](src/main.rs#L108)）。
- **命令映射**（[main.rs:120](src/main.rs#L120)）：`Command`（[main.rs:27](src/main.rs#L27)）逐个转成 `IpcCommand`；`settings` 的 5 个子命令经 `parse_setting_value`（[lib.rs:135](src/lib.rs#L135)）把字符串值解析为 `SettingValue`，`shortcuts` 的 2 个子命令分别转为 `ShortcutsConfigure` / `ShortcutsReconnect`。
- **连接与发送**：`try_connect_and_send`（[lib.rs:91](src/lib.rs#L91)）连上 socket 并写一帧、读一帧，返回 `ConnectResult::{Response, ConnectError, TransportError}`（[lib.rs:81](src/lib.rs#L81)）。
- **toggle 自动拉起**（[main.rs:179](src/main.rs#L179)）：仅当首次尝试得到连接错误（退出码 2）且命令是 `IpcCommand::Toggle` 时，执行 `systemctl --user start flick.service`（[main.rs:185](src/main.rs#L185)）、等待 500ms（[main.rs:190](src/main.rs#L190)）后重试一次（[main.rs:193](src/main.rs#L193)），并以重试的退出码结束。其它命令的连接错误直接打印（[main.rs:226](src/main.rs#L226)）。

`--json` 是全局选项，须放在子命令之前（如 `flick --json doctor`，见 `docs/flick-service.md`）；开启后错误写 stdout 的结构化 JSON，否则写 stderr。

## socket 路径与退出码

| 项 | 值 / 说明 | 出处 |
| --- | --- | --- |
| 默认 socket | `$XDG_RUNTIME_DIR/flick/flick.sock` | [lib.rs:27](src/lib.rs#L27)（与 `flick-daemon/src/lifecycle.rs:64` 一致） |
| 覆盖方式 | `--socket <PATH>` | [main.rs:15](src/main.rs#L15) |
| 请求 ID | `cli-<递增序号>`（`AtomicU64` 从 1 开始） | [lib.rs:41](src/lib.rs#L41) |

`format_result` 的错误退出码映射（[lib.rs:49](src/lib.rs#L49)，[lib.rs:68](src/lib.rs#L68)）：`DaemonUnavailable` → 2，`ProtocolVersionMismatch` → 3，`DesktopUnavailable` → 1，其余（`UnknownCommand` / `ActionNotFound` / `InvalidInput` / `Internal` / `Cancelled` / `SettingsUnavailable`）→ 1；成功 → 0。`--json` 输出时错误体为 `{"error":{"code":<as_str>,"message":...}}`，成功体为 `{"ok":<value>}`；人类可读模式下错误写 stderr。连接失败（非 toggle 重试路径）的 JSON 码为 `DAEMON_UNAVAILABLE`（[lib.rs:102](src/lib.rs#L102)），传输错误为 `INTERNAL`（[lib.rs:117](src/lib.rs#L117)）。

## 命令与公开 API

### 子命令（[main.rs:27](src/main.rs#L27)）

| 命令 | 说明 |
| --- | --- |
| `toggle` | 切换搜索面板（桌面快捷键绑定的 IPC 回退，失败时自动拉起服务并重试一次） |
| `trigger <ACTION_ID> --text <TEXT>` | 触发动作；`--text` 为必填（`#[arg(long)]`，见 [main.rs:35](src/main.rs#L35)） |
| `status` | 查询守护进程状态 |
| `doctor` | 诊断信息 |
| `shortcuts configure [--show-search <TRIGGER>]` | 配置搜索快捷键，默认 `Super+Space`（[main.rs:98](src/main.rs#L98)） |
| `shortcuts reconnect` | 重新连接 Portal 会话（仅恢复，无绑定界面） |
| `settings list <NAMESPACE>` | 列出命名空间下的设置值 |
| `settings get <KEY>` | 读取单个设置项 |
| `settings set <KEY> <VALUE>` | 写入设置项（`VALUE` 为 JSON 字面量或字符串，见 [main.rs:73](src/main.rs#L73)） |
| `settings reset-key <KEY>` | 重置单个设置项 |
| `settings reset-namespace <NAMESPACE>` | 重置整个命名空间（保留声明） |
| `shutdown` | 关闭守护进程 |
| `show` / `hide` | 显示 / 隐藏窗口 |

全局选项：`--socket <PATH>`（[main.rs:15](src/main.rs#L15)）、`--json`（[main.rs:20](src/main.rs#L20)）。`SettingsAction` 定义见 [main.rs:61](src/main.rs#L61)，`ShortcutsAction` 见 [main.rs:93](src/main.rs#L93)。

### 库 API（[lib.rs](src/lib.rs)）

- `resolve_default_socket_path<F>(get_env)`（[lib.rs:20](src/lib.rs#L20)）/ `default_socket_path()`（[lib.rs:31](src/lib.rs#L31)）：解析 socket 路径，`XDG_RUNTIME_DIR` 缺失返回 `SocketPathError::MissingRuntimeDir`（[lib.rs:12](src/lib.rs#L12)）。
- `build_request(command: IpcCommand) -> IpcRequestEnvelope`（[lib.rs:36](src/lib.rs#L36)）：注入当前协议版本与递增 `request_id`。
- `format_result(&IpcResult, json_output) -> (String, bool, i32)`（[lib.rs:49](src/lib.rs#L49)）：返回（输出文本，是否写 stderr，退出码）。
- `parse_setting_value(&str) -> SettingValue`（[lib.rs:135](src/lib.rs#L135)）：把 CLI 字符串解析为带标签 JSON / 布尔 / 整数 / 字符串。
- `ConnectResult`（[lib.rs:81](src/lib.rs#L81)）：`Response` / `ConnectError` / `TransportError`。
- `try_connect_and_send(path, request)`（[lib.rs:91](src/lib.rs#L91)）、`format_connection_error` / `format_transport_error`（[lib.rs:102](src/lib.rs#L102)、[lib.rs:117](src/lib.rs#L117)）。

## 模块与文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | 可复用库：socket 解析、请求构造、连接、结果/错误格式化 |
| [src/main.rs](src/main.rs) | `flick` 可执行文件：clap 命令定义与 `toggle` 自动拉起重试 |
| [tests/cli_contract.rs](tests/cli_contract.rs) | 解析/格式化单测 + 真实 IPC 往返集成测试 |

## 测试与常用命令

```bash
cargo test -p flick-cli                 # 单测 + 集成测试
cargo run -p flick-cli -- status        # 查询守护进程（需先启动 flick-daemon）
cargo run -p flick-cli -- --json doctor
cargo clippy -p flick-cli --all-targets -- -D warnings
```

集成测试用 `tempfile` 起一个真实 `flick-ipc` server，验证命令映射、`request_id` 透传与结构化错误，不依赖真实守护进程。

## 与其他 crate 的关系

- **直接依赖**：`flick-ipc`（协议与传输）、`flick-domain`（领域类型）。
- **通信对象**：`flick-daemon`——CLI 只是客户端，命令由 daemon 的 `IpcDispatcher` 处理；`shortcuts` 子命令最终转给 `flick-adapters-linux` 的 Portal 适配器。
- 与 `flick-adapters-linux` 无直接代码依赖，二者通过 `flick-ipc` 的命令协议解耦。
