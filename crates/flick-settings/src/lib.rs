//! `flick-settings` — 设置业务层。
//!
//! 在 [`flick_data::StorageManager`] 之上提供：schema 注册与指纹短路、读写与
//! 默认值解析、revision 递增与变更订阅（见 `设置系统设计与约束.md` §8）。

pub mod core_schema;
pub mod error;
pub mod service;

pub use core_schema::{CORE_SCHEMA_VERSION, core_schema};
pub use error::SettingsError;
pub use service::{RegisterOutcome, SettingsService};
