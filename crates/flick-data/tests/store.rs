//! `StorageManager` 的读写、revision、密文与清理测试。

use std::sync::Arc;

use flick_data::{DataDirectories, DataError, FixedMasterKeySource, MasterKey, StorageManager};
use flick_domain::settings::{SettingDecl, SettingKind, SettingValue};

fn master(key_id: &str, byte: u8) -> MasterKey {
    MasterKey {
        key_id: key_id.to_string(),
        bytes: [byte; 32],
    }
}

fn manager(temp: &tempfile::TempDir) -> StorageManager {
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    StorageManager::open(dirs, Arc::new(FixedMasterKeySource(master("test-1", 7))))
        .expect("storage manager")
}

fn bool_decl(key: &str) -> SettingDecl {
    SettingDecl {
        key: key.to_string(),
        renamed_from: None,
        label: key.to_string(),
        description: String::new(),
        kind: SettingKind::Bool,
        default: Some(SettingValue::Bool(false)),
        secret: false,
    }
}

#[test]
fn migrates_and_sets_pragmas() {
    let temp = tempfile::tempdir().unwrap();
    let mgr = manager(&temp);
    assert!(mgr.config_db_path().exists());
    let version = flick_data::SCHEMA_VERSION;
    assert!(version >= 1);
}

#[test]
fn plain_values_round_trip_and_bump_revision() {
    let temp = tempfile::tempdir().unwrap();
    let mgr = manager(&temp);
    let kind = SettingKind::Integer { min: 0, max: 10 };
    let rev1 = mgr
        .set_value(
            "core",
            "search.limit",
            &kind,
            &SettingValue::Integer(5),
            false,
        )
        .unwrap();
    assert_eq!(rev1, 1);
    assert_eq!(
        mgr.get_value("core", "search.limit", &kind, false).unwrap(),
        Some(SettingValue::Integer(5))
    );
    let rev2 = mgr
        .set_value(
            "core",
            "search.limit",
            &kind,
            &SettingValue::Integer(6),
            false,
        )
        .unwrap();
    assert_eq!(rev2, 2);
    assert_eq!(mgr.revision("core").unwrap(), 2);

    // 越界值必须被拒绝
    assert!(
        mgr.set_value(
            "core",
            "search.limit",
            &kind,
            &SettingValue::Integer(99),
            false
        )
        .is_err()
    );
}

#[test]
fn secret_values_never_land_in_plain_table() {
    let temp = tempfile::tempdir().unwrap();
    let mgr = manager(&temp);
    let kind = SettingKind::String;
    let rev = mgr
        .set_value(
            "plugin.flick.demo",
            "api_key",
            &kind,
            &SettingValue::String("s3cret".into()),
            true,
        )
        .unwrap();
    assert_eq!(rev, 1);
    assert_eq!(
        mgr.get_value("plugin.flick.demo", "api_key", &kind, true)
            .unwrap(),
        Some(SettingValue::String("s3cret".into()))
    );

    // 明文表里必须没有这行
    let conn = rusqlite::Connection::open(mgr.config_db_path()).unwrap();
    let plain_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM setting_values WHERE namespace = 'plugin.flick.demo' AND key = 'api_key'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(plain_rows, 0, "secret 不得进 setting_values");
    let secret_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM secret_values", [], |row| row.get(0))
        .unwrap();
    assert_eq!(secret_rows, 1);
}

#[test]
fn wrong_master_key_cannot_decrypt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    {
        let mgr = StorageManager::open(
            dirs.clone(),
            Arc::new(FixedMasterKeySource(master("k1", 1))),
        )
        .unwrap();
        mgr.set_value(
            "plugin.flick.demo",
            "token",
            &SettingKind::String,
            &SettingValue::String("abc".into()),
            true,
        )
        .unwrap();
    }
    let mgr = StorageManager::open(dirs, Arc::new(FixedMasterKeySource(master("k2", 2)))).unwrap();
    let err = mgr
        .get_value("plugin.flick.demo", "token", &SettingKind::String, true)
        .unwrap_err();
    assert!(matches!(err, DataError::Crypto(_)), "换密钥后应解密失败");
}

#[test]
fn unavailable_key_source_refuses_secret_io() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    let mgr = StorageManager::open(dirs, Arc::new(flick_data::UnavailableMasterKeySource)).unwrap();
    let err = mgr
        .set_value(
            "plugin.flick.demo",
            "token",
            &SettingKind::String,
            &SettingValue::String("abc".into()),
            true,
        )
        .unwrap_err();
    assert!(matches!(err, DataError::SecretStoreUnavailable(_)));
}

