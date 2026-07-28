//! `flick-application` — Application layer for the Flick launcher.
//!
//! This crate contains the **coordinator** that bridges UI events (from
//! Iced/Portal) and the Flick Runtime. It defines narrow port traits
//! (`RuntimePort`, `HostEventSink`) that adapters implement, keeping
//! the application layer framework-agnostic.
//!
//! # Architecture
//!
//! ```text
//!   UI (Iced / Portal) ──► ApplicationCoordinator ──► Runtime
//!                              │
//!                              └──► HostEventSink ──► UI
//! ```
//!
//! The coordinator is the single owner of routing / invocation logic.
//! UI shells and CLI adapters are thin wrappers that forward messages.

pub mod coordinator;
pub mod ports;
pub mod query;

pub use coordinator::ApplicationCoordinator;
pub use ports::{HostEventSink, RuntimePort};
pub use query::QueryTracker;
