use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::vcp_client::StreamEvent;
use tauri::ipc::Channel;

#[allow(clippy::too_many_arguments)]
pub(super) fn send_stream_end(
    stream_channel: Option<Channel<StreamEvent>>,
    message_id: String,
    owner_id: &str,
    topic_id: &str,
    is_group: bool,
    finish_reason: Option<String>,
    end_blocks: Option<Vec<ContentBlock>>,
    final_ts: u64,
    generation: Option<u64>,
) -> Result<(), String> {
    let Some(channel) = stream_channel else {
        return Ok(());
    };
    let generation = generation
        .filter(|value| *value > 0)
        .ok_or_else(|| "流式 end 事件缺少正整数 generation".to_string())?;
    let context = build_stream_context(owner_id, topic_id, is_group);
    channel
        .send(StreamEvent::end(
            message_id,
            context,
            Some(finish_reason.unwrap_or_else(|| "completed".to_string())),
            end_blocks,
            Some(final_ts),
            generation,
        ))
        .map_err(|error| format!("发送流式 end 事件失败: {error}"))
}

fn build_stream_context(
    owner_id: &str,
    topic_id: &str,
    is_group: bool,
) -> Option<serde_json::Value> {
    if owner_id.is_empty() || topic_id.is_empty() {
        return None;
    }
    if is_group {
        Some(serde_json::json!({
            "groupId": owner_id,
            "ownerType": "group",
            "topicId": topic_id,
            "isGroupMessage": true,
        }))
    } else {
        Some(serde_json::json!({
            "agentId": owner_id,
            "ownerType": "agent",
            "topicId": topic_id,
        }))
    }
}
