# flick-ipc

Flick 的进程间通信（IPC）基础 crate：定义守护进程与客户端之间的消息模型、线编码、Unix socket 传输与服务端分发器。daemon 侧实现 `IpcDispatcher` 并跑 `IpcServer`，CLI 侧用 `UnixSocketClient` 发命令；双方都只依赖本 crate 与领域模型 `flick-domain`。

## 在 workspace 中的位置与依赖

`flick-ipc` 是 workspace 中偏底层的库 crate，被 `flick-daemon`（服务端）与 `flick-cli`（客户端）直接依赖；自身只依赖领域模型 `flick-domain` 与序列化/异步基础设施。

| 依赖 | 用途 |
| --- | --- |
| `flick-domain`（path） | 领域类型（如 `HostInfo`、`SettingValue`）作为消息负载 |
| `serde` / `serde_json` | 消息派生与 `Ok` 变体的 `Value` 负载 |
| `rmp-serde` | MessagePack 编解码 |
| `tokio` / `tokio-util` | 异步 I/O 与 `CancellationToken` |
| `thiserror` / `tracing` / `async-trait` | 错误类型、日志、异步 trait |

依赖声明见 [Cargo.toml](Cargo.toml)。

## 为什么设计这个 crate

从依赖关系看，`flick-daemon` 与 `flick-cli` 是两个独立的可执行文件，但它们必须就「命令有哪些、错误码是什么、一帧怎么编码」达成字节级一致。把消息模型、编解码与传输抽成独立 crate，让服务端和客户端共享同一份协议实现，避免两处各写一套而有 drift 风险；同时把这层与业务逻辑（daemon 的 `IpcDispatcher` 实现）和平台细节（Portal/socket 路径由上层决定）隔开。设计文档 `docs/flick-service.md` 描述的「单个 systemd 用户服务 + CLI 经 IPC 与其通信」正是这条边界：本 crate 只负责「消息怎么传」，不负责「命令做什么」——后者由 daemon 实现 `IpcDispatcher` 提供，socket 路径也由 daemon 在 `flick-daemon/src/lifecycle.rs:64` 决定（本 crate 只接受调用方传入的路径）。

## 如何运行

本 crate 是库，没有自己的入口；运行形态由上下游两个可执行文件决定：

