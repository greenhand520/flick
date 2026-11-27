# flick-shell-gpui

Flick 的 GPUI 桌面外壳：用全屏透明 overlay 绘制一个可拖动的搜索面板，处理输入捕获（粘贴 / 拖放 / 快捷键）、防抖、主题切换、Surface 交互，以及 Ctrl+D 分离出的独立窗口。

它既是一个库（`flick_shell_gpui`，供 [flick-daemon](../flick-daemon/README.md) 在主线程调用），也带一个独立演示二进制 `flick-gpui-demo`——后者用进程内自建的假宿主运行时跑起来，不依赖 daemon，方便单独调 UI。

## 为什么设计这个 crate

- **所有 GPUI 代码集中在一处**：`flick-ui` 的 `gpui` feature 默认关闭，本 crate 是唯一启用它的消费者，也是唯一把 `SurfaceNode` 树变成 GPUI 元素的地方。这样 `flick-builtins` 等无需渲染的 crate 不会把 gpui 拉进依赖图。
- **UI 与宿主进程解耦**：库只面对 `HostToUi` / `UiToHost` 两条无界通道与 `UiEvent` / `Effect` 状态机，不访问 `ActionRuntime`、Portal 或设置存储。daemon 持有宿主能力，本 crate 只做呈现与意图上报，二者可独立演化。
- **面板与独立窗口共用一套状态机**：两者都由同一个应用级事件泵管理窗口生命周期（`Show` → 建窗，`Hide` / Esc / 外部点击 → 销毁窗口），避免每个窗口各写一份键盘捕获和关闭逻辑。
- **可独立调试**：`flick-gpui-demo` 用 `fake_host` + `DemoClipboardPlugin` 在进程内提供最小宿主，UI 开发不必先启动 daemon 或 Portal。

## 在 workspace 中的位置

```
flick-daemon ──run_with_theme()──► flick-shell-gpui ──► flick-ui (gpui feature) ──► Surface 渲染/样式
                                        │                     ▲
                                        ├─► flick-application::presenter（PanelState / UiEvent / Effect）
                                        ├─► flick-runtime（仅 demo 的 fake_host 用）
                                        └─► gpui-kit 0.11（窗口 / Input / 组件）
```

依赖（见 [Cargo.toml](Cargo.toml)）：`flick-domain`、`flick-application`、`flick-runtime`、`flick-builtins`、`flick-adapters-linux`、`flick-ui`（启用 `gpui` feature）、`gpui-kit 0.11`、`async-trait`、`tokio`。本 crate 通过 `HostToUi` / `UiToHost` 通道与 daemon 通信，没有 Cargo 层反向依赖。

## 如何运行

