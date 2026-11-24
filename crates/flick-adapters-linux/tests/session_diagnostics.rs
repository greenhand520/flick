//! 会话诊断测试。

use flick_adapters_linux::diagnostics::{SessionType, probe_session};

#[test]
fn probe_detects_session_type() {
    let diag = probe_session();
    // 在不知道测试环境的情况下，我们无法断言 Wayland/X11，
    // 但可以验证诊断结构是否格式正确。
    match &diag.session_type {
        SessionType::Wayland | SessionType::X11 | SessionType::Unknown => {}
    }
}

#[test]
fn probe_reports_environment_variables() {
    let diag = probe_session();
    // 测试环境中可能存在也可能不存在 WAYLAND_DISPLAY 和 XDG_RUNTIME_DIR，
    // 只需验证该结构不会引发 panic。
    let _ = format!("{diag:?}");
}
