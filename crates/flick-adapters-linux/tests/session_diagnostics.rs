//! Session diagnostics tests.

use flick_adapters_linux::diagnostics::{probe_session, SessionType};

#[test]
fn probe_detects_session_type() {
    let diag = probe_session();
    // We can't assert Wayland/X11 without knowing the test environment,
    // but we can verify the diagnostic structure is well-formed.
    match &diag.session_type {
        SessionType::Wayland | SessionType::X11 | SessionType::Unknown => {}
    }
}

#[test]
fn probe_reports_environment_variables() {
    let diag = probe_session();
    // WAYLAND_DISPLAY and XDG_RUNTIME_DIR may or may not be present
    // in the test environment; just verify the structure doesn't panic.
    let _ = format!("{diag:?}");
}
