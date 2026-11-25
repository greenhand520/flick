use flick_domain::*;

#[test]
fn plugin_manifest_roundtrips_via_json() {
    let manifest = PluginManifest {
        id: "io.github.demo.notes".into(),
        name: "Notes".into(),
        version: "1.0.0".into(),
        kind: PluginKind::BuiltInRust,
        keywords: vec!["notes".into()],
        profile: PluginProfile {
            description: "demo".into(),
            categories: vec![Category::Productivity],
            tags: vec!["note".into()],
            languages: vec!["zh".into()],
            execution: ExecutionMode::Local,
            data_handling: None,
        },
        capabilities: vec![],
        plugin_dependencies: vec![PluginDependency {
            plugin_id: "flick.builtin.host".into(),
            requirement: DependencyRequirement::Optional,
        }],
        settings: vec![SettingDecl {
            key: "max_items".into(),
            label: "Max items".into(),
            description: "上限".into(),
            kind: SettingKind::Integer { min: 1, max: 100 },
            default: Some(serde_json::json!(20)),
            secret: false,
        }],
        surfaces: vec![UiSurfaceDecl {
            id: "main".into(),
            title: "Notes".into(),
            kind: SurfaceKind::Panel,
            default_placement: SurfacePlacement::BelowSearchBox,
        }],
    };
    let json = serde_json::to_string(&manifest).unwrap();
    let back: PluginManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(manifest, back);
}

#[test]
fn category_is_closed_enum() {
    let cats = [
        Category::System, Category::Apps, Category::Files, Category::Developer,
        Category::Productivity, Category::Media, Category::Network, Category::Text,
        Category::Translation, Category::Capture, Category::Window, Category::Security,
        Category::Other,
    ];
    assert_eq!(cats.len(), 13);
}
