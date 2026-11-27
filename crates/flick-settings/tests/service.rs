//! `SettingsService` 行为测试。

use std::sync::Arc;

use flick_data::{DataDirectories, FixedMasterKeySource, MasterKey, StorageManager};
use flick_domain::settings::{
    SettingDecl, SettingKey, SettingKind, SettingNamespace, SettingSchema, SettingValue,
};
use flick_settings::{SettingsError, SettingsService, core_schema};

fn service(temp: &tempfile::TempDir) -> SettingsService {
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    let keys = Arc::new(FixedMasterKeySource(MasterKey {
        key_id: "test".into(),
        bytes: [5u8; 32],
    }));
    SettingsService::new(Arc::new(StorageManager::open(dirs, keys).unwrap()))
}

fn plugin_schema(namespace: &str, declarations: Vec<SettingDecl>) -> SettingSchema {
    SettingSchema {
        namespace: SettingNamespace::Plugin(namespace.to_string()),
        declarations,
        schema_version: 1,
    }
}

fn decl(key: &str, kind: SettingKind) -> SettingDecl {
    SettingDecl {
        key: key.to_string(),
        renamed_from: None,
        label: key.to_string(),
        description: String::new(),
        kind,
        default: None,
        secret: false,
    }
}

#[test]
fn core_defaults_resolve_without_rows() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    settings.register_schema(core_schema(), "0.1.0", "0.1.0").unwrap();

    let mode = SettingKey::new(SettingNamespace::Core, "theme.mode");
    assert_eq!(
        settings.resolved(&mode).unwrap(),
        SettingValue::Enum("dark".to_string())
    );
    assert_eq!(settings.get(&mode).unwrap(), None);
    let snapshot = settings.list("core").unwrap();
    assert!(snapshot.values.is_empty());
    assert_eq!(snapshot.revision, 0);
}

#[test]
fn set_get_and_list_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    settings.register_schema(core_schema(), "0.1.0", "0.1.0").unwrap();
    let limit = SettingKey::new(SettingNamespace::Core, "search.result_limit");

    let revision = settings.set(&limit, SettingValue::Integer(35)).unwrap();
    assert_eq!(revision, 1);
    assert_eq!(settings.get(&limit).unwrap(), Some(SettingValue::Integer(35)));
    let snapshot = settings.list("core").unwrap();
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.get("search.result_limit"), Some(&SettingValue::Integer(35)));
}

#[test]
fn rejects_unknown_namespace_key_and_invalid_value() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    settings.register_schema(core_schema(), "0.1.0", "0.1.0").unwrap();

    let missing_ns = SettingKey::new(SettingNamespace::Plugin("flick.none".into()), "x");
    assert!(matches!(
        settings.get(&missing_ns),
        Err(SettingsError::UnknownNamespace(_))
    ));

    let missing_key = SettingKey::new(SettingNamespace::Core, "not.declared");
    assert!(matches!(
        settings.get(&missing_key),
        Err(SettingsError::UnknownKey { .. })
    ));

    let limit = SettingKey::new(SettingNamespace::Core, "search.result_limit");
    assert!(matches!(
        settings.set(&limit, SettingValue::Integer(1000)),
        Err(SettingsError::InvalidValue { .. })
    ));
    assert!(matches!(
        settings.set(&limit, SettingValue::String("x".into())),
        Err(SettingsError::InvalidValue { .. })
    ));
}

#[test]
fn plugin_registration_records_meta_and_short_circuits_on_same_fingerprint() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    let schema = plugin_schema(
        "flick.demo",
        vec![decl("enabled", SettingKind::Bool)],
    );

    let first = settings
        .register_schema(schema.clone(), "1.2.0", "1.0.0")
        .unwrap();
    assert!(first.changed);
    assert_eq!(first.diff.added, vec!["enabled".to_string()]);

    let second = settings.register_schema(schema, "1.2.0", "1.0.0").unwrap();
    assert!(!second.changed, "同指纹不应重复写声明");
    assert_eq!(second.namespace, "flick.demo");

    let meta = settings
        .storage()
        .plugin_meta("flick.demo")
        .unwrap()
        .expect("插件元数据应存在");
    assert_eq!(meta.status, "installed");
    assert_eq!(meta.decl_version, "1.0.0");
}

