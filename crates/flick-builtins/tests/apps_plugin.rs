//! 集成测试：`register_builtin` 必须注册应用搜索插件。

use flick_builtins::register_builtin;
use flick_runtime::ActionRuntime;

#[tokio::test]
async fn register_builtin_registers_apps_plugin() {
    let runtime = ActionRuntime::default();
    register_builtin(&runtime).await.unwrap();
    let route = runtime
        .route("app", flick_domain::InputType::PlainText)
        .await;
    assert!(
        route
            .iter()
            .any(|item| item.action_id.as_str() == "flick.builtin.apps.search")
    );
}

/// 端到端：搜索阶段返回 Execution 项，激活时交给父 Action 启动进程。
#[tokio::test]
async fn apps_execution_item_activates_through_runtime() {
    use flick_builtins::apps::AppsPlugin;
    use flick_domain::{ActionId, ExecutionEventKind};

    let dir = tempfile::tempdir().unwrap();
    let apps_dir = dir.path().join("applications");
    std::fs::create_dir_all(&apps_dir).unwrap();
    std::fs::write(
        apps_dir.join("true.desktop"),
        "[Desktop Entry]\nType=Application\nName=TrueApp\nExec=/bin/true\n",
    )
    .unwrap();

    let runtime = ActionRuntime::default();
    let plugin = AppsPlugin::with_dirs(vec![apps_dir]);
    runtime.register_plugin(&plugin).await.unwrap();
    runtime.activate_all_plugins().await;

    let items = runtime
        .search_invoke(
            flick_domain::QueryGeneration::INITIAL,
            flick_domain::ActionInput::plain_text("true"),
        )
        .await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "TrueApp");
    assert_eq!(items[0].item_id, "app:true");

    let outcome = runtime
        .execution_event(
            ActionId::from_static("flick.builtin.apps.search"),
            &items[0].item_id,
            ExecutionEventKind::Activate,
        )
        .await
        .expect("activation outcome");
    assert!(matches!(
        outcome,
        flick_domain::ActionOutcome::Notice { .. }
    ));
}
