# flick-ui

Flick 的 Surface 呈现层：把 `flick-domain` 里的 `SurfaceContent` 样式化并渲染成 GPUI 元素，同时提供与 UI 框架无关的高度估算。

本 crate 只做「呈现」，不定义数据模型、不持有运行时、不打开窗口。数据模型在 `flick-domain`（`SurfaceNode` / `SurfaceContent` / `SurfaceEventKind` 等），布局渲染与交互由上层 `flick-shell-gpui` 接管。它的 `gpui` feature 默认关闭，方便无 GUI 的单测（如高度估算）直接用。

## 为什么设计这个 crate

- **模型与呈现分离**：`SurfaceNode` / `SurfaceContent` / `SurfaceEventKind` / `ButtonTone` 定义在 `flick-domain`；本 crate 只负责把它们变成颜色、尺寸与 GPUI 元素，因此运行时与插件可以构造 Surface 而不依赖任何 UI 框架。
- **feature 边界隔离 gpui**：`gpui` 默认关闭（`default = []`），`render` 模块只在开启时编译；`flick-builtins` 等不渲染的 crate 由此不会被拖入 gpui-kit。`flick-shell-gpui` 是唯一启用该 feature 的消费者。
- **布局逻辑可复用且可测**：`style.rs` 是纯数据 + 纯函数，不依赖 gpui；`estimate_height` / `node_height` 既被面板用于计算卡片高度，也被独立窗口的 `detached_size` 复用，保证「估算尺寸」与「实际渲染」使用同一套度量。
- **交互按需开启**：提供只读、带事件回调、带输入控件注入三级渲染入口，调用方按场景选择，不必复制布局代码。

## 在 workspace 中的位置

```
flick-domain (SurfaceContent / SurfaceNode)   ← 数据模型，仅此一项是硬依赖
        │
        ▼
flick-ui ──pub use──► SurfaceStyle / estimate_height / Surface*
        │
        ├── [gpui feature] render.rs ──► AnyElement（GPUI 元素）
        │                                  ▲
        │                                  │ 调用 render_surface_with_inputs
        └───────────────────────────── flick-shell-gpui（面板 + 独立窗口）
```

依赖（见 [Cargo.toml](Cargo.toml)）：`flick-domain` 必选；`gpui-kit 0.11` 可选，由 `gpui` feature（`default = []`）开启。因此 `--no-default-features` 下仍可编译 `style.rs`。

## 如何运行

本 crate 是库，没有 `main`，也不拥有事件循环；它的生命周期由调用方驱动：daemon → `flick-shell-gpui::run_with_theme` → `AppView::render` → 本 crate 的渲染入口。

