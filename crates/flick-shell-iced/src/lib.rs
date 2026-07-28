//! Iced-based shell for Flick: search panel UI.
//!
//! Phase 0 provides a minimal search panel.
//! Note: Full Iced integration requires runtime wiring in the daemon;
//! this crate provides the types and state management.

pub mod panel;
pub use panel::{SearchPanel, Message};
