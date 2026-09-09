use super::*;
use crate::vcp_modules::chat::topic_types::{MessageKey, TopicKey};
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::mpsc;

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

async fn registered_lease() -> (u64, CompletionLease) {
    let registry = ActiveRequestRegistry::default();
    let key = MessageKey::new(
        TopicKey::new("agent", "agent-a", "assistant_chat"),
        "message-a",
    );
    let (_abort_receiver, epoch, lease) = register_request(&registry, key).await;
    (epoch, lease)
}

#[tokio::test]
async fn 非持久化流准备无需数据库并发送thinking事件() {
    let app = tauri::test::mock_app();
    let (epoch, lease) = registered_lease().await;
    let (channel, mut receiver) = capture_channel();
    let context = json!({
        "agentId": "agent-a",
        "ownerType": "agent",
        "topicId": "assistant_chat"
    });

    prepare_stream_request(
        app.handle(),
        Some(&context),
        &lease,
        Some(&channel),
        "message-a",
        epoch,
        VcpRequestMode::Ephemeral,
    )
    .await
    .expect("非持久化流不应依赖 DbState 或 topics 记录");

    let event = receive_event(&mut receiver).await;
    assert_eq!(event["type"], "thinking");
    assert_eq!(event["messageId"], "message-a");
    assert_eq!(event["generation"], epoch);
}

#[tokio::test]
async fn 持久化流准备仍要求数据库状态() {
    let app = tauri::test::mock_app();
    let (epoch, lease) = registered_lease().await;

    let result = prepare_stream_request(
        app.handle(),
        None,
        &lease,
        None,
        "message-a",
        epoch,
        VcpRequestMode::Persistent,
    )
    .await;

    match result {
        Err(PrepareError::Failed(error)) => {
            assert_eq!(error, crate::vcp_modules::db_manager::CORE_NOT_READY_ERROR)
        }
        _ => panic!("持久化流必须继续通过数据库准备路径"),
    }
}
