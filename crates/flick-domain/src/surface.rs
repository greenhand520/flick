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
    /// 交互节点：单行文本输入。`id` 在同一个 `SurfaceContent` 内唯一。
    Input { id: String, value: String, #[serde(default, skip_serializing_if = "Option::is_none")] placeholder: Option<String> },
    /// 交互节点：多行文本输入。
    TextArea { id: String, value: String, #[serde(default, skip_serializing_if = "Option::is_none")] placeholder: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] rows: Option<u16> },
    /// 交互节点：按钮。
    Button { id: String, label: String, #[serde(default)] tone: ButtonTone },
    /// 交互节点：复选框。
    Checkbox { id: String, checked: bool, label: String },
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

/// Button 的语义色调。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonTone {
    #[default]
    Neutral,
    Primary,
    Danger,
}

/// Surface 交互事件；由 UI 发往插件（经宿主转发）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceEventKind {
    /// 宿主搜索框文本变化（`needs_search` 插件）。
    SearchChanged { text: String },
    /// 交互节点文本变化。
    TextChanged { node_id: String, text: String },
    /// 交互节点提交（输入框回车）。
    Submitted { node_id: String, text: String },
    /// 交互节点被点击。
    Click { node_id: String },
    /// 复选框切换。
    Toggled { node_id: String, checked: bool },
    /// 预留，v1 不发送。
    ContextMenu { node_id: String, x: f32, y: f32 },
}

/// List 中的一项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_nodes_round_trip_json() {
        let nodes = vec![
            SurfaceNode::Input {
                id: "search".into(),
                value: "abc".into(),
                placeholder: Some("搜索".into()),
            },
            SurfaceNode::TextArea {
                id: "body".into(),
                value: "x".into(),
                placeholder: None,
                rows: Some(4),
            },
            SurfaceNode::Button {
                id: "save".into(),
                label: "保存".into(),
                tone: ButtonTone::Primary,
            },
            SurfaceNode::Checkbox {
                id: "remember".into(),
                checked: true,
                label: "记住".into(),
            },
        ];
        for node in nodes {
            let json = serde_json::to_string(&node).unwrap();
            let back: SurfaceNode = serde_json::from_str(&json).unwrap();
            assert_eq!(node, back);
        }
    }

    #[test]
    fn list_item_id_round_trips() {
        let item = ListItem {
            id: Some("row1".into()),
            title: "t".into(),
            subtitle: None,
            value: None,
            badge: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: ListItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item, back);

        // 旧数据无 id 字段也要能解码。
        let legacy = serde_json::json!({ "title": "t" });
        let back: ListItem = serde_json::from_value(legacy).unwrap();
        assert!(back.id.is_none());
    }

    #[test]
    fn surface_event_kind_round_trips() {
        let events = vec![
            SurfaceEventKind::SearchChanged { text: "q".into() },
            SurfaceEventKind::TextChanged { node_id: "n".into(), text: "v".into() },
            SurfaceEventKind::Submitted { node_id: "n".into(), text: "v".into() },
            SurfaceEventKind::Click { node_id: "n".into() },
            SurfaceEventKind::Toggled { node_id: "n".into(), checked: true },
            SurfaceEventKind::ContextMenu { node_id: "n".into(), x: 1.0, y: 2.0 },
        ];
        for e in events {
            let json = serde_json::to_string(&e).unwrap();
            let back: SurfaceEventKind = serde_json::from_str(&json).unwrap();
            assert_eq!(e, back);
        }
    }

    #[test]
    fn button_tone_defaults_to_neutral() {
        assert_eq!(ButtonTone::default(), ButtonTone::Neutral);
    }
}
