//! Iced daemon bridge smoke test.
//!
//! Run: `cargo run -p flick-shell-iced --example iced_daemon_smoke`
//!
//! This checks the same `app.rs` bridge used by the graphical daemon without
//! opening an Iced window or requiring a desktop session.

use flick_domain::*;
use flick_shell_iced::app::{self, AppMessage};
use tokio::sync::mpsc;

fn main() {
    println!("=== Iced Daemon Smoke Test ===\n");

    let (_host_tx, host_rx) = mpsc::unbounded_channel::<HostToUi>();
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiToHost>();
    let (mut state, _) = app::boot(ui_tx, host_rx);

    println!("1. Initial state: visible = {}", state.panel.visible);
    assert!(!state.panel.visible);

    let _ = app::update(
        &mut state,
        AppMessage::HostEvent(HostToUi::ShowSearchPanel {
            activation_token: Some("smoke-token".into()),
        }),
    );
    println!("2. Show: visible = {}", state.panel.visible);
    assert!(state.panel.visible);

    let _ = app::update(&mut state, AppMessage::InputChanged("echo".into()));
    let _ = app::update(&mut state, AppMessage::DebounceFlush);
    let command = ui_rx.try_recv().expect("query command");
    assert!(matches!(command, UiToHost::QueryChanged { .. }));

    let generation = state.panel.generation;
    let _ = app::update(
        &mut state,
        AppMessage::HostEvent(HostToUi::Routes {
            generation,
            items: vec![RouteItem {
                action_id: ActionId::from_static("flick.builtin.echo.v1.echo"),
                title: "Echo".into(),
                score: 1.0,
            }],
        }),
    );
    let _ = app::update(&mut state, AppMessage::Submitted);
    let command = ui_rx.try_recv().expect("invoke command");
    assert!(matches!(command, UiToHost::Invoke { .. }));

    let _ = app::update(&mut state, AppMessage::EscapePressed);
    assert!(!state.panel.visible);
    assert!(matches!(ui_rx.try_recv(), Ok(UiToHost::SearchPanelClosed)));

    println!("=== Smoke test passed ===");
}
