//! Core Action types: descriptors, requests, inputs, outcomes.

use crate::caller::Caller;
use crate::error::ActionError;
use crate::id::{ActionId, InvocationId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ─── Descriptor ───────────────────────────────────────────────────────────────

/// Describes a registered Action for routing and display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDescriptor {
    pub id: ActionId,
    pub title: String,
    /// Keywords for plain-text search recall.
    pub keywords: Vec<String>,
    pub matcher: Matcher,
    pub concurrency: ConcurrencyPolicy,
}

/// How this Action is matched against user input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Matcher {
    /// Keyword-based recall from plain-text input.
    Keyword,
    /// Exact prefix command with word-boundary matching.
    /// E.g., `echo` matches "echo hello" but not "echohello".
    PrefixCommand { command: String },
}

impl Matcher {
    /// Convenience constructor for a prefix-command matcher.
    pub fn prefix_command(command: &str) -> Self {
        Self::PrefixCommand {
            command: command.to_owned(),
        }
    }

    /// Returns the command string if this is a `PrefixCommand` matcher.
    pub fn as_prefix_command(&self) -> Option<&str> {
        match self {
            Self::PrefixCommand { command } => Some(command),
            _ => None,
        }
    }
}

/// Concurrency policy for an Action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConcurrencyPolicy {
    /// Multiple invocations can run in parallel.
    Parallel,
    /// Only one invocation at a time; new requests queue or reject.
    Exclusive,
}

// ─── Input ────────────────────────────────────────────────────────────────────

/// Input for an Action invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionInput {
    pub kind: InputKind,
    pub text: String,
}

impl ActionInput {
    pub fn plain_text(text: impl Into<String>) -> Self {
        Self {
            kind: InputKind::PlainText,
            text: text.into(),
        }
    }
}

/// Phase 0 input kind classifier.
///
/// Phase 1 will replace/augment this with a richer InputFacet model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputKind {
    PlainText,
}

// ─── Request ──────────────────────────────────────────────────────────────────

/// A request to execute a registered Action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub invocation_id: InvocationId,
    pub action_id: ActionId,
    pub input: ActionInput,
    pub caller: Caller,
    pub deadline: Deadline,
}

impl ActionRequest {
    /// Create an Echo action request (Phase 0 convenience).
    pub fn new_echo(text: &str, caller: Caller) -> Self {
        Self {
            invocation_id: InvocationId::new(),
            action_id: ActionId::from_static("flick.builtin.echo.v1.echo"),
            input: ActionInput::plain_text(text),
            caller,
            deadline: Deadline::from_secs(30),
        }
    }
}

// ─── Deadline ─────────────────────────────────────────────────────────────────

/// Deadline for an invocation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Deadline {
    pub timeout: Duration,
}

impl Deadline {
    pub fn from_secs(secs: u64) -> Self {
        Self {
            timeout: Duration::from_secs(secs),
        }
    }
}

// ─── Context ──────────────────────────────────────────────────────────────────

/// Runtime context provided to an ActionHandler during invocation.
///
/// Carries a cancellation token for cooperative cancellation.
pub struct ActionContext {
    pub cancellation: tokio_util::sync::CancellationToken,
}

// ─── Outcome ──────────────────────────────────────────────────────────────────

/// The result of an Action invocation.
///
/// Framework-agnostic: both UI and CLI consume this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ActionOutcome {
    /// Structured data result.
    Data(serde_json::Value),
    /// A human-readable notice (not an error).
    Notice { message: String },
    /// An error occurred during execution.
    Error(ActionError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerKind;

    #[test]
    fn action_request_echo_has_ui_caller() {
        let req = ActionRequest::new_echo("hello", Caller::ui("search-panel"));
        assert_eq!(req.caller.kind, CallerKind::Ui);
        assert_eq!(req.caller.id, "search-panel");
    }

    #[test]
    fn action_request_echo_has_unique_invocation() {
        let a = ActionRequest::new_echo("test", Caller::cli("cli"));
        let b = ActionRequest::new_echo("test", Caller::cli("cli"));
        assert_ne!(a.invocation_id, b.invocation_id);
    }

    #[test]
    fn action_outcome_data_serializes() {
        let outcome = ActionOutcome::Data(serde_json::json!({"echo": "hello"}));
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("hello"));
    }

    #[test]
    fn action_outcome_round_trips() {
        let outcomes = vec![
            ActionOutcome::Data(serde_json::json!({"echo": "hi"})),
            ActionOutcome::Notice {
                message: "done".into(),
            },
            ActionOutcome::Error(ActionError::Cancelled),
        ];
        for outcome in outcomes {
            let json = serde_json::to_string(&outcome).unwrap();
            let back: ActionOutcome = serde_json::from_str(&json).unwrap();
            // Debug comparison
            assert_eq!(format!("{outcome:?}"), format!("{back:?}"));
        }
    }

    #[test]
    fn matcher_prefix_command_round_trips() {
        let m = Matcher::prefix_command("echo");
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("echo"));
        let back: Matcher = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_prefix_command(), Some("echo"));
    }
}
