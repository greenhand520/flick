//! `flick-domain` — Flick 运行时的核心领域类型。
//!
//! 本 crate 包含 UI、CLI、Runtime 和 IPC 层共享的
//! 与框架无关的类型。它不依赖 Iced、Portal、D-Bus、SQLite、
//! Tokio（`ActionContext` 中的 `CancellationToken` 除外）、XDG、systemd、
//! 或 Unix Socket。

pub mod action;
pub mod caller;
pub mod error;
pub mod execution;
pub mod host;
pub mod id;
pub mod input;
pub mod paths;
pub mod plugin;
pub mod route;
pub mod settings;
pub mod surface;
pub mod ui;
pub mod usage;

/// 应用在桌面环境中的反向 DNS 标识；同时用作 portal app id、`.desktop`
/// 文件名与 GPUI 窗口的 `app_id`，三者必须一致，否则 GNOME 会显示 "null"。
pub const APP_ID: &str = "io.github.greenhand520.Flick";

// 为方便使用，在 crate 根目录重新导出关键类型。
pub use action::PluginContext;
pub use action::{
    ActionDescriptor, ActionInput, ActionOutcome, ActionRequest, ActionVisibility,
    ConcurrencyPolicy, Deadline, FileInput, FileRef, ImageData, InputType, InvocationPolicy,
    OutputType,
};
pub use caller::{Caller, CallerKind};
pub use error::ActionError;
pub use execution::{ActionSource, ExecutionEventKind, ExecutionItem};
pub use host::HostInfo;
pub use id::{ActionId, InvocationId, QueryGeneration, SurfaceSessionId};
pub use input::{
    InputContext, InputFacet, InputFacetKind, InputSource, PathLikeKind, SearchInputSnapshot,
    SearchQuery, UiInputKind,
};
pub use paths::{NoopPaths, PluginPaths};
pub use plugin::*;
pub use route::{MatchField, RouteItem, RouteItemKey, RouteItemKind};
pub use settings::{
    ConfigSnapshot, SettingChange, SettingDecl, SettingKey, SettingKind, SettingNamespace,
    SettingSchema, SettingSecretReader, SettingSnapshot, SettingValue, SettingValueError,
};
pub use surface::{
    BadgeTone, ButtonTone, ListItem, SURFACE_SCHEMA_VERSION, SpacerSize, SurfaceContent,
    SurfaceEventKind, SurfaceNode,
};
pub use ui::{HostToUi, RuntimeStatus, ThemeMode, UiIntent, UiToHost};
pub use usage::{UsageStat, UsageStore, now_secs};
