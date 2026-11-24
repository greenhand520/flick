# 搜索面板「最近工具·发芽条」交互与动画设计

- 状态：Draft（设计定稿；**初期不实现，仅存文档**）
- 日期：2026-11-24
- 相关：`crates/flick-shell-iced/src/panel.rs`、`crates/flick-domain/src/ui.rs`、`crates/flick-application/src/ports.rs`
- 动画参考实现：`/mnt/LinuxData/eartholnpc/Downloads/SpotlightModeMorphSurface.qml`

> **实现策略（重要）**：本设计**初期不实现，仅输出文档存档**。原因：这些功能目前都可以通过搜索框下方的 UI（路由结果列表）直接完成，发芽条属于后期可选的增强交互。§8 分期仅为未来落地时的路线图。

## 1. 背景与问题

Flick 的搜索面板当前由 `crates/flick-shell-iced` 实现，逻辑面板状态机在 `crates/flick-shell-iced/src/panel.rs`（307 行，零 iced 依赖），facet 由 `QueryPlanner` 自动识别，路由结果列表显示在输入框下方。计划把 UI 迁移到 gpui-kit，并在搜索框上引入类似 QML `SpotlightModeMorphSurface` 的「发芽 / morph」动画。

该 QML 动画的本质是：

- **单一 `railProgress`（0..1）驱动一切**——主胶囊宽度、圆钮位置/大小、粘连量、模糊区域都是它的纯函数，因此可打断、可反向、可随进度"擦洗"。
- **主胶囊弹簧展开**（阻尼响应，带轻微过冲）。
- **若干圆形按钮从右端错峰发芽**（每个有独立的延迟/衰减/频率 / 放大速率），像一串被甩出、逐个弹开的珠子。
- **metaball 粘连 + 背景模糊**：相邻圆钮先"融合出脖子"再分离，视觉上是"一块软体分裂成几个"。

动画本身只有装饰价值，必须绑定一个明确的交互语义。本设计把它落到：**面板刚弹出、输入为空时，从输入胶囊右端发芽出"最近使用的工具/动作"快捷入口，约 3 秒后自动收回。**

## 2. 目标 / 非目标

**目标**

- 定义「发芽条（Recents Sprout Rail）」的完整交互：状态机、快捷键、生命周期、排序。
- 给出动画移植方案，并说明 gpui 下的可行退化。
- 明确代码落点：框架无关的共享状态机、协议新增、新的 gpui shell。

**非目标**

- 不追求逐像素复刻 metaball SDF 融合（gpui 公开 API 目前不具备任意 fragment shader / SDF 能力，见 §4.3）。
- 不做完整的 iced → gpui 全量迁移；本设计只覆盖该面板 + 该动画。
- 不改动 iced shell 的既有对外行为（仅为共享状态机做等价抽取）。
- 初期不落地实现；本阶段只输出设计文档（见实现策略）。

## 3. 交互规格（状态机）

### 3.1 形态（"两段式单链"）

- **第一段**：输入胶囊（搜索框本身）。
- **第二段**：从胶囊右端发芽出来的圆钮串（≤4 个），即"最近工具/动作"。

```
发芽期（输入为空，刚弹出）：
[ 搜索…                        ]  (应用)(文件)(剪贴板)(OCR)

3s 到 / 开始打字 → 依次缩回：
[ 搜索…                        ]  ›            ← 只留一个小 chevron
```

### 3.2 状态

| 状态 | 描述 | 进入条件 |
|---|---|---|
| `Hidden` | 面板未显示 | 初始 / 面板关闭 |
| `Revealing` | `recentsProgress 0→1`，圆钮依次发芽 | 面板显示且输入为空、且存在 recents |
| `Resident` | 圆钮全显，停留窗口约 3s | `Revealing` 完成 |
| `Collapsing` | `recentsProgress 1→0`，圆钮依次缩回 | 停留超时 / 用户开始输入 / 按 Esc |
| `Collapsed` | 只剩输入胶囊 + 右端 chevron | `Collapsing` 完成 |

### 3.3 事件与转移

