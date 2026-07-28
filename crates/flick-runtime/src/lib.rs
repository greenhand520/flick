//! `flick-runtime` — Action routing, invocation, and lifecycle management.
//!
//! This crate provides the runtime layer for Flick's action system.
//! It is framework-agnostic: it does not depend on Iced, Portal,
//! D-Bus, or Unix Socket.
//!
//! # Key types
//!
//! * [`ActionHandler`] — trait that each registered Action implements.
//! * [`ActionRegistry`] — stores descriptors + handlers; supports
//!   keyword and prefix-command search.
//! * [`InvocationHandle`] — handle to a running invocation with
//!   cooperative cancellation.
//! * [`ActionRuntime`] — top-level orchestrator: routes queries,
//!   invokes actions, manages deadlines and cancellation.

pub mod handler;
pub mod invocation;
pub mod registry;
pub mod runtime;

// Re-export the primary public API at crate root.
pub use handler::ActionHandler;
pub use invocation::InvocationHandle;
pub use registry::ActionRegistry;
pub use runtime::ActionRuntime;
