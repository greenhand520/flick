use flick_ipc::codec::{CodecError, MAX_FRAME_SIZE, decode, encode, read_frame};
use flick_ipc::message::{IPC_PROTOCOL_VERSION, IpcCommand, IpcRequestEnvelope, IpcResult};
use std::io::Cursor;

#[test]
fn envelope_roundtrips_over_msgpack() {
    let env = IpcRequestEnvelope {
        protocol_version: IPC_PROTOCOL_VERSION,
        request_id: "r1".into(),
        command: IpcCommand::Trigger {
            action_id: "flick.builtin.echo.v1.echo".into(),
            text: "hi".into(),
        },
    };
    let bytes = encode(&env).unwrap();
    let back: IpcRequestEnvelope = decode(&bytes[4..]).unwrap();
    assert_eq!(back.request_id, "r1");
    match back.command {
        IpcCommand::Trigger { text, .. } => assert_eq!(text, "hi"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn ipc_result_ok_value_roundtrips() {
    let r = IpcResult::Ok(serde_json::json!({ "content": { "schema_version": 1 } }));
    let bytes = encode(&r).unwrap();
    let back: IpcResult = decode(&bytes[4..]).unwrap();
    match back {
        IpcResult::Ok(v) => assert_eq!(v["content"]["schema_version"], 1),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn frame_too_large_rejected() {
    let big = "x".repeat(MAX_FRAME_SIZE as usize + 16);
    let err = encode(&big).unwrap_err();
    assert!(matches!(err, CodecError::FrameTooLarge { .. }));
}

#[test]
fn max_frame_size_is_one_mib() {
    assert_eq!(MAX_FRAME_SIZE, 1024 * 1024);
}

// ─── 边界与二进制稳健性（处理评审延后的 Minor 项） ──────────────────────────

/// 负载长度恰为 `MAX_FRAME_SIZE` 时必须被接受（边界是 `>` 而非 `>=`）。
#[test]
fn frame_exactly_max_size_accepted() {
    // MessagePack 的 str32 头为 5 字节（0xdb + 4 字节长度），
    // 因此正文字符数取 MAX_FRAME_SIZE - 5，使整个 MessagePack 负载恰好等于上限。
    let payload_chars = MAX_FRAME_SIZE as usize - 5;
    let s = "a".repeat(payload_chars);
    let frame = encode(&s).unwrap();
    assert_eq!(frame.len(), 4 + MAX_FRAME_SIZE as usize);
    let back: String = decode(&frame[4..]).unwrap();
    assert_eq!(back.chars().count(), payload_chars);
}

/// 超长帧（长度前缀 > MAX_FRAME_SIZE）走读取路径时必须返回 `FrameTooLarge`。
#[test]
fn overlong_frame_rejected_on_read() {
    let len = MAX_FRAME_SIZE + 1;
    let mut frame = Vec::new();
    frame.extend_from_slice(&len.to_be_bytes());
    let mut cursor = Cursor::new(frame);
    let err = read_frame::<serde_json::Value>(&mut cursor).unwrap_err();
    assert!(matches!(err, CodecError::FrameTooLarge { size } if size == len));
}

/// `decode` 公开 API 自我保护：超过上限的裸负载直接拒绝，不再交给解码器。
#[test]
fn overlong_payload_rejected_on_decode() {
    let data = vec![0u8; MAX_FRAME_SIZE as usize + 1];
    let err = decode::<serde_json::Value>(&data).unwrap_err();
    assert!(matches!(err, CodecError::FrameTooLarge { .. }));
}

/// 含非 UTF-8 字节的 MessagePack bin8 必须能解码为 `Vec<u8>`（二进制安全）。
#[test]
fn binary_payload_roundtrips() {
    // bin8 标记 0xc4 + 长度 0x03 + 字节 [0x00,0xff,0x80]
    let encoded = [0xc4u8, 0x03, 0x00, 0xff, 0x80];
    let decoded: Vec<u8> = decode(&encoded).unwrap();
    assert_eq!(decoded, vec![0x00, 0xff, 0x80]);
}

/// 带 `kind` 标签的完整 Surface 结构经 MessagePack 往返后与原始值相等。
/// 注意 `SurfaceNode` 使用 `#[serde(tag = "kind", rename_all = "snake_case")]`，
/// 所以标签是小写下划线形式（`heading` / `key_value`）。
#[test]
fn full_surface_value_roundtrips_over_msgpack() {
    let value = serde_json::json!({
        "surface_id": "info",
        "content": {
            "schema_version": 1,
            "title": "系统信息",
            "body": [
                { "kind": "heading", "text": "x" },
                { "kind": "key_value", "key": "os", "value": "Linux" }
            ]
        }
    });
    let frame = encode(&value).unwrap();
    let back: serde_json::Value = decode(&frame[4..]).unwrap();
    assert_eq!(value, back);
}

#[test]
fn settings_commands_roundtrip_over_msgpack() {
    let cases = vec![
        IpcCommand::SettingsList {
            namespace: "core".into(),
        },
        IpcCommand::SettingsGet {
            key: "core.theme.mode".into(),
        },
        IpcCommand::SettingsSet {
            key: "core.theme.mode".into(),
            value: flick_domain::settings::SettingValue::Enum("light".into()),
        },
        IpcCommand::SettingsReset {
            key: Some("core.theme.mode".into()),
            namespace: None,
        },
        IpcCommand::SettingsReset {
            key: None,
            namespace: Some("flick.builtin.apps".into()),
        },
    ];
    for command in cases {
        let env = IpcRequestEnvelope {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id: "req-settings".into(),
            command,
        };
        let bytes = encode(&env).unwrap();
        let back: IpcRequestEnvelope = decode(&bytes[4..]).unwrap();
        assert_eq!(back.protocol_version, IPC_PROTOCOL_VERSION);
    }
}
