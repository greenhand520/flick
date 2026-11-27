# flick-builtins

随 Flick 一起发布的**内置插件集合**：提供 echo（回显）、host（系统信息）与 apps（应用搜索）三个开箱即用的动作，并在启动时通过插件注册表统一登记到动作运行时。

## 在 workspace 中的位置 / 依赖关系

本 crate 位于 `crates/flick-builtins`，构建在 `flick-runtime` 之上，把若干 `Plugin` 实现交给运行时登记与调用；它不定义领域模型，也不实现运行时调度。

- **直接依赖**：`flick-domain`（动作 / 插件领域类型）、`flick-runtime`（`ActionRuntime`、`Plugin`、`ActionHandler`）；外部库 `serde`、`serde_json`、`async-trait`、`ib_pinyin`、`tracing`。
- **开发依赖**：`tokio`、`tokio-util`、`tempfile`（仅测试使用）。
- **反向依赖**：`flick-daemon`、`flick-shell-gpui` 在启动时调用 [`register_builtin`](src/lib.rs#L23) 装配内置插件。

```
flick-daemon ──────┐
                   ├──► flick-builtins ──► flick-runtime ──► flick-domain
flick-shell-gpui ──┘          │
                              └──► ib_pinyin（应用名拼音匹配）
```

三个内置插件均声明为零插件依赖、不限平台（`host` 除外，仅声明支持 Linux），因此 [`register_builtin`](src/lib.rs#L23) 在注册完毕后统一调用 `resolve_dependencies()`：缺失的第三方依赖只会让对应插件不可用，不会中断启动。

## 为什么设计这个 crate

把「随发行版附带的动作」与「运行时调度」切成两个边界：

- `flick-runtime` 只认识 `Plugin` / `ActionHandler` trait 与注册表，不关心动作来自内置集合还是第三方；本 crate 是这些 trait 的**内置实现集合**，宿主只需一次 `register_builtin` 即可装配，无需知道 echo / host / apps 的内部结构。
- 内置动作与第三方插件走**完全相同的注册、依赖解析与生命周期规则**（动作 ID 冲突返回 `IdCollision`、不静默覆盖；缺依赖的插件被标记不可用），因此内置集合没有特权通道，也不会绕过运行时的安全约束。
- 把有磁盘 I/O 与进程启动副作用的 apps 扫描限制在一个插件内，其余两个是纯逻辑插件；隔离后运行时与 UI 层不必依赖 `.desktop` 解析或拼音匹配库。

## 如何运行

**启动装配**：宿主在构造 `ActionRuntime` 后调用一次 [`register_builtin(&runtime)`](src/lib.rs#L23)。它依次 `register_plugin` 三个插件（[lib.rs:24](src/lib.rs#L24)–[lib.rs:26](src/lib.rs#L26)），全部注册完成后才循环 [`resolve_dependencies()`](src/lib.rs#L28) 并把未解析的依赖记为 warning——放在最后是为了让结果与注册顺序无关；随后宿主调用 `ActionRuntime::activate_all_plugins()` 统一激活生命周期。重复调用 `register_builtin` 会因动作 ID 冲突而失败。

**运行期两条路径**：

1. 普通调用：echo / host 由运行时按动作 ID 路由到 `EchoHandler::invoke` / `HostHandler::invoke`（[echo.rs:23](src/echo.rs#L23)、[host.rs:80](src/host.rs#L80)）。host 要求 `InputType::None` 且上下文带 `HostInfo`，返回 `ActionOutcome::open_surface("info", content)`。
2. 搜索阶段调用：apps 的 search 动作声明 `search_invocable = true`，被运行时的 `search_invoke` 并行、只读地调用（[apps.rs:276](src/apps.rs#L276)），结果归一化为执行项；只有用户激活执行项时才走 [`execution_event`](src/apps.rs#L314)，`spawn` 对应进程——这是三个插件里唯一的进程副作用。

**线程 / 阻塞模型**：`register_builtin` 与生命周期 `activate` 是 async；apps 的 `.desktop` 索引在 `AppsLifecycle::activate`（[apps.rs:247](src/apps.rs#L247)）中**同步建一次**（不持有 Tokio 锁、不引入 tokio 依赖），此后只读共享的 `Arc<RwLock<Vec<DesktopApp>>>`；进程启动用 `std::process::Command::spawn`，父进程不等待子进程。

## 内部技术与架构细节

- **echo（Phase 0 参考实现）**：[`ECHO_ACTION_ID`](src/echo.rs#L12) = `flick.builtin.echo.v1.echo`；只接受 `InputType::PlainText`，否则 `InvalidInput`；返回 `ActionOutcome::Data({"echo": <text>})`，不碰文件 / 网络 / UI，也不按 `CallerKind` 改输出。描述符 [`echo_descriptor()`](src/echo.rs#L41) 的关键词为 `echo` / `回显` / `repeater`，并发策略 `Parallel`；manifest 的 `supported_os` / `supported_architectures` 为空，表示平台无关。
- **host（数据驱动 Surface）**：[`HOST_ACTION_ID`](src/host.rs#L9) = `flick.builtin.host.host_info`；描述符声明 `supported_inputs = [None]`、输出 `Surface`。manifest 声明 `info` Surface（`SurfaceKind::Panel`、位于 `BelowSearchBox`）且仅支持 Linux。 [`collect_extras()`](src/host.rs#L125) 做一次廉价的同步读取：`/proc/uptime`、`/proc/cpuinfo`、`/proc/meminfo` 以及 `XDG_CURRENT_DESKTOP` / `XDG_SESSION_TYPE` / `GTK_THEME` / `TERM_PROGRAM` / `TERM`；[`build_surface()`](src/host.rs#L207) 按 `SURFACE_SCHEMA_VERSION` 拼装 `SurfaceNode::KeyValue` / `Divider`。[`HostLifecycle`](src/host.rs#L105) 的空闲超时为 60 秒。GPU / 磁盘 / 包 / 电池 / 图标 / 字体 / 光标等字段仍是 TODO，未实现。
- **apps（唯一带 I/O 的插件）**：[`DesktopApp`](src/apps.rs#L14) = `{ id: "app:<stem>", name, exec, icon, keywords }`。[`desktop_dirs_from_env()`](src/apps.rs#L31) 优先取 `FLICK_APPS_DIRS`（冒号分隔），否则用 `XDG_DATA_HOME` / `$HOME/.local/share/applications` 与 `XDG_DATA_DIRS`；[`scan_apps_from_dirs()`](src/apps.rs#L60) 递归扫描 `.desktop`，按目录顺序**先到先得去重**（同 `id` 保留第一个），再按小写名称稳定排序。[`parse_desktop()`](src/apps.rs#L92) 只解析 `[Desktop Entry]` 组（忽略 `[Desktop Action …]`），跳过非 `Application`、`NoDisplay=true`、`Hidden=true`，并用 [`strip_field_codes()`](src/apps.rs#L155) 去掉 `%F` 等字段码。[`APP_SEARCH_ACTION_ID`](src/apps.rs#L163) = `flick.builtin.apps.search`，[`MAX_RESULTS = 20`](src/apps.rs#L166)；匹配为字面（大小写不敏感）或拼音（`ib_pinyin` 的 `PinyinMatcher`，`Ascii | AsciiFirstLetter`），返回 `{id,title,subtitle,icon}` 数组并截断到 20 条；src/apps.rs:348 起有 5 个内联单元测试。

## 公开 API / 核心类型

| 项目 | 位置 | 说明 |
| --- | --- | --- |
| `register_builtin()` | [lib.rs:23](src/lib.rs#L23) | 一次性注册 echo + host + apps，再解析依赖；每个运行时只应调用一次（重复动作 ID 返回 `IdCollision`，不静默覆盖） |
| `ECHO_ACTION_ID` | [echo.rs:12](src/echo.rs#L12) | echo 动作稳定 ID，`flick.builtin.echo.v1.echo` |
| `EchoHandler` | [echo.rs:19](src/echo.rs#L19) | 仅接受纯文本，返回 `ActionOutcome::Data({"echo": ...})` |
| `echo_descriptor()` | [echo.rs:41](src/echo.rs#L41) | 构造 echo 的动作描述符 |
| `EchoPlugin` | [echo.rs:51](src/echo.rs#L51) | 把 echo 暴露为 `Plugin` |
| `HOST_ACTION_ID` | [host.rs:9](src/host.rs#L9) | host 动作稳定 ID，`flick.builtin.host.host_info` |
| `HostPlugin` | [host.rs:11](src/host.rs#L11) | 打开 `info` Surface，展示系统信息 |
| `DesktopApp` | [apps.rs:14](src/apps.rs#L14) | 扫描 `.desktop` 得到的可启动应用条目 |
| `APP_SEARCH_ACTION_ID` | [apps.rs:163](src/apps.rs#L163) | 应用搜索动作稳定 ID，`flick.builtin.apps.search` |
| `AppsPlugin` | [apps.rs:169](src/apps.rs#L169) | 应用搜索插件；`with_dirs()` 可注入扫描目录 |
| `AppsPlugin::with_dirs()` | [apps.rs:184](src/apps.rs#L184) | 以显式目录构造插件（测试注入临时目录） |

`lib.rs` 还重导出了 `EchoPlugin` / `EchoHandler` / `ECHO_ACTION_ID` 与 `HostPlugin` / `HOST_ACTION_ID`，因此上层可写 `flick_builtins::EchoPlugin`。插件契约（`Plugin` / `ActionHandler` / `PluginLifecycle`）现由 `flick-plugin` 定义，`flick-runtime` 重导出，本 crate 直接依赖 `flick-runtime` 即可拿到这些 trait。

## 模块 / 文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 根；`register_builtin()` 与对外重导出 |
| [src/echo.rs](src/echo.rs) | echo 动作的处理器、描述符与插件 |
| [src/apps.rs](src/apps.rs) | `.desktop` 扫描、拼音搜索、进程启动；含 5 个内联单元测试 |
| [src/host.rs](src/host.rs) | host 信息采集、Surface 构建、生命周期（60 秒空闲超时） |
| [tests/echo_action.rs](tests/echo_action.rs) | echo 端到端路由与调用测试（7 例） |
| [tests/host_plugin.rs](tests/host_plugin.rs) | host manifest 合法性与 OpenSurface 输出测试（3 例） |
| [tests/apps_plugin.rs](tests/apps_plugin.rs) | 应用搜索插件注册与激活测试（2 例） |

## 测试与常用命令

```bash
cargo test -p flick-builtins                 # 全部单元测试 + 集成测试
cargo test -p flick-builtins --test echo_action
cargo test -p flick-builtins --test host_plugin
cargo test -p flick-builtins --test apps_plugin
cargo check -p flick-builtins
cargo doc -p flick-builtins --open
```

应用搜索测试通过 `FLICK_APPS_DIRS` / `AppsPlugin::with_dirs()` 注入临时目录，不依赖真实桌面环境；host 测试注入 `HostInfo` 桩数据。

## 与其他 crate 的关系

- `flick-domain`：提供 `ActionDescriptor`、`ActionRequest`、`ActionOutcome`、`PluginManifest`、`HostInfo`、`SurfaceNode` 等类型，本 crate 只消费不改。
- `flick-runtime`：提供 `ActionRuntime`（注册表 / 路由 / 调用）与 `Plugin`、`ActionHandler`、`PluginLifecycle` trait（后者由 `flick-plugin` 定义并经运行时重导出）；`register_builtin` 的返回错误 `PluginRegistrationError` 也来自这里。
- `flick-daemon` / `flick-shell-gpui`：上层宿主，在启动时调用本 crate 的 `register_builtin`，随后 `activate_all_plugins()`。
- 第三方插件：与内置插件共用同一个注册表；动作 ID 冲突时运行时返回 `IdCollision`，不做静默覆盖。
