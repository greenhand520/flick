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

#[test]
fn action_descriptor_new_fills_phase1_defaults() {
    let d = flick_domain::ActionDescriptor::new(
        ActionId::from_static("flick.builtin.host.show_info"),
        "系统信息",
        vec!["host".into(), "系统".into()],
        Matcher::prefix_command("host"),
        ConcurrencyPolicy::Parallel,
    );
    assert_eq!(d.supported_inputs, vec![InputType::PlainText]);
    assert!(d.file_extensions.is_empty());
    assert_eq!(d.output_types, vec![OutputType::Data]);
    assert_eq!(d.visibility, ActionVisibility::Public);
    assert_eq!(d.risk_level, RiskLevel::ReadOnly);
    assert_eq!(d.invocation_policy, InvocationPolicy::NonInteractive);
}

#[test]
fn action_outcome_new_variants_roundtrip() {
    let cases = vec![
        ActionOutcome::Text("hello".into()),
        ActionOutcome::Url("https://example.com".into()),
        ActionOutcome::File(FileRef { uri: "file:///tmp/a.txt".into(), display_name: "a.txt".into(), mime: Some("text/plain".into()) }),
        ActionOutcome::OpenSurface { surface_id: "info".into(), args: serde_json::json!({"k": 1}) },
    ];
    for c in cases {
        let s = serde_json::to_string(&c).unwrap();
        let back: ActionOutcome = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
    }
}
