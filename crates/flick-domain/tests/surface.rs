use flick_domain::{
    BadgeTone, ListItem, SURFACE_SCHEMA_VERSION, SpacerSize, SurfaceContent, SurfaceNode,
};

fn sample() -> SurfaceContent {
    SurfaceContent {
        schema_version: SURFACE_SCHEMA_VERSION,
        title: Some("系统信息".into()),
        body: vec![
            SurfaceNode::Heading {
                id: None,
                text: "概览".into(),
            },
            SurfaceNode::Text {
                id: Some("t1".into()),
                text: "hello".into(),
                muted: true,
            },
            SurfaceNode::Section {
                id: None,
                title: Some("CPU".into()),
                children: vec![
                    SurfaceNode::KeyValue {
                        id: None,
                        key: "型号".into(),
                        value: "x".into(),
                    },
                    SurfaceNode::Badge {
                        id: None,
                        text: "ok".into(),
                        tone: BadgeTone::Success,
                    },
                ],
            },
            SurfaceNode::List {
                id: None,
                items: vec![ListItem {
                    id: None,
                    title: "内存".into(),
                    subtitle: Some("15 GiB".into()),
                    value: None,
                    badge: Some("RAM".into()),
                }],
            },
            SurfaceNode::Row {
                id: None,
                children: vec![SurfaceNode::Divider { id: None }],
            },
            SurfaceNode::Column {
                id: None,
                children: vec![SurfaceNode::Spacer {
                    id: None,
                    size: SpacerSize::Md,
                }],
            },
        ],
    }
}

#[test]
fn surface_content_roundtrips_json() {
    let c = sample();
    let json = serde_json::to_string(&c).unwrap();
    let back: SurfaceContent = serde_json::from_str(&json).unwrap();
    assert_eq!(c, back);
}

#[test]
fn surface_content_roundtrips_msgpack() {
    let c = sample();
    let bytes = rmp_serde::to_vec_named(&c).unwrap();
    let back: SurfaceContent = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(c, back);
}

#[test]
fn schema_version_defaults_when_missing() {
    let c: SurfaceContent = serde_json::from_str(r#"{"body":[]}"#).unwrap();
    assert_eq!(c.schema_version, 1);
    assert!(c.title.is_none());
    assert!(c.body.is_empty());
}