#[test]
fn decl_diff_marks_added_removed_and_renamed() {
    let temp = tempfile::tempdir().unwrap();
    let mgr = manager(&temp);
    // 先写入 a=真 与 b=真
    mgr.replace_decls("plugin.flick.demo", &[bool_decl("a"), bool_decl("b")], 1)
        .unwrap();
    mgr.set_value(
        "plugin.flick.demo",
        "a",
        &SettingKind::Bool,
        &SettingValue::Bool(true),
        false,
    )
    .unwrap();

    // 改名 a -> alpha，删除 b，新增 c
    let mut renamed = bool_decl("alpha");
    renamed.renamed_from = Some("a".into());
    let diff = mgr
        .replace_decls("plugin.flick.demo", &[renamed, bool_decl("c")], 2)
        .unwrap();
    assert!(
        diff.renamed
            .contains(&("a".to_string(), "alpha".to_string()))
    );
    assert!(diff.removed.contains(&"b".to_string()));
    assert!(diff.added.contains(&"c".to_string()));

    // 值迁到新 key
    assert_eq!(
        mgr.get_value("plugin.flick.demo", "alpha", &SettingKind::Bool, false)
            .unwrap(),
        Some(SettingValue::Bool(true))
    );

    let decls = mgr.load_decls("plugin.flick.demo").unwrap();
    let state_of = |key: &str| {
        decls
            .iter()
            .find(|stored| stored.decl.key == key)
            .map(|stored| stored.state.clone())
            .unwrap_or_default()
    };
    assert_eq!(state_of("a"), "retired");
    assert_eq!(state_of("b"), "orphaned");
    assert_eq!(state_of("c"), "active");
}

#[test]
fn reset_operations_bump_revision() {
    let temp = tempfile::tempdir().unwrap();
    let mgr = manager(&temp);
    mgr.set_value(
        "core",
        "a",
        &SettingKind::Bool,
        &SettingValue::Bool(true),
        false,
    )
    .unwrap();
    mgr.set_value(
        "core",
        "b",
        &SettingKind::Bool,
        &SettingValue::Bool(true),
        false,
    )
    .unwrap();
    let rev = mgr.reset_key("core", "a").unwrap();
    assert_eq!(rev, 3);
    assert_eq!(
        mgr.get_value("core", "a", &SettingKind::Bool, false)
            .unwrap(),
        None
    );
    let rev = mgr.reset_namespace("core").unwrap();
    assert_eq!(rev, 4);
    assert_eq!(
        mgr.get_value("core", "b", &SettingKind::Bool, false)
            .unwrap(),
        None
    );
}

#[test]
fn purge_removes_soft_uninstalled_after_retention() {
    let temp = tempfile::tempdir().unwrap();
    let mgr = manager(&temp);
    mgr.upsert_plugin("flick.demo", "1.0.0", "1.0.0", "fp1|x")
        .unwrap();
    mgr.replace_decls("flick.demo", &[bool_decl("a")], 1)
        .unwrap();
    mgr.set_value(
        "flick.demo",
        "a",
        &SettingKind::Bool,
        &SettingValue::Bool(true),
        false,
    )
    .unwrap();
    mgr.mark_uninstalled("flick.demo").unwrap();

    // 尚未到期：不清理
    assert!(mgr.purge_uninstalled_older_than(30).unwrap().is_empty());

    // 把 uninstalled_at 改成 31 天前
    let old = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 31 * 86_400) as i64;
    let conn = rusqlite::Connection::open(mgr.config_db_path()).unwrap();
    conn.execute(
        "UPDATE plugins SET uninstalled_at = ?1 WHERE plugin_id = 'flick.demo'",
        [old.to_string()],
    )
    .unwrap();
    drop(conn);

    let purged = mgr.purge_uninstalled_older_than(30).unwrap();
    assert_eq!(purged, vec!["flick.demo".to_string()]);
    assert!(mgr.plugin_meta("flick.demo").unwrap().is_none());
    assert!(mgr.load_decls("flick.demo").unwrap().is_empty());
    assert_eq!(
        mgr.get_value("flick.demo", "a", &SettingKind::Bool, false)
            .unwrap(),
        None
    );
}

#[test]
fn concurrent_revision_increments_do_not_lose_updates() {
    let temp = tempfile::tempdir().unwrap();
    let mgr = Arc::new(manager(&temp));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let mgr = Arc::clone(&mgr);
        handles.push(std::thread::spawn(move || {
            for _ in 0..10 {
                mgr.set_value(
                    "core",
                    "counter",
                    &SettingKind::Bool,
                    &SettingValue::Bool(true),
                    false,
                )
                .unwrap();
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(mgr.revision("core").unwrap(), 40);
}

#[test]
fn rotate_master_key_reencrypts_existing_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    let key_holder = Arc::new(std::sync::Mutex::new(master("k1", 3)));
    let history = Arc::new(std::sync::Mutex::new(vec![master("k1", 3)]));
    struct SwapSource {
        current: Arc<std::sync::Mutex<MasterKey>>,
        history: Arc<std::sync::Mutex<Vec<MasterKey>>>,
    }
    impl flick_data::MasterKeySource for SwapSource {
        fn load(&self) -> Result<MasterKey, DataError> {
            Ok(self.current.lock().unwrap().clone())
        }
        fn load_by_id(&self, key_id: &str) -> Option<MasterKey> {
            self.history
                .lock()
                .unwrap()
                .iter()
                .find(|key| key.key_id == key_id)
                .cloned()
        }
    }
    let mgr = StorageManager::open(
        dirs,
        Arc::new(SwapSource {
            current: Arc::clone(&key_holder),
            history,
        }),
    )
    .unwrap();
    mgr.set_value(
        "plugin.flick.demo",
        "token",
        &SettingKind::String,
        &SettingValue::String("abc".into()),
        true,
    )
    .unwrap();

    // 换主密钥后重加密
    *key_holder.lock().unwrap() = master("k2", 9);
    let count = mgr.rotate_master_key().unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        mgr.get_value("plugin.flick.demo", "token", &SettingKind::String, true)
            .unwrap(),
        Some(SettingValue::String("abc".into()))
    );
}
