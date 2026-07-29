//! `flick-shell-iced` — Iced-based shell for Flick.
//!
//! Phase 0 owns one Iced bridge model:
//! - no window on startup;
//! - host events open/close the search panel;
//! - Esc/window-close hide the panel while the daemon stays alive;
//! - daemon shutdown exits Iced even while the panel is hidden.

pub mod app;
pub mod panel;

pub use app::AppState;
pub use panel::{PanelAction, PanelEvent, PanelState, SelectionState};
