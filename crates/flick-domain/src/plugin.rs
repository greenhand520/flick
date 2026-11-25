//! 插件元数据（纯数据、可序列化；UI 与外壳可复用）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PluginKind {
    #[default]
    BuiltInRust,
    Wasm,
}

/// 插件可运行的操作系统。
///
/// 固定为四个值：Linux / Windows / macOS / Other（其他系统统一归入 `Other`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum OperatingSystem {
    Linux,
    Windows,
    #[serde(rename = "macos")]
    MacOs,
    #[default]
    Other,
}

/// 处理器架构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Architecture {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "aarch64")]
    Aarch64,
    #[default]
    Other,
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
    /// 支持的操作系统；空列表表示"全部平台"（如纯逻辑插件）。
    pub supported_os: Vec<OperatingSystem>,
    /// 支持的处理器架构；空列表表示"全部架构"。
    pub supported_architectures: Vec<Architecture>,
}

impl PluginManifest {
    /// 该插件是否声明支持给定平台。
    ///
    /// 语义与 `file_extensions` 一致：**空列表 = 不限**。
    pub fn supports_platform(&self, os: OperatingSystem, arch: Architecture) -> bool {
        let os_ok = self.supported_os.is_empty() || self.supported_os.contains(&os);
        let arch_ok = self.supported_architectures.is_empty()
            || self.supported_architectures.contains(&arch);
        os_ok && arch_ok
    }
}
