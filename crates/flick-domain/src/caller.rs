//! 调用方标识：谁发起了 ActionRequest。

use serde::{Deserialize, Serialize};

/// ActionRequest 的直接发起者。
///
/// `kind` 标识类别；`id` 提供特定于实例的标签
/// （例如 UI 使用 "search-panel"，CLI 使用 "flick-cli"）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    pub kind: CallerKind,
    pub id: String,
}

impl Caller {
    pub fn ui(id: impl Into<String>) -> Self {
        Self {
            kind: CallerKind::Ui,
            id: id.into(),
        }
    }

    pub fn cli(id: impl Into<String>) -> Self {
        Self {
            kind: CallerKind::Cli,
            id: id.into(),
        }
    }
}

/// 调用方的类别。
///
/// Phase 0 仅使用 `Ui` 和 `Cli`。其他变体是协议级别的
/// 占位符，供未来阶段使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallerKind {
    /// Flick UI（搜索面板）。
    Ui,
    /// 通过 IPC 的命令行界面。
    Cli,
    /// 保留：未来用于直接调用全局快捷键。
    /// Phase 0 的 Portal 快捷键不使用此变体；它们会生成 UiIntent。
    Shortcut,
    /// 保留：未来用于内置或插件之间的调用。
    Plugin,
    /// 保留：未来用于系统事件（剪贴板、文件监视器、定时器）。
    SystemEvent,
    /// 保留：Phase 4 的 MCP 入口点。
    Mcp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_ui_has_correct_kind() {
        let c = Caller::ui("search-panel");
        assert_eq!(c.kind, CallerKind::Ui);
        assert_eq!(c.id, "search-panel");
    }

    #[test]
    fn caller_cli_has_correct_kind() {
        let c = Caller::cli("flick-cli");
        assert_eq!(c.kind, CallerKind::Cli);
    }
}
