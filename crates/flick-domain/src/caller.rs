//! Caller identity: who initiated an ActionRequest.

use serde::{Deserialize, Serialize};

/// The direct initiator of an ActionRequest.
///
/// `kind` identifies the category; `id` provides an instance-specific label
/// (e.g. "search-panel" for UI, "flick-cli" for CLI).
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

/// Category of caller.
///
/// Phase 0 only uses `Ui` and `Cli`. Other variants are protocol-level
/// placeholders for future phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallerKind {
    /// Flick UI (Iced search panel).
    Ui,
    /// Command-line interface via IPC.
    Cli,
    /// Reserved: future direct global shortcut invocation.
    /// Phase 0 Portal shortcuts do NOT use this; they produce UiIntent.
    Shortcut,
    /// Reserved: future built-in or plugin service invocation.
    Service,
    /// Reserved: future system events (clipboard, file watcher, timer).
    SystemEvent,
    /// Reserved: Phase 4 MCP entry point.
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
