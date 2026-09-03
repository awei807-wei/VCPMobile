use dashmap::DashSet;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use std::sync::Arc;
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};

use crate::vcp_modules::aurora_pipeline::AuroraUpdate;
use crate::vcp_modules::chat::topic_types::MessageKey;
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::db_manager::{DbState, CORE_NOT_READY_ERROR};
use crate::vcp_modules::settings_manager::{create_default_settings, Settings};

#[path = "vcp_client_active.rs"]
mod active;
#[path = "vcp_client_connection.rs"]
mod connection;
#[path = "vcp_client_non_stream.rs"]
mod non_stream;
#[path = "vcp_client_preprocess.rs"]
mod preprocess;
#[path = "vcp_client_recovery.rs"]
mod recovery;
#[path = "vcp_client_registry.rs"]
mod registry;
#[path = "vcp_client_resume.rs"]
mod resume;
#[path = "vcp_client_stream.rs"]
mod stream;
#[path = "vcp_client_transport.rs"]
mod transport;
use active::delete_active_generation_for_key;
#[allow(unused_imports)]
pub use active::{get_active_generations, interruptRequest, ActiveGeneration};
pub use connection::test_vcp_connection;
use non_stream::handle_non_streaming_request;
pub use preprocess::perform_vcp_request;
pub use recovery::recover_active_generation;
use registry::message_key_from_context;
pub use registry::{ActiveRequestRegistry, ActiveRequests};
pub use resume::resume_stream;
use stream::handle_streaming_request;

fn require_core_state<T>(state: Option<T>) -> Result<T, String> {
    state.ok_or_else(|| CORE_NOT_READY_ERROR.to_string())
}

fn db_pool_if_ready<R: Runtime>(app: &AppHandle<R>) -> Result<sqlx::Pool<sqlx::Sqlite>, String> {
    let db = require_core_state(app.try_state::<DbState>())?;
    Ok(db.pool.clone())
}

/// =================================================================
/// vcp_modules/vcp_client.rs - 统一的 VCP 请求处理模块 (Rust 重写版)
/// =================================================================
/// 该模块对应原项目的 modules/vcpClient.js，负责处理所有与 VCP 服务器的通信。
/// 包含动态路由、上下文注入（音乐、UI 规范）、流式 SSE 解析以及请求中止机制。
/// 请求参数结构体
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcpRequestPayload {
    pub vcp_url: String,        // VCP服务器URL
    pub vcp_api_key: String,    // API密钥
    pub messages: Vec<Value>,   // 消息数组
    pub model_config: Value,    // 模型配置 (包含 model, stream, temperature 等)
    pub message_id: String,     // 消息ID (用于跟踪和中止)
    pub context: Option<Value>, // 上下文信息 (agentId, topicId等)
}

/// 流式事件结构体，用于向前端发送数据
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: String, // 事件类型: "data", "aurora", "end", "error", "reconnecting"
    pub chunk: Option<Value>, // 数据块 (仅 type="data" 时有效)
    pub message_id: String, // 消息ID
    pub context: Option<Value>, // 透传的上下文信息
    pub finish_reason: Option<String>, // 结束原因
    pub error: Option<String>, // 错误信息 (仅 type="error" 时有效)
    pub aurora: Option<AuroraUpdate>, // Aurora 语义沉淀更新 (type="aurora" 时有效)
    pub blocks: Option<Vec<ContentBlock>>, // 持久化后的预渲染块 (仅 type="end" 时有效)
    pub timestamp: Option<u64>, // ⚡ 新增物理落笔时间戳
}

impl StreamEvent {
    pub fn thinking(message_id: String, context: Option<Value>) -> Self {
        Self {
            r#type: "thinking".into(),
            message_id,
            context,
            ..Default::default()
        }
    }

    pub fn aurora(message_id: String, aurora: AuroraUpdate, context: Option<Value>) -> Self {
        Self {
            r#type: "aurora".into(),
            aurora: Some(aurora),
            message_id,
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
    ) -> Self {
        Self {
            r#type: "end".into(),
            message_id,
            context,
            finish_reason,
            blocks,
            timestamp,
            ..Default::default()
        }
    }

    pub fn error(message_id: String, context: Option<Value>, error: String) -> Self {
        Self {
            r#type: "error".into(),
            message_id,
            context,
            finish_reason: Some("error".to_string()),
            error: Some(error),
            ..Default::default()
        }
    }
}

/// RAII guard：在 Drop 时自动从 ActiveRequests 中移除对应条目，防止 panic 导致泄漏
pub struct ActiveRequestGuard {
    requests: Arc<ActiveRequestRegistry>,
    message_key: MessageKey,
    epoch: u64,
}

impl ActiveRequestGuard {
    pub fn new(requests: Arc<ActiveRequestRegistry>, message_key: MessageKey, epoch: u64) -> Self {
        Self {
            requests,
            message_key,
            epoch,
        }
    }
}

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        self.requests
            .remove_if_current(&self.message_key, self.epoch);
    }
}

/// 群组回合取消令牌，用于标记需要中断接力赛的话题
/// topicId -> true (存在即代表已取消)
pub struct CancelledGroupTurns(pub Arc<DashSet<String>>);

impl Default for CancelledGroupTurns {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized CancelledGroupTurns successfully.");
        Self(Arc::new(DashSet::new()))
    }
}

