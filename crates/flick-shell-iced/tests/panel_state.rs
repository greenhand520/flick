//! Panel state machine tests.
//!
//! Covers all assertions required by Task 8 Step 1:
//! - First ShowSearchPanel creates window state
//! - Second show just shows existing window
//! - Each input increments generation
//! - Old generation routes discarded
//! - Arrow up/down safely moves in empty/non-empty candidates
//! - Enter sends selected Action's UiToHost::Invoke
//! - Esc/close only hides
//! - Activation token handling
//! - Debounce flush behavior

use flick_domain::*;
use flick_shell_iced::panel::{PanelAction, PanelEvent, PanelState};

fn make_route(id: &str, title: &str, score: f64) -> RouteItem {
    RouteItem {
        action_id: ActionId::new(id).unwrap(),
        title: title.into(),
        score,
    }
}

// ─── Window lifecycle ────────────────────────────────────────────────────────

#[test]
fn initial_state_is_hidden_and_not_created() {
    let panel = PanelState::new();
    assert!(!panel.visible);
    assert!(!panel.created);
}

#[test]
fn first_show_creates_and_shows_window() {
    let mut panel = PanelState::new();
    let actions = panel.update(PanelEvent::Show {
        activation_token: None,
    });
    assert!(panel.visible);
    assert!(panel.created);
    assert!(actions.is_empty(), "show emits no outbound actions");
}

#[test]
fn second_show_just_shows_existing_window() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    panel.update(PanelEvent::Hide);
    assert!(!panel.visible);
    assert!(panel.created, "window still marked as created after hide");

    // Second show
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    assert!(panel.visible);
    assert!(panel.created);
}

#[test]
fn hide_emits_search_panel_closed() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    let actions = panel.update(PanelEvent::Hide);
    assert!(!panel.visible);
    assert_eq!(actions.len(), 1);
    assert!(matches!(
        &actions[0],
        PanelAction::SendToHost(UiToHost::SearchPanelClosed)
    ));
}

#[test]
fn window_closed_hides_panel() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    let actions = panel.update(PanelEvent::WindowClosed);
    assert!(!panel.visible);
    assert!(matches!(
        &actions[0],
        PanelAction::SendToHost(UiToHost::SearchPanelClosed)
    ));
}

#[test]
fn escape_hides_and_invalidates_debounce() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    let actions = panel.update(PanelEvent::EscapePressed);
    assert!(!panel.visible);
    assert_eq!(actions.len(), 1);
    assert!(matches!(
        &actions[0],
        PanelAction::SendToHost(UiToHost::SearchPanelClosed)
    ));
}

// ─── Query input and generation ──────────────────────────────────────────────

#[test]
fn input_changed_starts_debounce() {
    let mut panel = PanelState::new();
    let actions = panel.update(PanelEvent::InputChanged("hello".into()));
    assert_eq!(panel.query, "hello");
    assert_eq!(panel.pending_query, "hello");
    assert_eq!(actions.len(), 1);
    assert!(matches!(
        &actions[0],
        PanelAction::StartDebounce { revision } if *revision == panel.debounce_revision()
    ));
}

#[test]
fn debounce_flush_sends_current_generation_and_query() {
    let mut panel = PanelState::new();
    let gen_before = panel.generation;
    panel.update(PanelEvent::Show {
        activation_token: None,
    });

    panel.update(PanelEvent::InputChanged("hello".into()));
    let actions = panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });

    assert_eq!(panel.generation, gen_before.next());
    assert_eq!(panel.query, "hello");
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        PanelAction::SendToHost(UiToHost::QueryChanged { generation, text }) => {
            assert_eq!(*generation, panel.generation);
            assert_eq!(text, "hello");
        }
        other => panic!("expected QueryChanged, got {other:?}"),
    }
}

#[test]
fn each_input_increments_generation() {
    let mut panel = PanelState::new();
    let g0 = panel.generation;

    panel.update(PanelEvent::InputChanged("a".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let g1 = panel.generation;

    panel.update(PanelEvent::InputChanged("ab".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let g2 = panel.generation;

    assert!(g0 < g1);
    assert!(g1 < g2);
}

#[test]
fn debounce_flush_with_same_query_is_noop() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::InputChanged("hello".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });

    // Flush again with same pending query (unchanged)
    let actions = panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    assert!(actions.is_empty(), "same query should not re-flush");
}

// ─── Route handling and generation discard ───────────────────────────────────

#[test]
fn routes_for_current_generation_are_accepted() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::InputChanged("echo hello".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });

    let gn = panel.generation;
    let routes = vec![make_route("flick.builtin.echo.v1.echo", "Echo", 1.0)];
    panel.update(PanelEvent::Routes {
        generation: gn,
        items: routes,
    });

    assert_eq!(panel.routes.len(), 1);
    assert_eq!(panel.routes[0].title, "Echo");
}

