use super::{
    expected_generation_for_identity, parse_native_generation, reject_partial_identity,
    stream_lease_tag, StreamIdentity,
};

#[test]
fn 原生服务必须返回正整数_generation() {
    assert_eq!(
        parse_native_generation(
            &serde_json::json!({"generation": 7}),
            "startStreamingService"
        ),
        Ok(7)
    );
    assert!(parse_native_generation(&serde_json::json!({}), "startStreamingService").is_err());
    assert!(parse_native_generation(
        &serde_json::json!({"generation": 0}),
        "startStreamingService"
    )
    .is_err());
}

#[test]
fn 完整身份释放必须提供_generation() {
    let identity = StreamIdentity::new("agent", "owner", "topic", "message");
    assert!(expected_generation_for_identity(Some(&identity), None).is_err());
    assert!(expected_generation_for_identity(Some(&identity), Some(0)).is_err());
    assert_eq!(
        expected_generation_for_identity(Some(&identity), Some(3)),
        Ok(Some(3))
    );
    assert_eq!(expected_generation_for_identity(None, None), Ok(None));
}

#[test]
fn 完整身份的前台标签必须按所有者和消息隔离() {
    let identity = StreamIdentity::new("group", "owner/1", "topic:1", "message-1");

    assert_eq!(
        stream_lease_tag("same agent name", Some(&identity)).unwrap(),
        "stream:session:67726f7570:6f776e65722f31:746f7069633a31:6d6573736167652d31"
    );
}

#[test]
fn 不完整身份不得回退到旧标签() {
    let identity = StreamIdentity {
        owner_type: Some("agent".to_string()),
        ..Default::default()
    };

    assert!(stream_lease_tag("legacy", Some(&identity)).is_err());
}

#[test]
fn 缺少身份时仅保留旧调用方兼容() {
    assert_eq!(stream_lease_tag("legacy", None).unwrap(), "stream:legacy");
}

#[test]
fn 部分身份在生成旧标签前被拒绝() {
    let identity = StreamIdentity {
        owner_type: Some("agent".to_string()),
        ..Default::default()
    };

    assert!(reject_partial_identity(Some(&identity)).is_err());
}

#[test]
fn 空白身份在生成旧标签前被拒绝() {
    let identity = StreamIdentity::new("agent", " ", "topic", "message");

    assert!(reject_partial_identity(Some(&identity)).is_err());
}

#[test]
fn android_cfg_释放分支使用已声明的_tag符号() {
    let source = include_str!("stream.rs");
    assert!(source.contains("serde_json::json!({ \"tag\": _tag })"));
    assert!(!source.contains("serde_json::json!({ \"tag\": tag })"));
}
