use super::*;
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::vcp_client::{ActiveRequestGuard, ActiveRequestRegistry};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::{mpsc, oneshot};

fn agent_config() -> AgentConfig {
    AgentConfig {
        id: "agent-a".to_string(),
        name: "助手".to_string(),
        system_prompt: "system".to_string(),
        mobile_system_prompt: String::new(),
        model: "model-a".to_string(),
        temperature: 1.0,
        context_token_limit: 1024,
        max_output_tokens: 256,
        stream_output: true,
        use_temperature: true,
        avatar_calculated_color: None,
        topics: Vec::new(),
    }
}

fn capture_channel() -> (
    Channel<StreamEvent>,
    mpsc::UnboundedReceiver<InvokeResponseBody>,
) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let channel = Channel::new(move |event| {
        sender.send(event).expect("记录流事件");
        Ok(())
    });
    (channel, receiver)
}

async fn receive_event(receiver: &mut mpsc::UnboundedReceiver<InvokeResponseBody>) -> Value {
    match receiver.recv().await.expect("应收到流事件") {
        InvokeResponseBody::Json(value) => serde_json::from_str(&value).expect("解析流事件"),
        _ => panic!("流事件必须序列化为 JSON"),
    }
}

async fn registered_request() -> (CompletionLease, ActiveRequestGuard) {
    let registry = Arc::new(ActiveRequestRegistry::default());
    let key = MessageKey::new(
        TopicKey::new("agent", "agent-a", "assistant_chat"),
        "message-a",
    );
    let (abort_sender, _abort_receiver) = oneshot::channel();
    let (epoch, _previous, lease) = registry.register(key.clone(), abort_sender).await;
    let guard = ActiveRequestGuard::new(registry, key, epoch);
    (lease, guard)
}

#[test]
fn 划词助手请求显式使用非持久化模式() {
    let payload = build_assistant_request_payload(
        "message-a".to_string(),
        "agent-a",
        &agent_config(),
        Vec::new(),
        "http://127.0.0.1:5890".to_string(),
        "key".to_string(),
    );

    assert_eq!(payload.mode, VcpRequestMode::Ephemeral);
    assert_eq!(
        payload.context.as_ref().unwrap()["topicId"],
        "assistant_chat"
    );
}

#[tokio::test]
async fn 非持久化成功终结只发送事件且不需要数据库() {
    let (completion_lease, request_guard) = registered_request().await;
    let generation = completion_lease.epoch();
    let (channel, mut receiver) = capture_channel();
    let outcome = VcpRequestOutcome {
        response: json!({
            "fullContent": "完成内容",
            "finishReason": "completed"
        }),
        is_aborted: false,
        completion_lease,
        request_guard,
    };

    emit_assistant_success(outcome, &channel)
        .await
        .expect("非持久化终结不应访问数据库");

    let event = receive_event(&mut receiver).await;
    assert_eq!(event["type"], "end");
    assert_eq!(event["messageId"], "message-a");
    assert_eq!(event["generation"], generation);
    assert_eq!(event["context"]["topicId"], "assistant_chat");
    assert!(event["blocks"].is_null());
}

#[tokio::test]
async fn 非持久化失败终结只发送错误事件且不需要数据库() {
    let (completion_lease, request_guard) = registered_request().await;
    let generation = completion_lease.epoch();
    let (channel, mut receiver) = capture_channel();
    let error = VcpRequestError {
        message: "上游失败".to_string(),
        completion_lease: Some(completion_lease),
        stale: false,
        request_guard: Some(request_guard),
    };

    let result = emit_assistant_error(error, &channel).await;

    assert_eq!(result, Err("上游失败".to_string()));
    let event = receive_event(&mut receiver).await;
    assert_eq!(event["type"], "error");
    assert_eq!(event["messageId"], "message-a");
    assert_eq!(event["generation"], generation);
    assert_eq!(event["error"], "上游失败");
}
