use super::preparation::build_resume_context;
use super::require_expected_generation;

#[test]
fn 接续必须携带正整数helper纪元() {
    assert!(require_expected_generation(None).is_err());
    assert!(require_expected_generation(Some(0)).is_err());
    assert_eq!(require_expected_generation(Some(7)).unwrap(), 7);
}

#[test]
fn agent接续上下文携带规范ownerId并保留agentId兼容字段() {
    let context = build_resume_context("agent-owner", "agent", "topic-a");

    assert_eq!(context["ownerId"], "agent-owner");
    assert_eq!(context["ownerType"], "agent");
    assert_eq!(context["topicId"], "topic-a");
    assert_eq!(context["agentId"], "agent-owner");
    assert!(context["groupId"].is_null());
}

#[test]
fn group接续上下文携带规范ownerId并保留groupId兼容字段() {
    let context = build_resume_context("group-owner", "group", "topic-g");

    assert_eq!(context["ownerId"], "group-owner");
    assert_eq!(context["ownerType"], "group");
    assert_eq!(context["topicId"], "topic-g");
    assert_eq!(context["groupId"], "group-owner");
    assert!(context["agentId"].is_null());
}
