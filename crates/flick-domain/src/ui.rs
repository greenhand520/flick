//! UI ↔ Host communication protocol.

use crate::action::{ActionInput, ActionOutcome};
use crate::id::{ActionId, InvocationId, QueryGeneration};
use crate::route::RouteItem;
use serde::{Deserialize, Serialize};

// ─── UI Intent (from Portal / external triggers) ─────────────────────────────

/// An intent to control Flick's own UI state.
///
/// These are NOT Action invocations. Portal `show-search` produces a
/// `UiIntent::ShowSearchPanel`, which the coordinator converts to a
/// `HostToUi::ShowSearchPanel` message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiIntent {
    ShowSearchPanel {
        activation_token: Option<String>,
    },
    HideSearchPanel,
}

impl UiIntent {
    /// Whether this intent represents an action invocation (it never does).
    pub fn is_action_invocation(&self) -> bool {
        false
    }
}

// ─── UI → Host messages ──────────────────────────────────────────────────────

/// Messages from the UI to the Host (coordinator).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiToHost {
    /// User typed/changed the search query.
    QueryChanged {
        generation: QueryGeneration,
        text: String,
    },
    /// User selected a route item to invoke.
    Invoke {
        invocation_id: InvocationId,
        action_id: ActionId,
        input: ActionInput,
    },
    /// User cancelled an in-flight invocation.
    Cancel {
        invocation_id: InvocationId,
    },
    /// The search panel was closed by the user.
    SearchPanelClosed,
}

// ─── Host → UI messages ──────────────────────────────────────────────────────

/// Messages from the Host (coordinator) to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostToUi {
    /// Route results for a search query.
    Routes {
        generation: QueryGeneration,
        items: Vec<RouteItem>,
    },
    /// An invocation completed.
    InvocationCompleted {
        invocation_id: InvocationId,
        outcome: ActionOutcome,
    },
    /// Runtime status update.
    RuntimeStatus(RuntimeStatus),
    /// Show the search panel (with optional activation token from Portal).
    ShowSearchPanel {
        activation_token: Option<String>,
    },
    /// Hide the search panel.
    HideSearchPanel,
}

// ─── Runtime Status ──────────────────────────────────────────────────────────

/// Diagnostic status of the Runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuntimeStatus {
    Running,
    Degraded { reason: String },
    Failed { reason: String },
}
