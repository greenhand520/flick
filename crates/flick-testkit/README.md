# flick-testkit

Flick 各 crate 的**测试工具与伪实现**集合：为 `flick-application` 的端口 trait 与 `flick-domain` 的领域类型提供可复用的测试替身（伪实现、桩、间谍）。

## 在 workspace 中的位置 / 依赖关系

本 crate 位于 `crates/flick-testkit`，是一个轻量的测试支撑库，不被任何生产 crate 直接依赖；需要集成测试支持的 crate 把它作为 `[dev-dependency]` 导入即可。

- **直接依赖**：`flick-domain`、`flick-application`（`HostEventSink` 端口）、`flick-runtime`、`tokio`、`async-trait`。
- **反向依赖**：目前 workspace 内没有 crate 声明它；按需在目标 crate 的 `[dev-dependencies]` 中加入 `flick-testkit = { path = "../flick-testkit" }`。

```
被测 crate 的测试 ──► flick-testkit ──► flick-application::HostEventSink（端口）
                               │
                               ├──────► flick-domain::HostToUi（事件类型）
                               └──────► flick-runtime（便于同时构造 ActionRuntime）
```

## 为什么设计这个 crate

项目的依赖方向固定为「shell / cli / daemon / adapters / builtins / ipc → application + runtime → domain」，并明确本 crate 仅提供 fake/fixture/断言、可以依赖领域协议，而**生产 crate 不得依赖 flick-testkit**。workspace 责任边界也把「Fake port、fixture、契约测试与测试断言」列为本 crate 的责任，并把「不承担真实桌面会话依赖」列为明确边界。

单独拆出的理由是：核心 crate 的测试必须在**没有 Portal、D-Bus、Wayland 会话或 GUI 窗口**的环境下运行，而验证 `ApplicationCoordinator` 最直接的办法是观察它发出的 `HostToUi` 序列。把这些伪实现集中起来，测试代码就不必各自造一个假的 UI 接收端，也不会让生产二进制链接测试专用代码——依赖方向始终是「测试 → testkit → 端口」，反向不连通。

## 如何运行

本 crate 是库，没有 `main`、没有进程、没有事件循环，也不创建线程。它的「运行」发生在**使用方的测试函数里**：测试先构造 `FakeHostEventSink::new()`（[fake_host.rs:29](src/fake_host.rs#L29)，或 `Default`），把它作为 `Arc<dyn HostEventSink>` 与一个伪造的 `RuntimePort` 一起传给 `ApplicationCoordinator::new`；随后在测试中调用协调器的 `handle_ui` / `handle_intent`，每发出一个 `HostToUi`，`send()`（[fake_host.rs:57](src/fake_host.rs#L57)）就把事件压入内部缓冲区。

断言有三种方式：`events()`（[fake_host.rs:36](src/fake_host.rs#L36)）取全部事件的克隆且不排空，`drain()`（[L41](src/fake_host.rs#L41)）排空并返回，`last()`（[L46](src/fake_host.rs#L46)）窥视最近一条。sink 本身是同步的、不需要 tokio；若被测协调器的 `handle_ui` 是异步的，测试用 `#[tokio::test]` 驱动即可。[fake_host.rs:13-22](src/fake_host.rs#L13) 的 crate 文档示例（`#![ignore]`，不会作为 doctest 执行）演示了完整构造与断言流程。

## 内部技术与架构细节

**共享缓冲区。** `FakeHostEventSink`（[fake_host.rs:23](src/fake_host.rs#L23)）只有一个字段 `events: Arc<Mutex<Vec<HostToUi>>>`（[L23-L25](src/fake_host.rs#L23)），事件类型来自 `flick_domain::HostToUi`。`Arc` 让同一个接收端可以被测试与被测代码同时持有，`Mutex` 保证并发 `send` 下的串行化；`new()` 创建空缓冲区，`Default`（[L51](src/fake_host.rs#L51)）等价。

**记录语义。** `impl HostEventSink::send`（[fake_host.rs:57](src/fake_host.rs#L57)）只做 `push`，不做过滤、排序或去重，因此事件顺序就是协调器实际发出的顺序，适合断言「首屏 Routes 先于补发的 ExecutionResults」这类时序。`events()` 返回 `lock().clone()`（不消费），`drain()` 调用 `Vec::drain(..)`（消费），`last()` 返回 `last().cloned()`（不消费）——三种读取方式互不影响，但 `drain()` 会清空后续断言可见的历史。

**依赖方向。** 本 crate 重新导出 `FakeHostEventSink`（[lib.rs:10](src/lib.rs#L10)），测试直接写 `flick_testkit::FakeHostEventSink`。它依赖 `HostEventSink` 端口与 `HostToUi` 类型，但不实现 `RuntimePort`、不启动 Runtime；`flick-runtime` 只是传递依赖，便于测试同时构造 `ActionRuntime`。目前没有其它测试替身（如 fake portal / fixture）落地。

## 公开 API / 核心类型

| 项目 | 位置 | 说明 |
| --- | --- | --- |
| `FakeHostEventSink` | [fake_host.rs:23](src/fake_host.rs#L23) | 伪 `HostEventSink`，记录协调器发出的每一个 `HostToUi` 事件 |
| `FakeHostEventSink::new()` | [fake_host.rs:29](src/fake_host.rs#L29) | 创建一个新的空接收端 |
| `events()` | [fake_host.rs:36](src/fake_host.rs#L36) | 返回所有已收集事件的克隆，**不**排空 |
| `drain()` | [fake_host.rs:41](src/fake_host.rs#L41) | 排空并返回全部已收集事件 |
| `last()` | [fake_host.rs:46](src/fake_host.rs#L46) | 窥视最近发出的事件，**不**移除 |
| `impl HostEventSink` | [fake_host.rs:57](src/fake_host.rs#L57) | `send()` 把事件压入内部缓冲区；另有 `Default` 实现 |

`lib.rs` 重导出了 `FakeHostEventSink`，因此可直接写 `flick_testkit::FakeHostEventSink`，无需关心内部模块路径。

## 模块 / 文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根；声明 `fake_host` 模块并重导出 `FakeHostEventSink` |
| [src/fake_host.rs](src/fake_host.rs) | 唯一的测试替身实现，基于 `Arc<Mutex<Vec<HostToUi>>>` 收集事件 |

本 crate 没有 `tests/` 目录：它自身就是测试工具，正确性由使用它的 crate 的测试间接覆盖。

## 测试与常用命令

```bash
cargo check -p flick-testkit
cargo doc -p flick-testkit --open
# 在目标 crate 中加入 dev-dependency 后：
cargo test -p <目标 crate>
```

本 crate 没有自带测试；作为测试工具被导入后，随目标 crate 的 `cargo test` 一起编译运行。

## 与其他 crate 的关系

- `flick-domain`：`HostToUi` 事件类型的来源。
- `flick-application`：`HostEventSink` 端口 trait 的定义处；`FakeHostEventSink` 实现该 trait，替代真实 UI 接收端。
- `flick-runtime`：传递依赖，便于测试同时构造 `ActionRuntime`。
- 使用方：任何需要验证 `ApplicationCoordinator` 事件输出的 crate（如 `flick-daemon`、`flick-shell-gpui` 的测试）可把它加为开发依赖。
