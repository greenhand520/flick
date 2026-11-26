//! 集成测试：`register_builtin` 必须注册应用搜索插件。

use flick_builtins::register_builtin;
use flick_runtime::ActionRuntime;

#[tokio::test]
async fn register_builtin_registers_apps_plugin() {
    let runtime = ActionRuntime::new();
    register_builtin(&runtime).await.unwrap();
    let route = runtime.route("app", flick_domain::InputType::PlainText).await;
    assert!(route
        .iter()
        .any(|item| item.action_id.as_str() == "flick.builtin.apps.search"));
}
