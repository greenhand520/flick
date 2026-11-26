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
    System,
    Apps,
    Files,
    Developer,
    Productivity,
    Media,
    Network,
    Text,
    Translation,
    Capture,
    Window,
    Security,
    Other,
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
    /// 插件作者（必填；建议使用可展示的单一作者名）。
    pub author: String,
    pub version: String,
    pub kind: PluginKind,
    pub keywords: Vec<String>,
    /// 插件图标。内置 Rust 插件可填内嵌 SVG 或相对可执行文件的图片路径；
    /// 推荐 SVG 或 `.svg` 文件，可省略。WASM 插件的图标规则后续定义。
    #[serde(default)]
    pub icon: Option<String>,
    pub profile: PluginProfile,
    /// 插件声明的权限。当前仅作声明，具体鉴权由宿主执行。
    #[serde(default)]
    pub permissions: Vec<String>,
    pub plugin_dependencies: Vec<PluginDependency>,
    pub settings: Vec<SettingDecl>,
    pub surfaces: Vec<UiSurfaceDecl>,
    /// 支持的操作系统；空列表表示"全部平台"（如纯逻辑插件）。
    pub supported_os: Vec<OperatingSystem>,
    /// 支持的处理器架构；空列表表示"全部架构"。
    pub supported_architectures: Vec<Architecture>,
    /// 该插件的 Surface 是否需要宿主搜索框（全局，默认 false）。
    #[serde(default)]
    pub needs_search: bool,
}

impl PluginManifest {
    /// 该插件是否声明支持给定平台。
    ///
    /// 语义与 `file_extensions` 一致：**空列表 = 不限**。
    pub fn supports_platform(&self, os: OperatingSystem, arch: Architecture) -> bool {
        let os_ok = self.supported_os.is_empty() || self.supported_os.contains(&os);
        let arch_ok =
            self.supported_architectures.is_empty() || self.supported_architectures.contains(&arch);
        os_ok && arch_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PluginManifest {
        PluginManifest {
            id: "demo.plugin".into(),
            name: "Demo".into(),
            author: "Demo Author".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            keywords: vec![],
            icon: None,
            profile: PluginProfile::default(),
            permissions: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: true,
        }
    }

    #[test]
    fn needs_search_defaults_to_false_when_missing() {
        let value = serde_json::to_value(sample()).unwrap();
        let mut object = value.as_object().unwrap().clone();
        object.remove("needs_search");
        let back: PluginManifest =
            serde_json::from_value(serde_json::Value::Object(object)).unwrap();
        assert!(!back.needs_search);
    }

    #[test]
    fn icon_and_permissions_default_when_missing() {
        let value = serde_json::to_value(sample()).unwrap();
        let mut object = value.as_object().unwrap().clone();
        object.remove("icon");
        object.remove("permissions");
        let back: PluginManifest =
            serde_json::from_value(serde_json::Value::Object(object)).unwrap();
        assert!(back.icon.is_none());
        assert!(back.permissions.is_empty());
    }

    #[test]
    fn needs_search_round_trips() {
        let m = sample();
        let json = serde_json::to_string(&m).unwrap();
        let back: PluginManifest = serde_json::from_str(&json).unwrap();
        assert!(back.needs_search);
        assert_eq!(m, back);
    }
}
