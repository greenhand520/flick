use flick_runtime::{ActionSpec, Plugin, PluginLifecycle, PluginRuntimeError, PluginUiFactory};

// 仅用于编译期检查 trait 存在与对象安全。
fn _assert_plugin(_p: &dyn Plugin) {}
fn _assert_lifecycle(_l: &dyn PluginLifecycle) {}
fn _assert_ui(_u: &dyn PluginUiFactory) {}

#[test]
fn plugin_traits_exist() {
    // 一个最小实现能被当作 dyn Plugin（编译即可）
    struct Dummy;
    impl Plugin for Dummy {
        fn manifest(&self) -> flick_domain::PluginManifest { unimplemented!() }
        fn actions(&self) -> Vec<ActionSpec> { Vec::new() }
    }
    let d = Dummy;
    _assert_plugin(&d);
    assert!(matches!(PluginRuntimeError::Cancelled, PluginRuntimeError::Cancelled));
}
