//! 设置类型的契约测试：值校验矩阵、全限定键解析、默认值回退。

use flick_domain::{
    SettingDecl, SettingKey, SettingKind, SettingNamespace, SettingSchema, SettingValue,
    SettingValueError,
};

#[test]
fn validate_covers_every_kind() {
    let cases: Vec<(SettingKind, SettingValue, bool)> = vec![
        (SettingKind::Bool, SettingValue::Bool(true), true),
        (SettingKind::Bool, SettingValue::Integer(1), false),
        (
            SettingKind::Integer { min: 0, max: 10 },
            SettingValue::Integer(10),
            true,
        ),
        (
            SettingKind::Integer { min: 0, max: 10 },
            SettingValue::Integer(11),
            false,
        ),
        (SettingKind::String, SettingValue::String("x".into()), true),
        (
            SettingKind::Enum {
                variants: vec!["dark".into(), "light".into()],
            },
            SettingValue::Enum("dark".into()),
            true,
        ),
        (
            SettingKind::Enum {
                variants: vec!["dark".into()],
            },
            SettingValue::Enum("solarized".into()),
            false,
        ),
        (
            SettingKind::Path,
            SettingValue::Path("file:///tmp/a".into()),
            true,
        ),
        (
            SettingKind::Json,
            SettingValue::Json(serde_json::json!({"a": 1})),
            true,
        ),
    ];
    for (kind, value, ok) in cases {
        assert_eq!(kind.validate(&value).is_ok(), ok, "{kind:?} / {value:?}");
    }
}

#[test]
fn fallback_values_follow_design_table() {
    assert_eq!(SettingKind::Bool.fallback_value(), SettingValue::Bool(false));
    assert_eq!(
        SettingKind::Integer { min: 3, max: 9 }.fallback_value(),
        SettingValue::Integer(3)
    );
    assert_eq!(
        SettingKind::String.fallback_value(),
        SettingValue::String(String::new())
    );
    assert_eq!(
        SettingKind::Enum {
            variants: vec!["b".into(), "a".into()]
        }
        .fallback_value(),
        SettingValue::Enum("b".into())
    );
    assert_eq!(
        SettingKind::Path.fallback_value(),
        SettingValue::Path(String::new())
    );
    assert_eq!(
        SettingKind::Json.fallback_value(),
        SettingValue::Json(serde_json::Value::Null)
    );
}

#[test]
fn qualified_key_round_trips() {
    let core = SettingKey::parse("core.shortcut.show_search").expect("core key");
    assert_eq!(core.namespace, SettingNamespace::Core);
    assert_eq!(core.key, "shortcut.show_search");
    assert_eq!(core.qualified(), "core.shortcut.show_search");

    let plugin = SettingKey::parse("plugin.flick.builtin.apps.max_recent").expect("plugin key");
    assert_eq!(
        plugin.namespace,
        SettingNamespace::Plugin("flick.builtin.apps".into())
    );
    assert_eq!(plugin.key, "max_recent");
    assert_eq!(plugin.qualified(), "plugin.flick.builtin.apps.max_recent");
}

#[test]
fn parse_rejects_bad_prefixed_and_injected_keys() {
    // 注意：`plugin.flick.builtin.apps` 有歧义（可读作 plugin=`flick.builtin`、
    // key=`apps`），按 rsplit 规则视为合法，故不列入非法集合。
    for bad in [
        "max_recent",
        "core.",
        "plugin.flick.builtin.apps.",
        "../etc/passwd",
        "core..x",
    ] {
        assert!(
            SettingKey::parse(bad).is_err(),
            "expected parse failure for {bad}"
        );
    }
    assert!(matches!(
        SettingKey::parse("plugin.../../x.y"),
        Err(SettingValueError::InvalidNamespace { .. }) | Err(SettingValueError::InvalidKey { .. })
    ));
}

