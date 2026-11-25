//! 插件元数据（纯数据、可序列化；UI 与外壳可复用）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PluginKind {
    #[default]
    BuiltInRust,
    Wasm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    System, Apps, Files, Developer, Productivity, Media, Network,
    Text, Translation, Capture, Window, Security, Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExecutionMode {
    #[default]
    Local,
    Remote,
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginProfile {
    pub description: String,
    pub categories: Vec<Category>,
    pub tags: Vec<String>,
    pub languages: Vec<String>,
    pub execution: ExecutionMode,
    pub data_handling: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyRequirement {
    Required,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDependency {
    pub plugin_id: String,
    pub requirement: DependencyRequirement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingKind {
    Bool,
    Integer { min: i64, max: i64 },
    String,
    Enum { variants: Vec<String> },
    Path,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingDecl {
    pub key: String,
    pub label: String,
    pub description: String,
    pub kind: SettingKind,
    pub default: Option<serde_json::Value>,
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceKind {
    Panel,
    Overlay,
    Dialog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SurfacePlacement {
    #[default]
    BelowSearchBox,
    DetachedWindow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSurfaceDecl {
    pub id: String,
    pub title: String,
    pub kind: SurfaceKind,
    pub default_placement: SurfacePlacement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: PluginKind,
    pub keywords: Vec<String>,
    pub profile: PluginProfile,
    pub capabilities: Vec<String>,
    pub plugin_dependencies: Vec<PluginDependency>,
    pub settings: Vec<SettingDecl>,
    pub surfaces: Vec<UiSurfaceDecl>,
}
