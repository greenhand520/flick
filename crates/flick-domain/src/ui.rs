//! UI ↔ Host 通信协议。

use crate::action::{ActionInput, ActionOutcome};
use crate::execution::ExecutionEventKind;
use crate::id::{ActionId, InvocationId, QueryGeneration, SurfaceSessionId};
use crate::input::SearchInputSnapshot;
use crate::route::RouteItem;
use crate::surface::{SurfaceContent, SurfaceEventKind};
use serde::{Deserialize, Serialize};

// ─── UI 意图（来自 Portal / 外部触发器）─────────────────────────────

/// 用于控制 Flick 自身 UI 状态的意图。
///
/// 这些不是动作调用。Portal 的 `show-search` 会产生一个
/// `UiIntent::ShowSearchPanel`，协调器将其转换为
/// `HostToUi::ShowSearchPanel` 消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiIntent {
    ShowSearchPanel {
        activation_token: Option<String>,
    },
    HideSearchPanel,
    /// 切换搜索面板：不可见则唤起、可见则收起。全局快捷键使用此语义。
    ToggleSearchPanel,
}

impl UiIntent {
    /// 此意图是否表示一次动作调用（永远不会）。
    pub fn is_action_invocation(&self) -> bool {
        false
    }
}

// ─── UI → Host 消息 ──────────────────────────────────────────────────────

/// 从 UI 发往 Host（协调器）的消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UiToHost {
    /// 查询文本或附件已变化；携带 UI 采集到的完整输入快照。
    QueryChanged {
        generation: QueryGeneration,
        input: SearchInputSnapshot,
    },
    /// 用户选择了一个路由项进行调用。
    ///
    /// `generation` 与 `item_id` 用于候选身份校验：宿主只接受属于当前
    /// 查询代次、且仍在最近一次结果集中的候选（设计文档 §12.2-3）。
    Invoke {
        /// 该候选所属的查询代次。
        generation: QueryGeneration,
        /// 候选的稳定标识（`RouteItem::item_id`）。
        item_id: String,
        /// 本次调用标识。
        invocation_id: InvocationId,
        /// 候选对应的 Action。
        action_id: ActionId,
        /// 调用输入。
        input: ActionInput,
    },
    /// 用户激活了一个搜索阶段 Execution 项。
    ///
    /// Execution 项不是独立 Action，UI 不自行解释它；宿主应把事件转交
    /// 产出该条目的父 Action，由父 Action 的 `execution_event` 决定行为。
    ExecutionActivated {
        /// 产生该 Execution 项的查询代次。
        generation: QueryGeneration,
        /// 产出该 Execution 项的父 Action。
        parent_action_id: ActionId,
        /// 本次激活使用的调用标识，用于把完成结果对应回 UI。
        invocation_id: InvocationId,
        /// 父 Action 定义的结果项标识。
        item_id: Option<String>,
        /// 用户操作类型（当前仅 Activate）。
        event: ExecutionEventKind,
    },
    /// 用户取消了一个进行中的调用。
    Cancel { invocation_id: InvocationId },
    /// 搜索面板被用户关闭。
    SearchPanelClosed,
    /// 用户在 Surface 中产生了交互事件。
    SurfaceEvent {
        session_id: SurfaceSessionId,
        kind: SurfaceEventKind,
    },
    /// 用户关闭了某个 Surface 会话。
    SurfaceClosed { session_id: SurfaceSessionId },
}

// ─── Host → UI 消息 ──────────────────────────────────────────────────────

/// 从 Host（协调器）发往 UI 的消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HostToUi {
    /// 守护进程正在停止；外壳必须退出其事件循环。
    Shutdown,
    /// 搜索查询的路由结果。
    Routes {
        generation: QueryGeneration,
        items: Vec<RouteItem>,
    },
    /// 搜索阶段执行项（设计文档 §7.3）。
    ///
    /// 在 [`HostToUi::Routes`] 之后异步补发：`Routes` 只含纯内存路由得到的
    /// action candidates，本事件补充各 Action 的搜索阶段执行结果。
    /// UI 必须按 `generation` 校验，丢弃旧代次的结果。
    ExecutionResults {
        /// 产生这批执行项的查询代次。
        generation: QueryGeneration,
        /// 执行项：`kind == RouteItemKind::Execution`，`parent_action_id` 指向父 Action。
        items: Vec<RouteItem>,
    },
    /// 一次调用已完成。
    InvocationCompleted {
        invocation_id: InvocationId,
        outcome: ActionOutcome,
    },
    /// Runtime 状态更新。
    RuntimeStatus(RuntimeStatus),
    /// 显示搜索面板（带有来自 Portal 的可选激活令牌）。
    ShowSearchPanel { activation_token: Option<String> },
    /// 隐藏搜索面板。
    HideSearchPanel,
    /// Surface 内容因交互而更新。
    SurfaceUpdated {
        session_id: SurfaceSessionId,
        content: SurfaceContent,
    },
    /// Surface 交互处理失败（保留旧内容，显示错误）。
    SurfaceFailed {
        session_id: SurfaceSessionId,
        message: String,
    },
}

// ─── Runtime 状态 ──────────────────────────────────────────────────────────

/// Runtime 的诊断状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuntimeStatus {
    Running,
    Degraded { reason: String },
    Failed { reason: String },
}