- **服务端**（`flick-daemon`）：在 `flick-daemon/src/bootstrap.rs:564` 调 `UnixSocketServer::bind(path)` 绑定 socket，随后 `IpcServer::new(server, dispatcher)`（`bootstrap.rs:567`）并 `run(cancel)`（`bootstrap.rs`）。`run` 在一个 biased `tokio::select!` 里同时监听新连接与取消信号：每个连接派生一个任务放入 `JoinSet`，收到取消后先 cancel 子 `CancellationToken`，再 `await` 所有在途客户端任务结束，实现优雅关闭（[server.rs:49](src/server.rs#L49)）。
- **客户端**（`flick-cli`）：`connect(path)`（[unix.rs:120](src/unix.rs#L120)）在 2 秒内连上 socket（`IPC_CONNECT_TIMEOUT`），`request()`（[unix.rs:138](src/unix.rs#L138)）把「写一帧 + 读一帧」包在 5 秒 `IPC_REQUEST_TIMEOUT` 内。
- **单连接生命周期**：`handle_client`（[server.rs:99](src/server.rs#L99)）读一条请求 → 校验版本并 `dispatch` → 写回一条响应 → `after_response`（[server.rs:28](src/server.rs#L28)，用于 `Shutdown`）→ 关闭连接；一次连接只处理一条请求。`dispatch` 本身也包在 5 秒超时内，慢客户端不会拖垮健康客户端。
- **帧格式**：4 字节大端 `u32` 长度 + MessagePack 负载，上限 1 MiB。见下一节。

> 注意：模块文档 [lib.rs:8](src/lib.rs#L8) 仍写着「长度前缀 JSON 帧 / 64 KiB」，是历史描述；实际实现为 MessagePack、上限 1 MiB，并有测试把 `MAX_FRAME_SIZE` 锁死为 1 MiB（`tests/codec_msgpack.rs` 的 `max_frame_size_is_one_mib`）。

## 协议常量与线格式

| 常量 | 值 | 出处 |
| --- | --- | --- |
| `IPC_PROTOCOL_VERSION` | `4`（`u16`） | [message.rs:9](src/message.rs#L9) |
| `MAX_FRAME_SIZE` | `1024 * 1024`（1 MiB，`u32`） | [codec.rs:18](src/codec.rs#L18) |
| `IPC_CONNECT_TIMEOUT` | 2 秒 | [unix.rs:15](src/unix.rs#L15) |
| `IPC_REQUEST_TIMEOUT` | 5 秒 | [unix.rs:18](src/unix.rs#L18) |
| 默认 socket 路径 | `$XDG_RUNTIME_DIR/flick/flick.sock` | `flick-daemon/src/lifecycle.rs:64`（本 crate 只接受调用方传入的路径） |

- **编码**：`encode`（[codec.rs:39](src/codec.rs#L39)）用 `rmp_serde::to_vec_named`（[codec.rs:40](src/codec.rs#L40)）按字段名序列化；`write_frame`（[codec.rs:78](src/codec.rs#L78)）先写 4 字节大端长度再写负载。
- **解码**：`read_frame`（[codec.rs:86](src/codec.rs#L86)）读长度字段，超过 `MAX_FRAME_SIZE` 立即返回 `CodecError::FrameTooLarge`；`decode`（[codec.rs:61](src/codec.rs#L61)）自身也做一次上限自检，防止直接调用时被超大负载击穿。
- **bind 的权限与单实例探测**（[unix.rs:58](src/unix.rs#L58)）：先尝试连接现有 socket，连得上就返回 `TransportError::AlreadyRunning`；连不上则删除陈旧 socket，递归创建权限 `0o700` 的父目录，绑定后把 socket 文件权限设为 `0o600`。
- **版本握手**：`process_request`（[server.rs:141](src/server.rs#L141)）先比对信封里的 `protocol_version`，不符则回 `IpcErrorCode::ProtocolVersionMismatch`（[server.rs:149](src/server.rs#L149)），不进入 `dispatch`。

## 公开 API 与核心类型

所有公开项在 [lib.rs:25](src/lib.rs#L25) 统一重导出。

### 消息模型（[message.rs](src/message.rs)）

- `IpcCommand`（[message.rs:35](src/message.rs#L35)）：请求命令枚举，`serde(tag="type", content="payload", rename_all="snake_case")`。13 个变体：`Toggle`、`Trigger { action_id, text }`、`Status`、`Doctor`、`ShortcutsConfigure { preferred_trigger }`、`ShortcutsReconnect`、`Shutdown`、`Show`、`Hide`、`SettingsList { namespace }`、`SettingsGet { key }`、`SettingsSet { key, value: SettingValue }`、`SettingsReset { key, namespace }`（后两者分别为 `Option<String>`，必须且只能给一个）。
- `IpcRequestEnvelope`（[message.rs:15](src/message.rs#L15)）：`{ protocol_version, request_id, command }`。
- `IpcResponseEnvelope`（[message.rs:23](src/message.rs#L23)）：`{ protocol_version, request_id, result }`。
- `IpcResult`（[message.rs:75](src/message.rs#L75)）：`Ok(serde_json::Value)` 或 `Error { code, message }`，`serde(tag="result", rename_all="snake_case")`；构造器 `ok()`（[message.rs:82](src/message.rs#L82)）/ `error()`（[message.rs:87](src/message.rs#L87)）。
- `IpcErrorCode`（[message.rs:100](src/message.rs#L100)）：9 个变体——`DaemonUnavailable`、`ProtocolVersionMismatch`、`UnknownCommand`、`ActionNotFound`、`InvalidInput`、`DesktopUnavailable`、`Internal`、`Cancelled`、`SettingsUnavailable`；`as_str()`（[message.rs:123](src/message.rs#L123)）给出 snake_case 字符串。

### 编解码（[codec.rs](src/codec.rs)）

- `encode<T: Serialize>(&T) -> Result<Vec<u8>, CodecError>`（[codec.rs:39](src/codec.rs#L39)）、`decode<T>(&[u8]) -> Result<T, CodecError>`（[codec.rs:61](src/codec.rs#L61)）。
- `write_frame` / `read_frame`（[codec.rs:78](src/codec.rs#L78)、[codec.rs:86](src/codec.rs#L86)）：读写带长度前缀的帧。
- `CodecError`（[codec.rs:22](src/codec.rs#L22)）：`FrameTooLarge { size }` / `Io` / `Encode` / `Decode`。

### 传输（[unix.rs](src/unix.rs)）

- `UnixSocketServer`（[unix.rs:46](src/unix.rs#L46)）与 `bind(path)`（[unix.rs:58](src/unix.rs#L58)）、`accept()`（[unix.rs:106](src/unix.rs#L106)）。
- `UnixSocketClient`（[unix.rs:115](src/unix.rs#L115)）与 `connect(path)`（[unix.rs:120](src/unix.rs#L120)）、`request(&IpcRequestEnvelope)`（[unix.rs:138](src/unix.rs#L138)）、`read_request` / `write_response`（[unix.rs:185](src/unix.rs#L185)、[unix.rs:192](src/unix.rs#L192)）。
- `TransportError`（[unix.rs:24](src/unix.rs#L24)）：`Io` / `Codec` / `AlreadyRunning` / `TimedOut { operation }`。

### 服务端分发（[server.rs](src/server.rs)）

- `IpcDispatcher` trait（[server.rs:20](src/server.rs#L20)）：`dispatch`（[server.rs:22](src/server.rs#L22)）处理命令，`after_response`（[server.rs:28](src/server.rs#L28)）在响应写出后执行副作用（用于 `Shutdown`）。
- `IpcServer`（[server.rs:33](src/server.rs#L33)）：`new()`（[server.rs:40](src/server.rs#L40)）、`run(self, cancel: CancellationToken)`（[server.rs:49](src/server.rs#L49)）。`run` 用 biased select + `JoinSet`，取消时等待在途客户端任务结束。

## 模块与文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根：模块声明与公开重导出 |
| [src/message.rs](src/message.rs) | 协议版本、命令/信封/结果/错误码 |
| [src/codec.rs](src/codec.rs) | 4 字节长度前缀 + MessagePack 编解码 |
| [src/unix.rs](src/unix.rs) | Unix socket 绑定/连接、超时、单实例探测、请求应答 |
| [src/server.rs](src/server.rs) | `IpcDispatcher` 分发与 `IpcServer` 生命周期 |
| [tests/codec_msgpack.rs](tests/codec_msgpack.rs) | 编码、帧大小边界、负载往返测试 |
| [tests/unix_transport.rs](tests/unix_transport.rs) | 传输往返、陈旧 socket、版本拒绝、超时与取消测试 |

## 测试与常用命令

```bash
cargo test -p flick-ipc                        # 全部单元 + 集成测试
cargo test -p flick-ipc --test codec_msgpack   # 仅编解码测试
cargo test -p flick-ipc --test unix_transport  # 仅传输/服务端测试
cargo clippy -p flick-ipc --all-targets -- -D warnings
```

集成测试用 `tempfile` 创建临时 socket 路径，不需要真实守护进程。

## 与其他 crate 的关系

- **上游**：`flick-domain` 提供负载中的领域类型（如 `SurfaceNode`、`SettingValue`）。
- **下游**：`flick-daemon` 实现 `IpcDispatcher` 并在 `$XDG_RUNTIME_DIR/flick/flick.sock` 上跑 `IpcServer`；`flick-cli` 通过 `UnixSocketClient` 发命令。
- 协议版本 `4` 与错误码是跨 crate 契约：客户端版本不符会被服务端以 `ProtocolVersionMismatch` 拒绝（[server.rs:149](src/server.rs#L149)）。