#[test]
fn secret_settings_use_secret_table_and_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    let mut token = decl("api_token", SettingKind::String);
    token.secret = true;
    settings
        .register_schema(plugin_schema("flick.demo", vec![token]), "1.0.0", "1.0.0")
        .unwrap();

    let key = SettingKey::new(SettingNamespace::Plugin("flick.demo".into()), "api_token");
    settings.set(&key, SettingValue::String("tok".into())).unwrap();
    assert_eq!(
        settings.get(&key).unwrap(),
        Some(SettingValue::String("tok".into()))
    );
    // 明文表里没有该行
    let conn = rusqlite::Connection::open(settings.storage().config_db_path()).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM setting_values WHERE key = 'api_token'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn renamed_declaration_migrates_existing_value() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    settings
        .register_schema(
            plugin_schema("flick.demo", vec![decl("old_name", SettingKind::Bool)]),
            "1.0.0",
            "1.0.0",
        )
        .unwrap();
    let old_key = SettingKey::new(SettingNamespace::Plugin("flick.demo".into()), "old_name");
    settings.set(&old_key, SettingValue::Bool(true)).unwrap();

    let mut renamed = decl("new_name", SettingKind::Bool);
    renamed.renamed_from = Some("old_name".into());
    let outcome = settings
        .register_schema(
            plugin_schema("flick.demo", vec![renamed]),
            "1.1.0",
            "1.1.0",
        )
        .unwrap();
    assert_eq!(
        outcome.diff.renamed,
        vec![("old_name".to_string(), "new_name".to_string())]
    );

    let new_key = SettingKey::new(SettingNamespace::Plugin("flick.demo".into()), "new_name");
    assert_eq!(
        settings.get(&new_key).unwrap(),
        Some(SettingValue::Bool(true))
    );
    assert!(matches!(
        settings.get(&old_key),
        Err(SettingsError::UnknownKey { .. })
    ));
}

#[test]
fn revisions_and_change_events_follow_writes() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    settings.register_schema(core_schema(), "0.1.0", "0.1.0").unwrap();
    let mut receiver = settings.subscribe();

    let key = SettingKey::new(SettingNamespace::Core, "theme.mode");
    let revision = settings
        .set(&key, SettingValue::Enum("light".into()))
        .unwrap();
    assert_eq!(revision, 1);
    let change = receiver.try_recv().expect("应收到变更事件");
    assert_eq!(change.namespace, SettingNamespace::Core);
    assert_eq!(change.revision, 1);

    let reset_revision = settings.reset_key(&key).unwrap();
    assert_eq!(reset_revision, 2);
    let change = receiver.try_recv().unwrap();
    assert_eq!(change.revision, 2);
}

#[test]
fn reset_namespace_clears_values_but_keeps_declarations() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    settings
        .register_schema(
            plugin_schema(
                "flick.demo",
                vec![decl("a", SettingKind::Bool), decl("b", SettingKind::Bool)],
            ),
            "1.0.0",
            "1.0.0",
        )
        .unwrap();
    let a = SettingKey::new(SettingNamespace::Plugin("flick.demo".into()), "a");
    settings.set(&a, SettingValue::Bool(true)).unwrap();

    let revision = settings.reset_namespace("flick.demo").unwrap();
    assert_eq!(revision, 2);
    assert!(settings.list("flick.demo").unwrap().values.is_empty());
    assert_eq!(settings.schema("flick.demo").unwrap().declarations.len(), 2);
}

#[test]
fn invalid_plugin_namespace_is_rejected_at_registration() {
    let temp = tempfile::tempdir().unwrap();
    let settings = service(&temp);
    let bad = plugin_schema("../escape", vec![decl("a", SettingKind::Bool)]);
    assert!(matches!(
        settings.register_schema(bad, "1.0.0", "1.0.0"),
        Err(SettingsError::InvalidSchema { .. })
    ));
}