/// 中止群组的整个接力赛回合
#[tauri::command]
#[allow(non_snake_case)]
pub fn interruptGroupTurn(
    state: tauri::State<'_, CancelledGroupTurns>,
    topic_id: String,
) -> Result<Value, String> {
    log::info!(
        "[VCPClient] interruptGroupTurn called for topicId: {}",
        topic_id
    );
    state.0.insert(topic_id);
    Ok(json!({"status": "cancelled"}))
}

/// 核心请求函数：sendToVCP
/// 对应 JS 版的 sendToVCP。处理逻辑：
/// 1. 数据验证与规范化 (通过 Rust 类型系统自动处理部分)
/// 2. 动态路由切换 (根据设置注入 /v1/chatvcp/completions)
/// 3. 上下文注入 (音乐信息、UI 规范要求)
/// 4. 发起 HTTP 请求 (支持流式和非流式)
/// 5. 注册 AbortController 实现中止机制
#[tauri::command]
#[allow(non_snake_case)]
pub async fn sendToVCP<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, ActiveRequests>,
    payload: VcpRequestPayload,
    stream_channel: Channel<StreamEvent>,
) -> Result<Value, String> {
    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let request_key = message_key_from_context(context.as_ref(), &message_id)?;
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);

    let (res, is_aborted) =
        match perform_vcp_request(&app, state.0.clone(), payload, Some(stream_channel.clone()))
            .await
        {
            Ok(val) => val,
            Err(e) => {
                if is_stream {
                    let pool = app
                        .state::<crate::vcp_modules::db_manager::DbState>()
                        .pool
                        .clone();
                    let _ = delete_active_generation_for_key(&pool, &request_key).await;
                }
                return Err(e);
            }
        };

    if is_stream {
        let finish_reason = if is_aborted {
            Some("cancelled_by_user".to_string())
        } else {
            res["finishReason"].as_str().map(|s| s.to_string())
        };

        // 从 context 解出 owner_id, owner_type, topic_id 并委派统一终结器
        let ctx = context.as_ref();
        let group_id = ctx.and_then(|c| c["groupId"].as_str());
        let agent_id = ctx.and_then(|c| c["agentId"].as_str());
        let topic_id = ctx
            .and_then(|c| c["topicId"].as_str())
            .unwrap_or("")
            .to_string();

        let (owner_id, owner_type) = if let Some(gid) = group_id {
            (gid, "group")
        } else if let Some(aid) = agent_id {
            (aid, "agent")
        } else {
            ("", "agent")
        };

        let pool = app
            .state::<crate::vcp_modules::db_manager::DbState>()
            .pool
            .clone();

        crate::vcp_modules::chat::message_service::finalize_stream_message(
            app.clone(),
            &pool,
            owner_id,
            owner_type,
            topic_id,
            message_id,
            res["fullContent"].as_str().unwrap_or("").to_string(),
            is_aborted,
            finish_reason,
            Some(stream_channel),
            agent_id.map(|s| s.to_string()),
        )
        .await?;
    }

    Ok(res)
}

async fn load_app_settings<R: Runtime>(app: &AppHandle<R>) -> Result<Settings, String> {
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;

    let row = sqlx::query("SELECT value FROM settings WHERE key = 'global'")
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(row) = row {
        use sqlx::Row;
        let content: String = row.get("value");
        let settings = serde_json::from_str::<Settings>(&content)
            .unwrap_or_else(|_| create_default_settings());
        Ok(settings)
    } else {
        Ok(create_default_settings())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_db_state_returns_retryable_core_not_ready_error() {
        let error = require_core_state::<()>(None).expect_err("missing state must be rejected");

        assert_eq!(error, CORE_NOT_READY_ERROR);
    }

    #[tokio::test]
    async fn active_generation_delete_and_legacy_recovery_are_owner_scoped() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        sqlx::query(
            "CREATE TABLE active_generations (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
            )",
        )
        .execute(&pool)
        .await
        .expect("active generation schema");

        let agent_key =
            registry::message_key_from_parts("owner-a", "agent", "shared-topic", "same-msg")
                .unwrap();
        let group_key =
            registry::message_key_from_parts("owner-g", "group", "shared-topic", "same-msg")
                .unwrap();
        for (key, created_at) in [(&agent_key, 10_i64), (&group_key, 20_i64)] {
            sqlx::query(
                "INSERT INTO active_generations
                 (owner_type, owner_id, topic_id, msg_id, created_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&key.topic.owner_type)
            .bind(&key.topic.owner_id)
            .bind(&key.topic.topic_id)
            .bind(&key.msg_id)
            .bind(created_at)
            .execute(&pool)
            .await
            .expect("active generation row");
        }

        let legacy_error = recovery::resolve_legacy_generation_key(&pool, "same-msg")
            .await
            .expect_err("legacy recovery must reject ambiguous msg id");
        assert!(legacy_error.contains("ambiguous"));

        delete_active_generation_for_key(&pool, &agent_key)
            .await
            .expect("scoped delete");
        let remaining: (String, String, String, String) =
            sqlx::query_as("SELECT owner_type, owner_id, topic_id, msg_id FROM active_generations")
                .fetch_one(&pool)
                .await
                .expect("group generation remains");
        assert_eq!(
            remaining,
            (
                "group".to_string(),
                "owner-g".to_string(),
                "shared-topic".to_string(),
                "same-msg".to_string()
            )
        );
        delete_active_generation_for_key(&pool, &group_key)
            .await
            .expect("second scoped delete");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .expect("count active generations");
        assert_eq!(count, 0);
    }
}
