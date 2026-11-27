//! HostPlugin 的测试：manifest 合法性与 OpenSurface 输出。

use flick_domain::*;
use flick_runtime::{ActionRuntime, Plugin};

#[tokio::test]
async fn phase0_builtins_register_and_route_host() {
    let info = flick_domain::HostInfo {
        os: flick_domain::OperatingSystem::Linux,
        arch: flick_domain::Architecture::X86_64,
        kernel: "test".into(),
        locale: "en_US.UTF-8".into(),
        shell: "bash".into(),
        window_manager: Some("Mutter".into()),
        lan_ip: None,
    };
    let rt = ActionRuntime::new(
        std::sync::Arc::new(info),
        std::sync::Arc::new(flick_domain::NoopPaths),
    );
    flick_builtins::register_builtin(&rt)
        .await
        .expect("register builtin plugins");
    rt.activate_all_plugins().await;

    let items = rt.route("host", InputType::PlainText).await;
    assert!(
        items
            .iter()
            .any(|i| i.action_id.as_str() == flick_builtins::HOST_ACTION_ID),
        "query `host` should route to the host plugin, got {items:?}"
    );

    let echo = rt.route("echo hello", InputType::PlainText).await;
    assert!(
        echo.iter()
            .any(|i| i.action_id.as_str() == flick_builtins::ECHO_ACTION_ID),
        "query `echo hello` should still route to echo"
    );
}

#[test]
fn host_manifest_is_valid() {
    let m = flick_builtins::host::HostPlugin.manifest();
    assert_eq!(m.id, "flick.builtin.host");
    assert!(m.profile.categories.contains(&Category::System));
    assert_eq!(m.surfaces.len(), 1);
    let a = flick_builtins::host::HostPlugin.actions();
    assert_eq!(a.len(), 1);
    assert_eq!(
        a[0].descriptor.id.as_str(),
        flick_builtins::host::HOST_ACTION_ID
    );
}

#[tokio::test]
async fn host_action_returns_open_surface() {
    let info = flick_domain::HostInfo {
        os: flick_domain::OperatingSystem::Linux,
        arch: flick_domain::Architecture::X86_64,
        kernel: "test".into(),
        locale: "en_US.UTF-8".into(),
        shell: "bash".into(),
        window_manager: Some("Mutter".into()),
        lan_ip: None,
    };
    let rt = ActionRuntime::new(
        std::sync::Arc::new(info),
        std::sync::Arc::new(flick_domain::NoopPaths),
    );
    rt.register_plugin(&flick_builtins::host::HostPlugin)
        .await
        .unwrap();
    let req = ActionRequest {
        invocation_id: InvocationId::new(),
        action_id: ActionId::from_static(flick_builtins::host::HOST_ACTION_ID),
        input: ActionInput::plain_text("host"),
        caller: Caller::cli("test"),
        source: flick_domain::ActionSource::Ui,
        deadline: Deadline::from_secs(30),
    };
    match rt.invoke(req).await {
        ActionOutcome::OpenSurface {
            surface_id,
            content,
            ..
        } => {
            assert_eq!(surface_id, "info");
            assert_eq!(content.schema_version, flick_domain::SURFACE_SCHEMA_VERSION);
            assert!(!content.body.is_empty());

            // os / arch 必须是人类可读文案，而非 Debug 派生（如 "X86_64"）。
            let kv = |key: &str| -> Option<String> {
                content.body.iter().find_map(|n| match n {
                    SurfaceNode::KeyValue { key: k, value, .. } if k == key => Some(value.clone()),
                    _ => None,
                })
            };
            assert_eq!(kv("系统").as_deref(), Some("Linux"));
            assert_eq!(kv("架构").as_deref(), Some("x86_64"));
            assert_eq!(kv("窗口管理器").as_deref(), Some("Mutter"));
        }
        other => panic!("expected OpenSurface, got {other:?}"),
    }
}
