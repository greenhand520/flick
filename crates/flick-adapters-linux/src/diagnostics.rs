//! 会话环境诊断：探测 Wayland、Portal 和运行时条件。

use serde::{Deserialize, Serialize};

/// 无需打开桌面 Portal 会话即可收集的会话相关信息。
///
/// Portal 状态属于 [`crate::portal::PortalShortcutAdapter`]，并由守护进程的
/// 诊断单独报告。将探测限定在环境相关信息上，可避免呈现过期或模拟的
/// Portal 信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDiagnostics {
    pub session_type: SessionType,
    pub wayland_display: Presence,
    pub runtime_dir: Presence,
    pub session_bus: Presence,
    pub current_desktop: ValueOrMissing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionType {
    Wayland,
    X11,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Presence {
    Present,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValueOrMissing {
    Value(String),
    Missing,
}

/// 探测当前会话环境。
pub fn probe_session() -> SessionDiagnostics {
    let session_type = match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => SessionType::Wayland,
        Ok("x11") => SessionType::X11,
        _ => SessionType::Unknown,
    };

    let wayland_display = env_presence("WAYLAND_DISPLAY");
    let runtime_dir = env_presence("XDG_RUNTIME_DIR");
    let session_bus = env_presence("DBUS_SESSION_BUS_ADDRESS");
    let current_desktop = env_value_or_missing("XDG_CURRENT_DESKTOP");

    SessionDiagnostics {
        session_type,
        wayland_display,
        runtime_dir,
        session_bus,
        current_desktop,
    }
}

/// 检查当前是否处于适用于图形模式的有效 Wayland 会话。
pub fn is_wayland_session() -> bool {
    matches!(std::env::var("XDG_SESSION_TYPE").as_deref(), Ok("wayland"))
        && std::env::var("WAYLAND_DISPLAY").is_ok()
}

fn env_presence(var: &str) -> Presence {
    if std::env::var(var).is_ok() {
        Presence::Present
    } else {
        Presence::Missing
    }
}

fn env_value_or_missing(var: &str) -> ValueOrMissing {
    match std::env::var(var) {
        Ok(v) => ValueOrMissing::Value(v),
        Err(_) => ValueOrMissing::Missing,
    }
}
