//! `flick-shell-iced` — Iced-based shell for Flick.
//!
//! Phase 0: minimal search panel with Iced daemon mode.
//! - No window on startup; created on ShowSearchPanel
//! - Text input with generation tracking
//! - Route item list with selection
//! - Invocation result display
//! - Esc/window-close hides (daemon keeps running)

pub mod app;
pub mod bridge;
pub mod message;
pub mod panel;
pub mod shell;

pub use app::AppState;
pub use bridge::IcedHostBridge;
pub use message::ShellMessage;
pub use panel::{PanelAction, PanelEvent, PanelState, SelectionState};
pub use shell::{IcedShell, WindowRole};
