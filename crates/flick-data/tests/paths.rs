//! 路径解析与目录安全测试。

use flick_data::{DataDirectories, DataError};
use flick_domain::PluginPaths;

#[test]
fn discover_yields_four_non_empty_roots() {
    let dirs = DataDirectories::discover().expect("discover");
    assert!(dirs.config_root().is_absolute() || !dirs.config_root().as_os_str().is_empty());
    assert!(!dirs.data_root().as_os_str().is_empty());
    assert!(!dirs.cache_root().as_os_str().is_empty());
    assert!(!dirs.runtime_root().as_os_str().is_empty());
    assert!(dirs.config_db().ends_with("core.sqlite3"));
}

#[test]
fn plugin_dirs_are_created_with_0700() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    let data_dir = dirs.plugin_dir("flick.builtin.apps").expect("plugin dir");
    assert!(data_dir.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&data_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "目录权限应为 0700");
    }
    assert!(dirs.plugin_cache_dir("flick.builtin.apps").unwrap().is_dir());
    assert!(dirs.plugin_tmp_dir("flick.builtin.apps").unwrap().is_dir());
}

#[test]
fn plugin_paths_trait_exposes_all_three_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    let provider: &dyn PluginPaths = &dirs;
    assert_eq!(
        provider.data_dir("flick.demo"),
        Some(root.join("data").join("plugins").join("flick.demo"))
    );
    assert_eq!(
        provider.cache_dir("flick.demo"),
        Some(root.join("cache").join("plugins").join("flick.demo"))
    );
    assert_eq!(
        provider.tmp_dir("flick.demo"),
        Some(root.join("runtime").join("plugins").join("flick.demo"))
    );
    assert_eq!(provider.data_dir("bad/id"), None);
}

#[test]
fn invalid_plugin_ids_are_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    for bad in ["", "..", ".", "core", "a/b", "../escape", "/abs", "a\\b"] {
        assert!(
            matches!(dirs.plugin_dir(bad), Err(DataError::InvalidPluginId(_))),
            "应拒绝 {bad:?}"
        );
    }
}

#[test]
#[cfg(unix)]
fn symlinked_plugin_dir_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let dirs = DataDirectories::with_roots(
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("runtime"),
    );
    let plugins = root.join("data").join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let target = root.join("elsewhere");
    std::fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, plugins.join("flick.evil")).unwrap();
    assert!(matches!(
        dirs.plugin_dir("flick.evil"),
        Err(DataError::Symlink { .. })
    ));
}