1. **尺寸阶段**：`flick-shell-gpui` 在计算面板 / 独立窗口大小时调用 `estimate_height`（[style.rs:82](src/style.rs#L82)），它递归 `node_height`（[style.rs:93](src/style.rs#L93)）累加各节点高度；这里只估算、不做 clamp，最终夹取由上层 `desired_size` / `detached_size` 完成。
2. **渲染阶段**：`gpui` feature 下，[render.rs](src/render.rs) 提供三级入口——只读 `render_surface`（[render.rs:78](src/render.rs#L78)）/ `render_node`（[render.rs:115](src/render.rs#L115)）；带 `SurfaceEventKind` 回调的 `render_surface_with_events`（[render.rs:84](src/render.rs#L84)）；带 `input_widget` 注入的 `render_surface_with_inputs`（[render.rs:94](src/render.rs#L94)）与逐节点 `render_node_with`（[render.rs:315](src/render.rs#L315)）。上层用 `collect_inputs` 收集节点里的输入框，再按 id 注入自己持有的控件实体。
3. **构建 / 测试**：`cargo check -p flick-ui --no-default-features` 只编译 `style.rs`；`cargo test -p flick-ui --features gpui` 连带编译 `render.rs`。

## 内部技术与架构细节

- **`SurfaceStyle` 数据模型**：[style.rs:6](src/style.rs#L6) 定义 12 个 `(u8,u8,u8,u8)` 颜色字段与 `radius_row` / `pad` / `row_gap` / `line_height` / `section_gap` 五个布局字段；`Default`（[style.rs:29](src/style.rs#L29)）给出深色面板配色（`panel_bg = (20,20,24,250)`、`surface = (255,255,255,10)`、`primary = (103,80,164,255)`）与尺寸（`radius_row 14.0`、`pad 10.0`、`row_gap 8.0`、`line_height 22.0`、`section_gap 10.0`）。`button_colors`（[style.rs:55](src/style.rs#L55)）把 `ButtonTone` 映射为前景/背景：Primary 用主色、Danger 用 `badge_danger` + `panel_bg`、Neutral 用 `surface` + `text`。
- **文本截断**：`truncate_chars`（[style.rs:69](src/style.rs#L69)）按 Unicode 字符而非字节截断，超过 `max` 时保留 `max - 1` 个字符并补 `…`；渲染侧另设 `TEXT_MAX_CHARS = 200`（[render.rs:12](src/render.rs#L12)）与 `TITLE_MAX_CHARS = 80`（[render.rs:14](src/render.rs#L14)）防止超长文本撑爆卡片。
- **高度估算规则**：`node_height`（[style.rs:93](src/style.rs#L93)）对 Heading 取 `1.25 × line_height`、Text 按 `ceil(chars / 28)` 行、KeyValue / Badge 一行、List 每项 `1.6 ×` 并在带副标题 / badge 时加一行、Divider 1.0、Spacer 按 `Sm 6 / Md 12 / Lg 24`、Section 标题 `1.1 ×` 加 `section_gap`、Row 取子节点最大值、Column 求和加间距、Input / Button / Checkbox 固定 40.0、TextArea 按 `rows × line_height`。`estimate_height`（[style.rs:82](src/style.rs#L82)）累加 body 节点并在 section 间加 `section_gap`，不设上下限。
- **颜色转换与徽章**：`col`（[render.rs:17](src/render.rs#L17)）把 `(u8,u8,u8,u8)` 转成 `gpui::Rgba`；`tone_color`（[render.rs:26](src/render.rs#L26)）按语义取色；`badge_fg`（[render.rs:36](src/render.rs#L36)）按亮度（阈值 140）在 `panel_bg` 与 `text` 间选前景，保证徽章对比度。
- **渲染分层**：`render_surface_with_inputs`（[render.rs:94](src/render.rs#L94)）以 `flex_col` + `section_gap` 为根，先画标题再按序画 body；`render_node`（[render.rs:115](src/render.rs#L115)）只生成只读元素；`render_node_with`（[render.rs:315](src/render.rs#L315)）在它之上把 Button 接 `Click`、Checkbox 接 `Toggled`、ListItem 用 `on_mouse_down(MouseButton::Left)` 接 `Click`，并把 Input / TextArea 委托给调用方注入的 widget，未匹配的节点回退到 `render_node`。`InputWidgetSpec`（[render.rs:68](src/render.rs#L68)，id / value / placeholder / multiline / rows）是注入协议；上层 `flick-shell-gpui` 侧对应的缓存描述叫 `InputSpec`。

## 公开 API / 核心类型

| 项 | 位置 | 作用 |
| --- | --- | --- |
| `SurfaceStyle` | [style.rs:6](src/style.rs#L6) | 颜色 + `radius_row` / `pad` / `row_gap` / `line_height` / `section_gap` 等布局字段 |
| `button_colors` | [style.rs:55](src/style.rs#L55) | 按 `ButtonTone` 返回前景/背景色 |
| `truncate_chars` | [style.rs:69](src/style.rs#L69) | 按字符安全截断并补省略号 |
| `estimate_height` | [style.rs:82](src/style.rs#L82) | 估算 `SurfaceContent` 高度（纯函数，可无 GUI 单测） |
| `col` | [render.rs:17](src/render.rs#L17) | `(u8,u8,u8,u8)` → `gpui::Rgba` |
| `render_surface` | [render.rs:78](src/render.rs#L78) | 只读渲染 |
| `render_surface_with_events` | [render.rs:84](src/render.rs#L84) | 带交互回调的渲染 |
| `render_surface_with_inputs` | [render.rs:94](src/render.rs#L94) | 带输入控件注入的渲染 |
| `InputWidgetSpec` | [render.rs:68](src/render.rs#L68) | id / value / placeholder / multiline / rows，供上层复用控件 |
| `Surface*` 重导出 | [lib.rs:10](src/lib.rs#L10) | 直接 `pub use flick_domain::surface::*` |

`lib.rs`（[lib.rs:1](src/lib.rs#L1)）明确说明：模型在 `flick-domain`，本 crate 只做呈现；`render` 模块仅在 `gpui` feature 下编译（[lib.rs:8](src/lib.rs#L8)）。

## 模块 / 文件清单

| 文件 | 职责 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | crate 文档、模块声明、重导出（11 行） |
| [src/style.rs](src/style.rs) | `SurfaceStyle`、颜色、截断与高度估算 + 10 个单测（302 行） |
| [src/render.rs](src/render.rs) | GPUI 渲染：`col` / badge / surface / node，只读与交互两版（仅 `gpui` feature，478 行） |

无 `tests/` 与 `resources/` 目录；单测与源码同文件（`#[cfg(test)]` 模块）。

## 测试与常用命令

```bash
cargo test -p flick-ui                              # 默认无 gpui：只跑 style 单测
cargo test -p flick-ui --features gpui              # 连带编译 render.rs
cargo check -p flick-ui --no-default-features       # 验证无 GPU 依赖可编译
cargo doc -p flick-ui --open
```

- `cargo test -p flick-ui` 覆盖 `style.rs` 的 10 个单测：截断保持 / 补省略号 / 多字节安全、空内容零高、列表逐项增高、交互节点高度、textarea 行数、按钮配色等。
- 渲染相关没有单测，验证靠 `flick-shell-gpui` 的 UI 联调。

## 与其他 crate 的关系

- `flick-domain`：唯一硬依赖，`SurfaceNode` / `SurfaceContent` / `SurfaceEventKind` / `ButtonTone` 均来自它，`lib.rs` 直接重导出 `surface` 模块。
- `flick-shell-gpui`：唯一启用 `gpui` feature 的消费者，用 `render_surface_with_inputs` 与 `SurfaceStyle` 把 Surface 画进面板和独立窗口，并复用 `estimate_height` 保证尺寸估算与渲染不漂移。
- `gpui-kit 0.11`：仅 `gpui` feature 的可选依赖，提供 `AnyElement` / `Rgba` 与输入组件类型；本 crate 不依赖 daemon、IPC 或 Portal。
