//! `flick-domain` — Core domain types for the Flick runtime.
//!
//! This crate contains framework-agnostic types shared by UI, CLI, Runtime,
//! and IPC layers. It has no dependency on Iced, Portal, D-Bus, SQLite,
//! Tokio (except `CancellationToken` in `ActionContext`), XDG, systemd,
//! or Unix Socket.

pub mod action;
pub mod caller;
pub mod error;
pub mod id;
pub mod input;
pub mod route;
pub mod ui;

// Re-export key types at crate root for convenience.
pub use action::{
    ActionContext, ActionDescriptor, ActionInput, ActionOutcome, ActionRequest,
    ConcurrencyPolicy, Deadline, InputKind, Matcher,
};
pub use caller::{Caller, CallerKind};
pub use error::ActionError;
pub use id::{ActionId, InvocationId, QueryGeneration};
pub use input::SearchQuery;
pub use route::RouteItem;
pub use ui::{HostToUi, RuntimeStatus, UiIntent, UiToHost};
