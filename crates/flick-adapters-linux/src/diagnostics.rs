//! Session environment diagnostics: probe Wayland, Portal, and runtime conditions.

use serde::{Deserialize, Serialize};

/// Full session diagnostics snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDiagnostics {
    pub session_type: SessionType,
    pub wayland_display: Presence,
    pub runtime_dir: Presence,
    pub session_bus: Presence,
    pub current_desktop: ValueOrMissing,
    pub portal_global_shortcuts: PortalCapability,
    pub portal_backend: ValueOrMissing,
    pub shortcut_state: String,
    pub shortcut_count: u32,
    pub portal_session_age: Option<String>,
    pub last_portal_error: Option<String>,
    pub reconnect_attempt: Option<u32>,
    pub next_reconnect_at: Option<String>,
    pub external_fallback: ExternalFallback,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PortalCapability {
    Available { version: u32 },
    Unavailable { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExternalFallback {
    CommandAvailable,
    NotConfigured,
    Unknown,
}

/// Probe the current session environment.
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
        portal_global_shortcuts: PortalCapability::Unavailable {
            reason: "not probed".into(),
        },
        portal_backend: ValueOrMissing::Missing,
        shortcut_state: "unknown".into(),
        shortcut_count: 0,
        portal_session_age: None,
        last_portal_error: None,
        reconnect_attempt: None,
        next_reconnect_at: None,
        external_fallback: ExternalFallback::Unknown,
    }
}

/// Check if we're in a valid Wayland session for graphical mode.
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
