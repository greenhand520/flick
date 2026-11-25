//! 数据驱动的 Surface 内容模型（与 UI 框架无关）。
//!
//! 插件只产出本模块的数据结构；由宿主（`flick-ui` + `flick-shell-gpui`）负责渲染。
//! 进程内为强类型零序列化；跨 IPC 时经 MessagePack 编解码。

use serde::{Deserialize, Serialize};

/// 当前 Surface 协议版本。
pub const SURFACE_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    SURFACE_SCHEMA_VERSION
}

/// 一个 Surface 的完整内容（标题 + 节点树）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceContent {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Vec<SurfaceNode>,
}

/// Surface 节点。每个变体自带可选 `id`（为未来事件协议预留，Plan 2 不消费）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceNode {
    Section { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] title: Option<String>, #[serde(default)] children: Vec<SurfaceNode> },
    Row { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, #[serde(default)] children: Vec<SurfaceNode> },
    Column { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, #[serde(default)] children: Vec<SurfaceNode> },
    Heading { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, text: String },
    Text { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, text: String, #[serde(default)] muted: bool },
    KeyValue { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, key: String, value: String },
    Badge { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, text: String, #[serde(default)] tone: BadgeTone },
    List { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, #[serde(default)] items: Vec<ListItem> },
    Divider { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String> },
    Spacer { #[serde(default, skip_serializing_if = "Option::is_none")] id: Option<String>, #[serde(default)] size: SpacerSize },
}

/// Badge 的语义色调。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadgeTone {
    #[default]
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

/// Spacer 的尺寸档位。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpacerSize {
    #[default]
    Sm,
    Md,
    Lg,
}

/// List 中的一项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
}
