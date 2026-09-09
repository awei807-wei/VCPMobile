use crate::vcp_modules::aurora_pipeline::AuroraUpdate;
use crate::vcp_modules::content_parser::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum VcpRequestMode {
    #[default]
    Persistent,
    Ephemeral,
}

/// VCP 请求参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcpRequestPayload {
    pub vcp_url: String,
    pub vcp_api_key: String,
    pub messages: Vec<Value>,
    pub model_config: Value,
    pub message_id: String,
    pub context: Option<Value>,
    /// 仅供 Rust 内部调用方选择；IPC 输入始终使用持久化模式。
    #[serde(skip)]
    pub(crate) mode: VcpRequestMode,
}

/// 发送给前端的 VCP 流式事件。
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: String,
    pub chunk: Option<Value>,
    pub message_id: String,
    /// 应用内请求纪元；所有可消费的流事件都必须绑定正整数纪元。
    pub generation: u64,
    pub context: Option<Value>,
    pub finish_reason: Option<String>,
    pub error: Option<String>,
    pub aurora: Option<AuroraUpdate>,
    pub blocks: Option<Vec<ContentBlock>>,
    pub timestamp: Option<u64>,
}

impl StreamEvent {
    pub fn thinking(message_id: String, context: Option<Value>, generation: u64) -> Self {
        Self {
            r#type: "thinking".into(),
            message_id,
            generation: require_stream_generation(generation),
            context,
            ..Default::default()
        }
    }

    pub fn aurora(
        message_id: String,
        aurora: AuroraUpdate,
        context: Option<Value>,
        generation: u64,
    ) -> Self {
        Self {
            r#type: "aurora".into(),
            aurora: Some(aurora),
            message_id,
            generation: require_stream_generation(generation),
            context,
            ..Default::default()
        }
    }

    pub fn end(
        message_id: String,
        context: Option<Value>,
        finish_reason: Option<String>,
        blocks: Option<Vec<ContentBlock>>,
        timestamp: Option<u64>,
        generation: u64,
    ) -> Self {
        Self {
            r#type: "end".into(),
            message_id,
            generation: require_stream_generation(generation),
            context,
            finish_reason,
            blocks,
            timestamp,
            ..Default::default()
        }
    }

    pub fn error(
        message_id: String,
        context: Option<Value>,
        error: String,
        generation: u64,
    ) -> Self {
        Self {
            r#type: "error".into(),
            message_id,
            generation: require_stream_generation(generation),
            context,
            finish_reason: Some("error".to_string()),
            error: Some(error),
            ..Default::default()
        }
    }
}

fn require_stream_generation(generation: u64) -> u64 {
    assert!(generation > 0, "流事件 generation 必须是正整数");
    generation
}
