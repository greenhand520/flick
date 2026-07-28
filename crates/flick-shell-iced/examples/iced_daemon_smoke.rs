//! Iced daemon smoke test.
//!
//! Run: `cargo run -p flick-shell-iced --example iced_daemon_smoke`
//!
//! Verifies the SearchPanel state machine and IcedHostBridge work
//! together without a real desktop session. Sends synthetic events
//! and prints the resulting state transitions.
//!
//! Expected behavior:
//! - Panel starts hidden
//! - Show event makes it visible
//! - Input changes generate debounced queries
//! - Escape hides the panel
//! - Process exits with code 0

use flick_domain::*;
use flick_shell_iced::bridge::IcedHostBridge;

use flick_shell_iced::shell::IcedShell;
use flick_shell_iced::ShellMessage;
use tokio::sync::mpsc;

fn main() {
    println!("=== Iced Daemon Smoke Test ===\n");

    // Create channels
    let (_host_tx, host_rx) = mpsc::unbounded_channel::<HostToUi>();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<UiToHost>();

    let bridge = IcedHostBridge::new(host_rx);
    let mut shell = IcedShell::new(bridge, cmd_tx);

    // 1. Initial state: hidden
    println!("1. Initial state:");
    println!("   visible = {}", shell.is_visible());
    assert!(!shell.is_visible(), "panel should start hidden");

    // 2. Show panel
    println!("\n2. Show panel (via HostToUi):");
    shell.update(ShellMessage::HostEvent(HostToUi::ShowSearchPanel {
        activation_token: Some("smoke-token".into()),
    }));
    println!("   visible = {}", shell.is_visible());
    println!("   activation_token = {:?}", shell.panel.activation_token);
    assert!(shell.is_visible());
    assert_eq!(shell.panel.activation_token.as_deref(), Some("smoke-token"));

    // 3. Input changes
    println!("\n3. Input changes:");
    shell.update(ShellMessage::InputChanged("echo".into()));
    println!("   pending_query = {:?}", shell.panel.pending_query);
    println!("   debounce_active = {}", shell.is_debounce_active());
    assert_eq!(shell.panel.pending_query, "echo");
    assert!(shell.is_debounce_active());

    // 4. Debounce flush
    println!("\n4. Debounce flush:");
    shell.update(ShellMessage::DebounceFlush);
    println!("   query = {:?}", shell.panel.query);
    println!("   generation = {}", shell.panel.generation);
    assert_eq!(shell.panel.query, "echo");

    // Check that QueryChanged was sent
    let cmd = cmd_rx.try_recv().unwrap();
    match &cmd {
        UiToHost::QueryChanged { generation, text } => {
            println!("   → Sent QueryChanged(gen={generation}, text={text:?})");
        }
        other => println!("   → Sent {other:?}"),
    }

    // 5. Routes arrive
    println!("\n5. Routes arrive:");
    let gn = shell.panel.generation;
    shell.update(ShellMessage::HostEvent(HostToUi::Routes {
        generation: gn,
        items: vec![RouteItem {
            action_id: ActionId::from_static("flick.builtin.echo.v1.echo"),
            title: "Echo".into(),
            score: 1.0,
        }],
    }));
    println!("   routes = {} items", shell.panel.routes.len());
    println!("   selection = {}", shell.panel.selection.index);
    assert_eq!(shell.panel.routes.len(), 1);

    // 6. Submit
    println!("\n6. Submit (Enter):");
    shell.update(ShellMessage::Submitted);
    let cmd = cmd_rx.try_recv().unwrap();
    match &cmd {
        UiToHost::Invoke { action_id, input, .. } => {
            println!("   → Sent Invoke(action={action_id}, text={:?})", input.text);
        }
        other => println!("   → Sent {other:?}"),
    }

    // 7. Invocation completed
    println!("\n7. Invocation completed:");
    if let UiToHost::Invoke { invocation_id, .. } = &cmd {
        shell.update(ShellMessage::HostEvent(HostToUi::InvocationCompleted {
            invocation_id: *invocation_id,
            outcome: ActionOutcome::Data(serde_json::json!({"echo": "echo"})),
        }));
        println!("   last_outcome = {:?}", shell.panel.last_outcome);
        assert!(shell.panel.active_invocation.is_none());
    }

    // 8. Escape
    println!("\n8. Escape pressed:");
    shell.update(ShellMessage::EscapePressed);
    println!("   visible = {}", shell.is_visible());
    assert!(!shell.is_visible());

    // Check SearchPanelClosed was sent
    let cmd = cmd_rx.try_recv().unwrap();
    match &cmd {
        UiToHost::SearchPanelClosed => println!("   → Sent SearchPanelClosed"),
        other => println!("   → Sent {other:?}"),
    }

    println!("\n=== Smoke test passed ===");
}
