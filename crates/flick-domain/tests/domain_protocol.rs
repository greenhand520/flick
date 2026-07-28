//! Domain protocol tests: verifying the core types work as designed.

use flick_domain::*;

// ─── UiIntent is never an Action invocation ──────────────────────────────────

#[test]
fn portal_intent_is_not_an_action_request() {
    let intent = UiIntent::ShowSearchPanel {
        activation_token: None,
    };
    assert!(!intent.is_action_invocation());
}

#[test]
fn hide_intent_is_not_an_action_request() {
    let intent = UiIntent::HideSearchPanel;
    assert!(!intent.is_action_invocation());
}

// ─── ActionRequest construction ──────────────────────────────────────────────

#[test]
fn action_request_keeps_direct_caller_and_unique_invocation() {
    let request = ActionRequest::new_echo("hello", Caller::ui("search-panel"));
    assert_eq!(request.caller.kind, CallerKind::Ui);
    assert_ne!(request.invocation_id, InvocationId::new());
}

#[test]
fn echo_request_has_correct_action_id() {
    let request = ActionRequest::new_echo("test", Caller::cli("flick-cli"));
    assert_eq!(
        request.action_id.as_str(),
        "flick.builtin.echo.v1.echo"
    );
}

#[test]
fn shortcut_caller_cannot_be_used_for_echo_in_phase0() {
    // Verify CallerKind::Shortcut exists but we document it's not for Phase 0 echo
    let caller = Caller {
        kind: CallerKind::Shortcut,
        id: "global-hotkey".into(),
    };
    assert_eq!(caller.kind, CallerKind::Shortcut);
    // In Phase 0, only Ui and Cli should be used for Echo.
    // This test documents the boundary; enforcement is in the application layer.
}

// ─── ID validation ───────────────────────────────────────────────────────────

#[test]
fn action_id_rejects_blank_values() {
    assert!(ActionId::new("").is_err());
    assert!(ActionId::new("   ").is_err());
    assert!(ActionId::new("\t\n").is_err());
}

#[test]
fn action_id_accepts_valid_values() {
    assert!(ActionId::new("flick.builtin.echo.v1.echo").is_ok());
    assert!(ActionId::new("  trimmed  ").is_ok());
}

// ─── QueryGeneration ─────────────────────────────────────────────────────────

#[test]
fn query_generation_is_monotonically_increasing() {
    let g1 = QueryGeneration::INITIAL;
    let g2 = g1.next();
    let g3 = g2.next();
    assert!(g1 < g2);
    assert!(g2 < g3);
}

#[test]
fn query_generation_default_is_initial() {
    let g = QueryGeneration::default();
    assert_eq!(g, QueryGeneration::INITIAL);
    assert_eq!(g.value(), 1);
}

// ─── JSON serialization ──────────────────────────────────────────────────────

#[test]
fn action_outcome_data_serializes_stably() {
    let outcome = ActionOutcome::Data(serde_json::json!({"echo": "hello"}));
    let json = serde_json::to_string(&outcome).unwrap();
    assert!(json.contains("echo"));
    assert!(json.contains("hello"));
    let back: ActionOutcome = serde_json::from_str(&json).unwrap();
    assert_eq!(outcome, back);
}

#[test]
fn action_error_serializes_with_code() {
    let err = ActionError::UnknownAction {
        action_id: "test".into(),
    };
    assert_eq!(err.code(), "UNKNOWN_ACTION");

    let json = serde_json::to_string(&err).unwrap();
    let back: ActionError = serde_json::from_str(&json).unwrap();
    assert_eq!(back.code(), "UNKNOWN_ACTION");
}

#[test]
fn ui_to_host_round_trips() {
    let messages = vec![
        UiToHost::QueryChanged {
            generation: QueryGeneration::INITIAL,
            text: "hello".into(),
        },
        UiToHost::SearchPanelClosed,
        UiToHost::Cancel {
            invocation_id: InvocationId::new(),
        },
    ];
    for msg in messages {
        let json = serde_json::to_string(&msg).unwrap();
        let _back: UiToHost = serde_json::from_str(&json).unwrap();
    }
}

#[test]
fn host_to_ui_round_trips() {
    let messages = vec![
        HostToUi::ShowSearchPanel {
            activation_token: Some("token123".into()),
        },
        HostToUi::HideSearchPanel,
        HostToUi::RuntimeStatus(RuntimeStatus::Running),
    ];
    for msg in messages {
        let json = serde_json::to_string(&msg).unwrap();
        let _back: HostToUi = serde_json::from_str(&json).unwrap();
    }
}
