//! Surface 的样式与渲染。模型在 `flick-domain`，本 crate 只做呈现。
//!
//! `gpui` feature 默认关闭：任何不渲染的 crate（如 `flick-builtins`）都不应拉入 gpui。

pub mod style;

#[cfg(feature = "gpui")]
pub mod render;

pub use flick_domain::surface::*;
pub use style::{estimate_height, SurfaceStyle};
