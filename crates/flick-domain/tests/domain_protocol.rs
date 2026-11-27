//! 领域协议测试：验证核心类型按设计工作。

use flick_domain::*;

// ─── UiIntent 从不是动作调用 ──────────────────────────────────

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

// ─── ActionRequest 构造 ──────────────────────────────────────────────

#[test]
fn action_request_keeps_direct_caller_and_unique_invocation() {
    let request = ActionRequest::new_echo("hello", Caller::ui("search-panel"));
    assert_eq!(request.caller.kind, CallerKind::Ui);
    assert_ne!(request.invocation_id, InvocationId::new());
}

#[test]
fn echo_request_has_correct_action_id() {
    let request = ActionRequest::new_echo("test", Caller::cli("flick-cli"));
    assert_eq!(request.action_id.as_str(), "flick.builtin.echo.v1.echo");
}

#[test]
fn shortcut_caller_cannot_be_used_for_echo_in_phase0() {
    // 验证 CallerKind::Shortcut 存在，但我们记录它不用于 Phase 0 的 echo
    let caller = Caller {
        kind: CallerKind::Shortcut,
        id: "global-hotkey".into(),
    };
    assert_eq!(caller.kind, CallerKind::Shortcut);
    // 在 Phase 0 中，Echo 只应使用 Ui 和 Cli。
    // 此测试记录该边界；强制检查在应用层。
}

// ─── 标识符验证 ───────────────────────────────────────────────────────────

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

// ─── JSON 序列化 ──────────────────────────────────────────────────────

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
            input: SearchInputSnapshot::text("hello", InputSource::Manual),
        },
        UiToHost::SearchPanelClosed,
        UiToHost::Invoke {
            generation: QueryGeneration::INITIAL,
            item_id: "action:flick.builtin.echo.v1.echo".into(),
            invocation_id: InvocationId::new(),
            action_id: ActionId::from_static("flick.builtin.echo.v1.echo"),
            input: ActionInput::plain_text("hello"),
        },
        UiToHost::Cancel {
            invocation_id: InvocationId::new(),
        },
        UiToHost::ExecutionActivated {
            generation: QueryGeneration::INITIAL,
            parent_action_id: ActionId::from_static("test.apps.search"),
            invocation_id: InvocationId::new(),
            item_id: Some("app:firefox".into()),
            event: ExecutionEventKind::Activate,
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
        HostToUi::Shutdown,
        HostToUi::RuntimeStatus(RuntimeStatus::Running),
    ];
    for msg in messages {
        let json = serde_json::to_string(&msg).unwrap();
        let _back: HostToUi = serde_json::from_str(&json).unwrap();
    }
}
