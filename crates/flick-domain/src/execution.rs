//! 搜索阶段通用执行项与调用来源。

use serde::{Deserialize, Serialize};

use crate::id::{ActionId, InvocationId};

/// 一次 ActionRequest 是由哪个阶段发起的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ActionSource {
    /// 由 UI 搜索面板直接发起。
    #[default]
    Ui,
    /// 由搜索路由阶段发起。
    SearchRoute,
    /// 由 CLI 发起。
    Cli,
    /// 由插件内部发起。
    Plugin,
    /// 由系统事件触发。
    SystemEvent,
    /// 由 MCP 接口发起。
    Mcp,
}

/// 用户对 Execution 项的操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionEventKind {
    /// 用户激活（执行）该条目。
    Activate,
}

/// `search_invocable` Action 返回 `ActionOutcome::Data` 后，
/// Runtime 归一化出的通用执行项。`output` 仅供父 Action 使用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionItem {
    /// 条目在父 Action 内的稳定标识。
    pub item_id: String,
    /// 产出该条目的父 Action。
    pub parent_action_id: ActionId,
    /// 父 Action 本次搜索调用的标识。
    pub invocation_id: InvocationId,
    /// 主标题。
    pub title: String,
    /// 次级说明。
    pub subtitle: Option<String>,
    /// 图标名或标识。
    pub icon: Option<String>,
    /// 角标文本。
    pub badge: Option<String>,
    /// 结构化数据，仅供父 Action 激活时使用。
    pub output: serde_json::Value,
}