- `Tab`（或点击 chevron）：`Collapsed → Revealing`（重新发芽）。
- `Alt+1 … Alt+4`：激活左→右第 N 个最近工具/动作（**面板打开期间始终生效**，不依赖发芽条是否可见，便于肌肉记忆）。默认使用 `Alt` 修饰键以避免与"输入数字"冲突；**支持用户自定义改键**。
- 用户在输入框**开始输入** → 触发 `Collapsing`，并让数字/字符回归文本输入。
- `Esc` → `Collapsing`。
- 选中某个圆钮（点击或 `Alt+n`）→ 面板 morph 到对应工具/动作（例如切到应用搜索 / 文件搜索 / 剪贴板 / OCR 的视图或预填查询）。

### 3.4 视觉细节

- **不显示数字角标**（角标影响美观）；hover 时以 tooltip 提示 `Alt+n`。
- 停留时长 3s（可配置）。
- 排序：**最近使用时间 + 使用频次**混合，取前 4；不足 4 个显示实际数量；recents 为空时展示兜底项：应用搜索、设置、文件搜索、剪贴板。

## 4. 动画规格

### 4.1 驱动与时间线

- 由单一、可逆的 `recentsProgress ∈ [0,1]` 驱动。
- 时间线：发芽约 420ms（弹簧），停留约 3s，收回约 380ms（弹簧）。
- 任何时刻被"打字 / Tab / Esc"打断都能反向或重新采样，不播放一次性动画。

### 4.2 从 QML 直接移植的数学

参考文件 `/mnt/LinuxData/eartholnpc/Downloads/SpotlightModeMorphSurface.qml` 中的纯函数原样搬运（它们都只依赖 progress）：

- `smoothstep(value)`、`stage(start, end)`、`interpolate(from, to, progress)`
- `response(delay, decay, frequency, phase)`：归一化的阻尼响应
  `1 - exp(-decay*t) * (cos(freq*t) + phase*sin(freq*t))`，并对端点归一化，保证终态精确、可反向。
- `buttonGrowth(index)`、`buttonCenterX(index)`、`buttonShape(index)`：圆钮发芽与错峰级联。
- 关键常数直接采用 QML 初值：
  - `travelDelays = [0.06, 0.036, 0.032]`
  - `travelDecays = [7.2, 5.4, 5.4]`
  - `travelFrequencies = [8.9, 6.2, 5.35]`
  - `growthRates = [3.8, 3.1, 2.7]`

### 4.3 gpui 实现与退化

- **几何**：输入胶囊用圆角矩形（半径 = 高/2）；圆钮用圆形/圆角矩形。
- **动画采样**：用 gpui 的动画能力（`Animation` + 自定义 easing，或每帧按 `recentsProgress` 采样）驱动上述纯函数。
- **metaball 融合的难点**：gpui 未向应用层开放任意 fragment shader / SDF，无法像 QML 的 `ShaderEffect` 那样做真正的"液体脖子"。退化方案按性价比排序：
  1. **（默认）只做"珠子依次弹出"**：不画桥，纯圆钮错峰弹簧发芽/缩回。最干净、实现成本最低。
  2. **颈桥近似**：在相邻圆钮之间绘制随进度伸缩的胶囊"连接杆"，观感上"连成串"，但没有液体融合的细腻。
  3. 若未来 gpui 支持自定义 paint / shader，再实现真正的 metaball + blur。
- **无障碍**：尊重系统"减少动态效果"设置（reduce motion）→ 退化为淡入淡出或瞬时出现。

## 5. 代码架构与落点

**现实约束**：仓库当前**没有 `flick-shell-gpui`，也没有任何 gpui 依赖**，只有 `crates/flick-shell-iced`。因此该动画是"新建 gpui shell"的一部分，不是往现有 crate 里塞。

### 5.1 抽出框架无关的共享状态机

- 把 `crates/flick-shell-iced/src/panel.rs` 抽到新 crate（建议 `crates/flick-shell-core`，或复用现有命名风格），让 iced / gpui 两个 shell 共用。
- 新增发芽条相关状态与事件，例如：
  - `PanelEvent::{RecentsShown, RecentsHidden, ToggleRecents, ActivateRecent { slot: u8 }, TypingStarted}`
  - 状态字段：`recents_progress: f32`、`recents_visible: bool`、`resident_deadline: Option<Instant>`、`recents: Vec<RecentTool>`。