库入口 [lib.rs:175](src/lib.rs#L175) 的 `run(host_tx, host_rx)` 用 `detect_initial_theme()` 转发到 [run_with_theme:364](src/lib.rs#L364)，后者按以下顺序启动：

1. `application().with_assets(gpui_kit::assets::Assets).with_quit_mode(Explicit).run(...)`（[lib.rs:369](src/lib.rs#L369)）建立 GPUI 应用；初始化 `ClipboardWatch`（[lib.rs:376](src/lib.rs#L376)）并配置主题 / `window_radius = 26` / 字体族（[lib.rs:378](src/lib.rs#L378)–[lib.rs:395](src/lib.rs#L395)）。
2. `cx.bind_keys` 绑定动作（[lib.rs:396](src/lib.rs#L396)）：`Escape`/`↑`/`↓`、`Ctrl+Shift+L`（主题）、`Ctrl+D`（分离）、`CloseDetached`，分别落在 `FlickPanel` / `FlickDetached` 两个 key context。
3. 建四条应用级通道 `hide_tx` / `detach_tx` / `close_tx` / `theme_tx`（[lib.rs:407](src/lib.rs#L407)–[lib.rs:413](src/lib.rs#L413)）。
4. 应用级事件泵 `cx.spawn`（[lib.rs:419](src/lib.rs#L419)）用 `tokio::select!`（[lib.rs:432](src/lib.rs#L432)）同时收 `HostToUi` 与上述通道：`ShowSearchPanel` → `open_panel_window` 建窗（[lib.rs:445](src/lib.rs#L445)），`HideSearchPanel` → 销毁（[lib.rs:484](src/lib.rs#L484)），`SurfaceUpdated` / `SurfaceFailed` 按 `session_id` 路由到面板或独立窗口（[lib.rs:493](src/lib.rs#L493)–[lib.rs:569](src/lib.rs#L569)），detach / close / theme / hide 分别处理（[lib.rs:585](src/lib.rs#L585)–[lib.rs:659](src/lib.rs#L659)）；`Shutdown` 分支先关掉所有独立窗口再 `app.quit()`（[lib.rs:436](src/lib.rs#L436)）。

窗口生命周期是「隐藏即销毁」：面板本体是全屏透明 overlay（[view.rs:1](src/view.rs#L1) 模块注释），`open_panel_window`（[lib.rs:185](src/lib.rs#L185)）优先 layer-shell overlay（namespace `flick:launcher`、`Layer::Overlay`、`exclusive_zone = 0`、`KeyboardInteractivity::Exclusive`），不支持时回退普通全屏透明窗口；关闭时 `remove_window`，下次唤起是全新窗口，规避 Wayland 下 resize 焦点抖动与「后台应用无法抢焦点」。独立窗口由 `open_detached_window`（[lib.rs:287](src/lib.rs#L287)）按纯函数 `detached_bounds`（[detached.rs:79](src/detached.rs#L79)）居中 + 级联 + 夹进取景；其内容是分离瞬间的快照，不订阅 daemon。

`AppView` 的事件闭环：[view.rs:319](src/view.rs#L319) `dispatch` 把 `UiEvent` 交给框架无关的 `AppState::update`（[app.rs:74](src/app.rs#L74)），得到的 `Effect` 由 [view.rs:401](src/view.rs#L401) `apply` 转成发往宿主的 `UiToHost` / 启动防抖 / 分离 / 退出。

演示二进制入口 [main.rs:343](src/main.rs#L343)：建两条 mpsc，spawn current-thread Tokio 跑 `fake_host`（[main.rs:233](src/main.rs#L233)，内含 `ActionRuntime` + 内置动作 + `DemoClipboardPlugin`），然后 `detect_initial_theme` + `run_with_theme`。

## 内部技术与架构细节

- **视图树与事件捕获**：[view.rs:518](src/view.rs#L518) 的 `Render for AppView` 在隐藏时只画空 `div().id("flick-root")`；可见时根节点是全屏透明层，外层捕 `on_mouse_left`（点外部 → `WindowClosed`）、鼠标移动 / 抬起（拖动面板）、`on_drop::<ExternalPaths>`（文件拖放）；内层卡片 `.id("flick-panel")` 只按 `desired_size` 改**卡片高度**，不调用 `window.resize`，这是 overlay 不抢焦点抖动的关键。键盘通过 `key_context` + `capture_key_down` / `capture_action` 捕获，面板与独立窗口各有自己的 context。
- **状态机与防抖**：`UiEvent`（[app.rs:17](src/app.rs#L17)）→ `AppState::update`（[app.rs:74](src/app.rs#L74)）→ `Effect`（[app.rs:44](src/app.rs#L44)，`Send` / `StartDebounce` / `Quit` / `Detach`）；`DEBOUNCE_MILLIS = 150`（[app.rs:13](src/app.rs#L13)）。`apply` 用 `cx.spawn_in` + revision 计时，旧任务被 drop 即取消，避免旧查询乱序回包。
- **Surface 内可编辑控件**：`collect_inputs`（[view.rs:1049](src/view.rs#L1049)）递归 `Section` / `Row` / `Column` 收集 `InputSpec`（[view.rs:1043](src/view.rs#L1043)，id / value / placeholder）；`sync_surface_widgets`（[view.rs:337](src/view.rs#L337)）按节点 id 缓存 `Entity<InputState>`，订阅 `Change` → `SurfaceEventKind::TextChanged`、`PressEnter` → `Submitted`，仅在未聚焦时同步 value，避免打断输入。注意本 crate 的类型叫 `InputSpec`；`InputWidgetSpec` 是 `flick-ui` 的 `render.rs`（[render.rs:68](../flick-ui/src/render.rs#L68)）里的注入描述。
- **附件与剪贴板链**：`input_capture.rs` 是纯函数层——`paths_from_clipboard_text`（[input_capture.rs:85](src/input_capture.rs#L85)）识别 Linux 文件管理器以 `text/plain` 给出的路径文本，`snapshot_from_clipboard_item`（[input_capture.rs:140](src/input_capture.rs#L140)）按「图片 > 外部路径 > 文本」优先级产出 `SearchInputSnapshot`，`clipboard_fingerprint`（[input_capture.rs:179](src/input_capture.rs#L179)）用 `DefaultHasher` 算指纹。副作用在 `clipboard_watch.rs`：应用级轮询 1s（[clipboard_watch.rs:38](src/clipboard_watch.rs#L38)）、5s 变更窗口（[clipboard_watch.rs:36](src/clipboard_watch.rs#L36)）、`baseline_known` 避免启动时误导入旧剪贴板（[clipboard_watch.rs:19](src/clipboard_watch.rs#L19)）；轮询任务持有 `&mut AsyncApp`，窗口销毁 / 重建不丢状态。
- **主题与尺寸单一来源**：`PANEL_WIDTH = 666.0`（[theme.rs:7](src/theme.rs#L7)）同时供 `Metrics::default().width` 与 `detached::DETACHED_WIDTH`；`ThemeMode` / `UiTheme` / `Metrics`（[theme.rs:11](src/theme.rs#L11) / [theme.rs:88](src/theme.rs#L88) / [theme.rs:31](src/theme.rs#L31)）是纯数据，`resolve()`（[theme.rs:110](src/theme.rs#L110)）产出配色与度量。初始主题按「配置文件 > `FLICK_SHELL_THEME` > gsettings > 暗色」探测（[lib.rs:137](src/lib.rs#L137)）；写回由 `persist_theme`（[view.rs:1080](src/view.rs#L1080)）完成。
- **字体解析**：`ui_font_family` / `ui_mono_font_family`（[lib.rs:62](src/lib.rs#L62) / [lib.rs:79](src/lib.rs#L79)）用 `fc-match` 探测，刻意避开不存在的 `.SystemUIFont` 并优先 Noto Sans CJK，保证中英文混排。

## 公开 API / 核心类型

| 项 | 位置 | 作用 |
| --- | --- | --- |
| `run` / `run_with_theme` | [lib.rs:175](src/lib.rs#L175) / [lib.rs:364](src/lib.rs#L364) | 启动 GPUI 应用与宿主事件泵 |
| `AppView` | [view.rs:47](src/view.rs#L47) | 搜索面板视图（状态 + 输入实体 + 焦点 + 通道） |
| `DetachedSurfaceView` | [detached.rs:131](src/detached.rs#L131) | Ctrl+D 分离出的独立 Surface 窗口（内容快照，不订阅 daemon） |
| `AppState` / `UiEvent` / `Effect` | [app.rs:52](src/app.rs#L52) / [app.rs:17](src/app.rs#L17) / [app.rs:44](src/app.rs#L44) | 框架无关的状态机：事件→效果 |
| `desired_size` | [app.rs:208](src/app.rs#L208) | 由 `PanelState` 与 `Metrics` 算卡片尺寸 |
| `ThemeMode` / `UiTheme` / `Metrics` | [theme.rs:11](src/theme.rs#L11) / [theme.rs:88](src/theme.rs#L88) / [theme.rs:31](src/theme.rs#L31) | 明暗主题与布局常量（`PANEL_WIDTH = 666`） |
| `detect_initial_theme` | [lib.rs:137](src/lib.rs#L137) | 配置文件 > `FLICK_SHELL_THEME` > gsettings > 暗色回退 |
| `ui_font_family` / `ui_mono_font_family` | [lib.rs:62](src/lib.rs#L62) / [lib.rs:79](src/lib.rs#L79) | 解析中英文字体（含 `fc-match`） |
| `detached_bounds` / `detached_size` | [detached.rs:79](src/detached.rs#L79) / [detached.rs:59](src/detached.rs#L59) | 独立窗口定位与尺寸（纯函数） |
| `InputSpec` / `collect_inputs` | [view.rs:1043](src/view.rs#L1043) / [view.rs:1049](src/view.rs#L1049) | Surface 内可编辑 Input/TextArea 的描述与收集 |

`lib.rs` 还重导出了 `flick_application::presenter` 的 `PanelAction` / `PanelEvent` / `PanelState` / `SelectionState`（[lib.rs:12](src/lib.rs#L12)），供宿主侧直接使用。

## 模块 / 文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | 对外 API、主题探测/字体解析、窗口创建、应用级事件泵 |
| [src/main.rs](src/main.rs) | `flick-gpui-demo` 演示二进制与假的宿主运行时 |
| [src/app.rs](src/app.rs) | 纯逻辑状态机 `AppState::update`、防抖与尺寸计算 |
| [src/view.rs](src/view.rs) | 唯一接触 GPUI 的渲染层：面板视图、输入捕获、附件 chip、动作绑定 |
| [src/detached.rs](src/detached.rs) | 独立窗口视图、级联定位、尺寸估算 |
| [src/input_capture.rs](src/input_capture.rs) | 纯函数：路径/剪贴板条目 → `SearchInputSnapshot`，文件 URI 编解码 |
| [src/clipboard_watch.rs](src/clipboard_watch.rs) | 应用级剪贴板轮询与 5 秒变更检测 |
| [src/theme.rs](src/theme.rs) | 纯数据主题与度量，不依赖 GPUI |

## 测试与常用命令

```bash
cargo test -p flick-shell-gpui                       # 全部单测（app/theme/input_capture/detached/view）
cargo test -p flick-shell-gpui --lib                 # 仅库单测
cargo check -p flick-shell-gpui
cargo run -p flick-shell-gpui --bin flick-gpui-demo   # 独立演示，不依赖 daemon
```

主题配置文件：`$XDG_CONFIG_HOME/flick/shell-gpui.toml`（仅一行 `mode = "dark" | "light"`，见 [lib.rs:97](src/lib.rs#L97)）；环境变量 `FLICK_SHELL_THEME=dark|light` 可覆盖 gsettings 探测。

## 与其他 crate 的关系

- `flick-daemon`：在主线程调用本 crate 的 `run_with_theme()` 起 UI，是唯一的生产者；两者靠 `UiToHost` / `HostToUi` 通道协作。
- `flick-ui`：本 crate 启用其 `gpui` feature，用 `render_surface_with_inputs` / `SurfaceStyle` 渲染 Surface，布局字段共用以确保尺寸估算与渲染一致。
- `flick-application`：复用 `presenter` 的 `PanelState` / `UiEvent` / `Effect`，状态机前移到这里，GPUI 只做呈现。
- `flick-runtime` / `flick-builtins`：仅 `flick-gpui-demo` 的 `fake_host` 使用，用于独立演示。
- `flick-adapters-linux`：链入依赖但未直接在 UI 代码中使用（Portal 快捷键在 daemon 侧）。
