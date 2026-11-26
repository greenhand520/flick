use flick_domain::{
    ActionDescriptor, ActionId, ActionSource, ConcurrencyPolicy, ExecutionEventKind, ExecutionItem,
    InvocationId, RouteItem, RouteItemKind,
};

#[test]
fn descriptor_defaults_version_and_search_invocable() {
    let d = ActionDescriptor::new(
        ActionId::from_static("test.plugin.search"),
        "搜索",
        vec!["search".into()],
        ConcurrencyPolicy::Parallel,
    );
    assert_eq!(d.version, "1.0.0");
    assert!(!d.search_invocable);
}

#[test]
fn action_source_default_is_ui() {
    assert_eq!(ActionSource::default(), ActionSource::Ui);
}

#[test]
fn execution_item_round_trips_and_route_helper_marks_action() {
    let item = ExecutionItem {
        item_id: "app:firefox".into(),
        parent_action_id: ActionId::from_static("flick.builtin.apps.search"),
        invocation_id: InvocationId::new(),
        title: "Firefox".into(),
        subtitle: Some("浏览器".into()),
        icon: Some("firefox".into()),
        badge: None,
        output: serde_json::json!({"exec": "firefox"}),
    };
    let json = serde_json::to_string(&item).unwrap();
    let back: ExecutionItem = serde_json::from_str(&json).unwrap();
    assert_eq!(back.title, "Firefox");
    assert_eq!(back.output["exec"], "firefox");

    let route = RouteItem::action(
        ActionId::from_static("flick.builtin.apps.search"),
        "应用搜索",
        1.0,
    );
    assert_eq!(route.kind, RouteItemKind::Action);
    assert_eq!(route.item_id, "action:flick.builtin.apps.search");
}

#[test]
fn execution_event_kind_serde() {
    let json = serde_json::to_string(&ExecutionEventKind::Activate).unwrap();
    assert_eq!(json, "\"Activate\"");
}
