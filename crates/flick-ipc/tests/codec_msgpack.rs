use flick_ipc::codec::{decode, encode, CodecError, MAX_FRAME_SIZE};
use flick_ipc::message::{
    IpcCommand, IpcRequestEnvelope, IpcResult, IPC_PROTOCOL_VERSION,
};

#[test]
fn envelope_roundtrips_over_msgpack() {
    let env = IpcRequestEnvelope {
        protocol_version: IPC_PROTOCOL_VERSION,
        request_id: "r1".into(),
        command: IpcCommand::Trigger { action_id: "flick.builtin.echo.v1.echo".into(), text: "hi".into() },
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
