//! `flick-domain` — Flick 运行时的核心领域类型。
//!
//! 本 crate 包含 UI、CLI、Runtime 和 IPC 层共享的
//! 与框架无关的类型。它不依赖 Iced、Portal、D-Bus、SQLite、
//! Tokio（`ActionContext` 中的 `CancellationToken` 除外）、XDG、systemd、
//! 或 Unix Socket。

pub mod action;
pub mod caller;
pub mod error;
pub mod id;
pub mod input;
pub mod route;
pub mod ui;

/// 应用在桌面环境中的反向 DNS 标识；同时用作 portal app id、`.desktop`
/// 文件名与 GPUI 窗口的 `app_id`，三者必须一致，否则 GNOME 会显示 "null"。
pub const APP_ID: &str = "io.github.greenhand520.Flick";

// 为方便使用，在 crate 根目录重新导出关键类型。
pub use action::{
    ActionContext, ActionDescriptor, ActionInput, ActionOutcome, ActionRequest, ConcurrencyPolicy,
    Deadline, InputKind, Matcher,
};
pub use caller::{Caller, CallerKind};
pub use error::ActionError;
pub use id::{ActionId, InvocationId, QueryGeneration};
pub use input::SearchQuery;
pub use route::RouteItem;
pub use ui::{HostToUi, RuntimeStatus, UiIntent, UiToHost};