#[test]
fn old_generation_routes_are_discarded() {
    let mut panel = PanelState::new();

    // First query
    panel.update(PanelEvent::InputChanged("first".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let gn1 = panel.generation;

    // Second query (newer generation)
    panel.update(PanelEvent::InputChanged("second".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let gn2 = panel.generation;

    // Routes for older generation arrive
    let old_routes = vec![make_route("old", "Old", 1.0)];
    panel.update(PanelEvent::Routes {
        generation: gn1,
        items: old_routes,
    });
    assert!(
        panel.routes.is_empty(),
        "old generation routes must be discarded"
    );

    // Routes for current generation arrive
    let new_routes = vec![make_route("new", "New", 1.0)];
    panel.update(PanelEvent::Routes {
        generation: gn2,
        items: new_routes,
    });
    assert_eq!(panel.routes.len(), 1);
    assert_eq!(panel.routes[0].title, "New");
}

// ─── Selection navigation ────────────────────────────────────────────────────

#[test]
fn selection_starts_at_zero() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::InputChanged("echo".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let gn = panel.generation;
    let routes = vec![
        make_route("a", "Alpha", 1.0),
        make_route("b", "Beta", 0.8),
        make_route("c", "Gamma", 0.6),
    ];
    panel.update(PanelEvent::Routes {
        generation: gn,
        items: routes,
    });

    assert_eq!(panel.selection.index, 0);
    assert_eq!(panel.selection.total, 3);
}

#[test]
fn arrow_down_moves_selection() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::InputChanged("test".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let gn = panel.generation;
    let routes = vec![make_route("a", "Alpha", 1.0), make_route("b", "Beta", 0.8)];
    panel.update(PanelEvent::Routes {
        generation: gn,
        items: routes,
    });

    panel.update(PanelEvent::SelectionDown);
    assert_eq!(panel.selection.index, 1);

    // Already at bottom — no-op
    panel.update(PanelEvent::SelectionDown);
    assert_eq!(panel.selection.index, 1);
}

#[test]
fn arrow_up_moves_selection() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::InputChanged("test".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let gn = panel.generation;
    let routes = vec![make_route("a", "Alpha", 1.0), make_route("b", "Beta", 0.8)];
    panel.update(PanelEvent::Routes {
        generation: gn,
        items: routes,
    });

    panel.update(PanelEvent::SelectionDown);
    assert_eq!(panel.selection.index, 1);

    panel.update(PanelEvent::SelectionUp);
    assert_eq!(panel.selection.index, 0);

    // Already at top — no-op
    panel.update(PanelEvent::SelectionUp);
    assert_eq!(panel.selection.index, 0);
}

#[test]
fn arrow_keys_safe_on_empty_routes() {
    let mut panel = PanelState::new();

    // No routes at all — arrow keys should not panic
    panel.update(PanelEvent::SelectionUp);
    assert_eq!(panel.selection.index, 0);

    panel.update(PanelEvent::SelectionDown);
    assert_eq!(panel.selection.index, 0);
}

// ─── Submit / Invoke ─────────────────────────────────────────────────────────

#[test]
fn submit_sends_invoke_for_selected_route() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    panel.update(PanelEvent::InputChanged("echo hello".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let gn = panel.generation;
    let routes = vec![make_route("flick.builtin.echo.v1.echo", "Echo", 1.0)];
    panel.update(PanelEvent::Routes {
        generation: gn,
        items: routes,
    });

    let actions = panel.update(PanelEvent::Submitted);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        PanelAction::SendToHost(UiToHost::Invoke {
            action_id, input, ..
        }) => {
            assert_eq!(action_id.as_str(), "flick.builtin.echo.v1.echo");
            assert_eq!(input.text, "echo hello");
        }
        other => panic!("expected Invoke, got {other:?}"),
    }

    // Active invocation should be set
    assert!(panel.active_invocation.is_some());
}

#[test]
fn submit_on_empty_query_is_noop() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    // No input — query is empty
    let actions = panel.update(PanelEvent::Submitted);
    assert!(actions.is_empty());
}

#[test]
fn submit_on_empty_routes_is_noop() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    panel.update(PanelEvent::InputChanged("echo hello".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    // No routes loaded
    let actions = panel.update(PanelEvent::Submitted);
    assert!(actions.is_empty());
}

#[test]
fn submit_after_selection_navigates_invokes_correct_item() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::InputChanged("test".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let gn = panel.generation;
    let routes = vec![
        make_route("first", "First", 1.0),
        make_route("second", "Second", 0.8),
    ];
    panel.update(PanelEvent::Routes {
        generation: gn,
        items: routes,
    });

    // Navigate to second item
    panel.update(PanelEvent::SelectionDown);
    assert_eq!(panel.selection.index, 1);

    let actions = panel.update(PanelEvent::Submitted);
    match &actions[0] {
        PanelAction::SendToHost(UiToHost::Invoke { action_id, .. }) => {
            assert_eq!(action_id.as_str(), "second");
        }
        other => panic!("expected Invoke for 'second', got {other:?}"),
    }
}

// ─── InvocationCompleted ─────────────────────────────────────────────────────

#[test]
fn invocation_completed_clears_active_and_stores_outcome() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::InputChanged("echo hi".into()));
    panel.update(PanelEvent::DebounceFlush {
        revision: panel.debounce_revision(),
    });
    let gn = panel.generation;
    panel.update(PanelEvent::Routes {
        generation: gn,
        items: vec![make_route("echo", "Echo", 1.0)],
    });
    let actions = panel.update(PanelEvent::Submitted);
    let completed_id = match &actions[0] {
        PanelAction::SendToHost(UiToHost::Invoke { invocation_id, .. }) => *invocation_id,
        _ => panic!(),
    };

    panel.update(PanelEvent::InvocationCompleted {
        invocation_id: completed_id,
        outcome: ActionOutcome::Data(serde_json::json!({"echo": "hi"})),
    });

    assert!(panel.active_invocation.is_none());
    assert!(panel.last_outcome.is_some());
    match panel.last_outcome.as_ref().unwrap() {
        ActionOutcome::Data(val) => assert_eq!(val["echo"], "hi"),
        _ => panic!("expected Data outcome"),
    }
}

// ─── Activation token ────────────────────────────────────────────────────────

#[test]
fn show_with_activation_token_stores_it() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: Some("portal-token-42".into()),
    });
    assert_eq!(panel.activation_token.as_deref(), Some("portal-token-42"));
    // Phase 0: token is not applied
    assert!(!panel.activation_token_applied);
}

