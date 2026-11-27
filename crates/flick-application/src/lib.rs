//! `flick-application` —— Flick 启动器的应用层。
//!
//! 此 crate 包含**协调器**，它桥接 UI 事件（来自 UI/Portal）与
//! Flick Runtime。它定义了窄端口 trait（`RuntimePort`、`HostEventSink`），
//! 由适配器实现，从而使应用层与框架无关。
//!
//! # 架构
//!
//! ```text
//!   UI (UI / Portal) ──► ApplicationCoordinator ──► Runtime
//!                              │
//!                              └──► HostEventSink ──► UI
//! ```
//!
//! 协调器是路由 / 调用逻辑的唯一所有者。
//! UI 外壳和 CLI 适配器只是转发消息的薄封装。

pub mod accumulate;
pub mod coordinator;
pub mod ports;
pub mod presenter;
pub mod query;
pub mod usage;

pub use accumulate::{MAX_ACCUMULATED_RESULTS, SearchSession, merge_batch, merge_execution_batch};
pub use coordinator::ApplicationCoordinator;
pub use ports::{HostEventSink, RuntimePort};
pub use presenter::{PanelAction, PanelEvent, PanelState, SelectionState};
pub use query::QueryTracker;
pub use usage::InMemoryUsageStore;