- 现有 `PanelEvent` / `PanelAction` 变体（`Show`、`InputChanged`、`Routes`、`InvocationCompleted`、`SendToHost`、`StartDebounce` 等）保持不变以兼容 iced shell。

### 5.2 协议新增（`crates/flick-domain/src/ui.rs`）

- `HostToUi::RecentTools { items: Vec<RecentTool> }`：把"最近工具/动作"推给 UI。
- 激活路径二选一（需定）：复用 `UiToHost::QueryChanged` 预填查询，或新增 `UiToHost::ActivateTool { tool_id }` 由 `ApplicationCoordinator` 处理。
- 复用现有 `RuntimePort` / `HostEventSink`（`crates/flick-application/src/ports.rs`），不引入 UI 依赖。

### 5.3 gpui 渲染层

- 新建 `crates/flick-shell-gpui`：窗口 + `app::run` + 本动画的自定义 element（绘制胶囊 / 圆钮 / 可选颈桥）。
- 与 `flick-daemon` 的组装方式对齐 iced：UI 侧持有 `ui_cmd_tx` / `host_event_rx` 两条通道。

### 5.4 数据来源

- 单元是**工具与动作混合**：统一抽象为 `RecentTool { id, title, icon, kind }`（`kind` 区分 tool / action），来源可以是插件工具，也可以是插件内的具体动作。
- 使用记录 `UsageRecord { tool_id, last_used_at, count }` **持久化在 runtime 层**（跨会话记忆），不放 UI 层。

## 6. 错误与边界

- **recents 为空**：展示兜底项（应用搜索、设置、文件搜索、剪贴板），不阻塞面板。
- **不足 4 个**：按实际数量发芽，索引连续。
- **动画期间输入**：立即转入 `Collapsing`，数字回归文本；不得吞掉按键。
- **`Alt+数字` 与桌面环境冲突**：部分 DE 将 `Alt+1..9` 绑给全局切换窗口；Wayland 下应用内通常无碍。**快捷键支持用户自定义**，冲突时可改修饰键/组合。
- **无 gpui crate**：需先完成 shell 骨架，本动画依赖它。
- **多显示器 / Wayland 定位**：沿用现有面板窗口的定位策略，不在本设计范围。

## 7. 测试

- **状态机单测**（框架无关，易测）：发芽完成→停留→超时收回；`Tab` 重发芽；`Alt+n` 激活索引正确；输入打断；recents 排序与裁剪；端点归一化保证终态精确、可反向。
- **动画属性测试**：`recentsProgress` 单调且在 0/1 端点精确；中断/反向仍是 progress 的纯函数。
- **协议集成**：构造 `HostToUi::RecentTools` 假数据驱动 UI。

## 8. 分期

> 注：按上文实现策略，本设计初期不实现，以下仅为未来落地路线图。

- **Phase 1**：抽 `flick-shell-core` + 协议新增 + 状态机与单测 + `flick-shell-gpui` 原型（退化方案 1，无 gooey）。
- **Phase 2**：recents 持久化（runtime 侧）+ 排序策略打磨。
- **Phase 3**（可选）：颈桥近似 / 若 gpui 支持 shader 再做真 metaball + blur。

## 9. 决策记录

本轮评审已确定：

- **发芽条单元**：工具（plugin）与动作（action）**混合**，统一为 `RecentTool`（`kind` 区分）。
- **recents 持久化**：归属 **runtime 层**；具体容量/淘汰策略留到实现阶段再定。
- **快捷键**：默认 `Alt+1..4`，**支持用户自定义改键**。
- **recents 为空兜底**：应用搜索、设置、文件搜索、剪贴板。

遗留（不影响本设计存档）：具体容量/淘汰策略、激活协议路径（复用 `QueryChanged` vs 新增 `ActivateTool`）在实现阶段确定。