// ─── RuntimeStatus ───────────────────────────────────────────────────────────

#[test]
fn runtime_status_is_stored() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::RuntimeStatus(RuntimeStatus::Degraded {
        reason: "portal reconnecting".into(),
    }));
    match &panel.runtime_status {
        RuntimeStatus::Degraded { reason } => assert_eq!(reason, "portal reconnecting"),
        _ => panic!("expected Degraded"),
    }
}

// ─── State-to-view layout ───────────────────────────────────────────────────

#[test]
fn result_area_is_hidden_for_the_initial_panel() {
    let panel = PanelState::new();
    assert_eq!(
        flick_shell_iced::app::results_area(&panel),
        flick_shell_iced::app::ResultsArea::Hidden
    );
    assert_eq!(
        flick_shell_iced::app::panel_window_size(&panel).height,
        flick_shell_iced::app::INPUT_ONLY_HEIGHT
    );
}

#[test]
fn result_area_becomes_visible_when_routes_arrive() {
    let mut panel = PanelState::new();
    let generation = panel.generation;
    panel.update(PanelEvent::Routes {
        generation,
        items: vec![make_route("echo", "Echo", 1.0)],
    });

    assert_eq!(
        flick_shell_iced::app::results_area(&panel),
        flick_shell_iced::app::ResultsArea::Visible
    );
    assert!(
        flick_shell_iced::app::panel_window_size(&panel).height
            > flick_shell_iced::app::INPUT_ONLY_HEIGHT
    );
}

#[test]
fn result_area_becomes_visible_for_an_invocation_outcome_without_routes() {
    let mut panel = PanelState::new();
    panel.last_outcome = Some(ActionOutcome::Notice {
        message: "Done".into(),
    });

    assert_eq!(
        flick_shell_iced::app::results_area(&panel),
        flick_shell_iced::app::ResultsArea::Visible
    );
    assert!(
        flick_shell_iced::app::panel_window_size(&panel).height
            > flick_shell_iced::app::INPUT_ONLY_HEIGHT
    );
}

// ─── Phase 0 session retention and debounce correctness ────────────────────