#[test]
fn schema_validation_rules() {
    let decl = |key: &str| SettingDecl {
        key: key.to_string(),
        renamed_from: None,
        label: String::new(),
        description: String::new(),
        kind: SettingKind::Bool,
        default: None,
        secret: false,
    };

    // 插件命名空间：短 key 不得含 `.`
    let dotted = SettingSchema {
        namespace: SettingNamespace::plugin("flick.demo"),
        declarations: vec![decl("a.b")],
        schema_version: 1,
    };
    assert!(dotted.validate().is_err());

    // 插件命名空间：不得使用保留前缀
    let reserved = SettingSchema {
        namespace: SettingNamespace::plugin("flick.demo"),
        declarations: vec![decl("core")],
        schema_version: 1,
    };
    assert!(matches!(
        reserved.validate(),
        Err(SettingValueError::ReservedPrefix { .. })
    ));

    // 重复短 key
    let duplicate = SettingSchema {
        namespace: SettingNamespace::plugin("flick.demo"),
        declarations: vec![decl("same"), decl("same")],
        schema_version: 1,
    };
    assert!(matches!(
        duplicate.validate(),
        Err(SettingValueError::DuplicateKey { .. })
    ));

    // core 命名空间的 key 允许含 `.`
    let core = SettingSchema {
        namespace: SettingNamespace::Core,
        declarations: vec![decl("shortcut.show_search")],
        schema_version: 1,
    };
    assert!(core.validate().is_ok());
}

#[test]
fn declaration_rejects_default_of_wrong_kind() {
    let decl = SettingDecl {
        key: "limit".into(),
        renamed_from: None,
        label: String::new(),
        description: String::new(),
        kind: SettingKind::Integer { min: 0, max: 10 },
        default: Some(SettingValue::String("ten".into())),
        secret: false,
    };
    assert!(matches!(
        decl.validate(),
        Err(SettingValueError::DefaultKindMismatch { .. })
    ));
}

#[test]
fn empty_enum_and_bad_range_are_rejected() {
    assert_eq!(
        SettingKind::Enum { variants: vec![] }.validate_declaration(),
        Err(SettingValueError::EmptyVariants)
    );
    assert_eq!(
        SettingKind::Integer { min: 5, max: 1 }.validate_declaration(),
        Err(SettingValueError::InvalidRange { min: 5, max: 1 })
    );
}

#[test]
fn plugin_id_rules_block_path_escapes() {
    assert!(flick_domain::settings::is_valid_plugin_id("flick.builtin.apps"));
    for bad in ["", ".", "..", "core", "/abs", "a/b", "-lead", "a b"] {
        assert!(!flick_domain::settings::is_valid_plugin_id(bad), "{bad}");
    }
}

// ─── S1-A：ConfigSnapshot 与 PluginContext ────────────────────────────────

use flick_domain::{ConfigSnapshot, PluginContext};
use tokio_util::sync::CancellationToken;

fn snapshot() -> ConfigSnapshot {
    let mut values = std::collections::BTreeMap::new();
    values.insert("enabled".to_string(), SettingValue::Bool(true));
    values.insert("limit".to_string(), SettingValue::Integer(7));
    values.insert("mode".to_string(), SettingValue::Enum("light".to_string()));
    values.insert("name".to_string(), SettingValue::String("flick".to_string()));
    ConfigSnapshot {
        namespace: "flick.demo".to_string(),
        revision: 5,
        values,
    }
}

#[test]
fn config_snapshot_typed_getters() {
    let s = snapshot();
    assert_eq!(s.revision(), 5);
    assert!(!s.is_empty());
    assert_eq!(s.bool("enabled"), Some(true));
    assert_eq!(s.i64("limit"), Some(7));
    assert_eq!(s.str("mode"), Some("light"));
    assert_eq!(s.str("name"), Some("flick"));
    assert!(s.is_enum("mode", "light"));
    assert!(!s.is_enum("mode", "dark"));
    assert_eq!(s.i64("missing"), None);
    assert!(s.value("enabled").is_some());
}

#[test]
fn empty_snapshot_is_degraded_default() {
    let empty = ConfigSnapshot::empty();
    assert!(empty.is_empty());
    assert_eq!(empty.revision(), 0);
    assert_eq!(empty.bool("enabled"), None);
}

#[test]
fn plugin_context_has_no_secret_reader_by_default() {
    let ctx = PluginContext::new(CancellationToken::new())
        .with_settings(snapshot())
        .with_plugin_id(Some("flick.demo".to_string()));
    assert_eq!(ctx.plugin_id(), Some("flick.demo"));
    assert_eq!(ctx.settings().revision(), 5);
    // 没有 secret 读取器时安全返回 None。
    assert_eq!(ctx.secret("api_token"), None);
}
