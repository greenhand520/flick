use flick_domain::*;

#[test]
fn plugin_manifest_roundtrips_via_json() {
    let manifest = PluginManifest {
        id: "io.github.demo.notes".into(),
        name: "Notes".into(),
        author: "Demo Author".into(),
        version: "1.0.0".into(),
        kind: PluginKind::BuiltInRust,
        keywords: vec!["notes".into()],
        icon: None,
        profile: PluginProfile {
            description: "demo".into(),
            categories: vec![Category::Productivity],
            tags: vec!["note".into()],
            languages: vec!["zh".into()],
            execution: ExecutionMode::Local,
            data_handling: None,
        },
        permissions: vec![],
        plugin_dependencies: vec![PluginDependency {
            plugin_id: "flick.builtin.host".into(),
            requirement: DependencyRequirement::Optional,
        }],
        settings: vec![SettingDecl {
            key: "max_items".into(),
            renamed_from: None,
            label: "Max items".into(),
            description: "上限".into(),
            kind: SettingKind::Integer { min: 1, max: 100 },
            default: Some(SettingValue::Integer(20)),
            secret: false,
        }],
        surfaces: vec![UiSurfaceDecl {
            id: "main".into(),
            title: "Notes".into(),
            kind: SurfaceKind::Panel,
            default_placement: SurfacePlacement::BelowSearchBox,
        }],
        supported_os: vec![OperatingSystem::Linux, OperatingSystem::Windows],
        supported_architectures: vec![Architecture::X86_64],
        needs_search: false,
    };
    let json = serde_json::to_string(&manifest).unwrap();
    let back: PluginManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(manifest, back);
}

#[test]
fn category_is_closed_enum() {
    let cats = [
        Category::System,
        Category::Apps,
        Category::Files,
        Category::Developer,
        Category::Productivity,
        Category::Media,
        Category::Network,
        Category::Text,
        Category::Translation,
        Category::Capture,
        Category::Window,
        Category::Security,
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
        ConcurrencyPolicy::Parallel,
    );
    assert!(d.supported_inputs.is_empty());
    assert!(d.file_extensions.is_empty());
    assert_eq!(d.output_types, vec![OutputType::Data]);
    assert_eq!(d.visibility, ActionVisibility::Public);
    assert_eq!(d.invocation_policy, InvocationPolicy::NonInteractive);
}

#[test]
fn action_outcome_new_variants_roundtrip() {
    let cases = vec![
        ActionOutcome::Text("hello".into()),
        ActionOutcome::Url("https://example.com".into()),
        ActionOutcome::File(FileRef {
            uri: "file:///tmp/a.txt".into(),
            display_name: "a.txt".into(),
            mime: Some("text/plain".into()),
        }),
        ActionOutcome::OpenSurface {
            surface_id: "info".into(),
            content: flick_domain::SurfaceContent {
                schema_version: flick_domain::SURFACE_SCHEMA_VERSION,
                title: None,
                body: vec![],
            },
            session_id: None,
            needs_search: false,
        },
    ];
    for c in cases {
        let s = serde_json::to_string(&c).unwrap();
        let back: ActionOutcome = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
    }
}

#[test]
fn manifest_supports_platform_treats_empty_as_wildcard() {
    let m = PluginManifest {
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
        supported_os: vec![OperatingSystem::Linux],
        supported_architectures: vec![],
        needs_search: false,
    };
    // 架构为空 = 全部架构。
    assert!(m.supports_platform(OperatingSystem::Linux, Architecture::X86_64));
    assert!(m.supports_platform(OperatingSystem::Linux, Architecture::Aarch64));
    // 明确的 OS 列表之外不支持。
    assert!(!m.supports_platform(OperatingSystem::Windows, Architecture::X86_64));

    let all = PluginManifest {
        supported_os: vec![],
        ..m
    };
    assert!(all.supports_platform(OperatingSystem::MacOs, Architecture::Other));
}

#[test]
fn operating_system_and_architecture_serde_names() {
    assert_eq!(
        serde_json::to_string(&OperatingSystem::MacOs).unwrap(),
        "\"macos\""
    );
    assert_eq!(
        serde_json::to_string(&OperatingSystem::Linux).unwrap(),
        "\"Linux\""
    );
    assert_eq!(
        serde_json::to_string(&Architecture::X86_64).unwrap(),
        "\"x86_64\""
    );
    assert_eq!(
        serde_json::to_string(&Architecture::Aarch64).unwrap(),
        "\"aarch64\""
    );
}

#[test]
fn dependency_status_serializes_with_requirement_and_availability() {
    let status = DependencyStatus {
        plugin_id: "flick.builtin.apps".into(),
        requirement: DependencyRequirement::Required,
        available: false,
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["plugin_id"], "flick.builtin.apps");
    assert_eq!(json["requirement"], "Required");
    assert_eq!(json["available"], false);
}