#[test]
fn newer_input_immediately_clears_stale_results_and_outcome() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    panel.update(PanelEvent::InputChanged("echo first".into()));
    let generation = panel.generation;
    panel.update(PanelEvent::Routes {
        generation,
        items: vec![make_route("echo", "Echo", 1.0)],
    });
    panel.last_outcome = Some(ActionOutcome::Notice {
        message: "old result".into(),
    });

    panel.update(PanelEvent::InputChanged("echo second".into()));

    assert_eq!(panel.query, "echo second");
    assert_eq!(panel.pending_query, "echo second");
    assert!(panel.routes.is_empty());
    assert_eq!(panel.selection.total, 0);
    assert!(panel.last_outcome.is_none());
}

#[test]
fn obsolete_debounce_revision_never_queries_host() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    panel.update(PanelEvent::InputChanged("ec".into()));
    let obsolete_revision = panel.debounce_revision();
    panel.update(PanelEvent::InputChanged("echo".into()));
    let current_revision = panel.debounce_revision();

    assert!(
        panel
            .update(PanelEvent::DebounceFlush {
                revision: obsolete_revision,
            })
            .is_empty()
    );

    let actions = panel.update(PanelEvent::DebounceFlush {
        revision: current_revision,
    });
    assert!(matches!(
        &actions[..],
        [PanelAction::SendToHost(UiToHost::QueryChanged { text, .. })] if text == "echo"
    ));
}

#[test]
fn hide_invalidates_pending_debounce_and_reports_close_once() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    panel.update(PanelEvent::InputChanged("echo".into()));
    let pending_revision = panel.debounce_revision();

    let first_hide = panel.update(PanelEvent::Hide);
    assert!(matches!(
        &first_hide[..],
        [PanelAction::SendToHost(UiToHost::SearchPanelClosed)]
    ));
    assert!(
        panel
            .update(PanelEvent::DebounceFlush {
                revision: pending_revision,
            })
            .is_empty()
    );
    assert!(panel.update(PanelEvent::WindowClosed).is_empty());
}

#[test]
fn hidden_panel_resumes_before_expiry_and_resets_after_expiry() {
    use std::time::{Duration, Instant};

    let start = Instant::now();
    let mut panel = PanelState::new();
    panel.update_at(
        PanelEvent::Show {
            activation_token: None,
        },
        start,
    );
    panel.update(PanelEvent::InputChanged("echo hello".into()));
    let generation_before_hide = panel.generation;
    panel.update(PanelEvent::Routes {
        generation: generation_before_hide,
        items: vec![make_route("echo", "Echo", 1.0)],
    });
    panel.last_outcome = Some(ActionOutcome::Notice {
        message: "retained".into(),
    });
    panel.update_at(PanelEvent::Hide, start + Duration::from_secs(1));

    panel.update_at(
        PanelEvent::Show {
            activation_token: None,
        },
        start + Duration::from_secs(10),
    );
    assert_eq!(panel.query, "echo hello");
    assert_eq!(panel.routes.len(), 1);
    assert!(panel.last_outcome.is_some());

    panel.update_at(PanelEvent::Hide, start + Duration::from_secs(11));
    panel.update_at(
        PanelEvent::Show {
            activation_token: None,
        },
        start + Duration::from_secs(21) + Duration::from_nanos(1),
    );

    assert!(panel.query.is_empty());
    assert!(panel.pending_query.is_empty());
    assert!(panel.routes.is_empty());
    assert_eq!(panel.selection.total, 0);
    assert!(panel.last_outcome.is_none());
    assert!(panel.generation > generation_before_hide);
}

#[test]
fn new_input_collapses_the_results_area_to_input_only_height() {
    let mut panel = PanelState::new();
    panel.update(PanelEvent::Show {
        activation_token: None,
    });
    panel.update(PanelEvent::InputChanged("echo".into()));
    let generation = panel.generation;
    panel.update(PanelEvent::Routes {
        generation,
        items: vec![make_route("echo", "Echo", 1.0)],
    });
    assert_eq!(
        flick_shell_iced::app::results_area(&panel),
        flick_shell_iced::app::ResultsArea::Visible
    );

    panel.update(PanelEvent::InputChanged("ec".into()));

    assert_eq!(
        flick_shell_iced::app::results_area(&panel),
        flick_shell_iced::app::ResultsArea::Hidden
    );
    assert_eq!(
        flick_shell_iced::app::panel_window_size(&panel).height,
        flick_shell_iced::app::INPUT_ONLY_HEIGHT
    );
}
