use dashmap::DashSet;
use serde_json::{json, Value};

use std::sync::Arc;
use tauri::{ipc::Channel, AppHandle, Manager, Runtime};

use crate::vcp_modules::aurora_pipeline::AuroraUpdate;
use crate::vcp_modules::chat::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::db_manager::{DbState, CORE_NOT_READY_ERROR};
use crate::vcp_modules::settings_manager::{create_default_settings, Settings};

#[path = "vcp_client_active.rs"]
mod active;
#[path = "vcp_client_connection.rs"]
mod connection;
#[path = "vcp_client_foreground.rs"]
mod foreground;
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
#[path = "vcp_client_types.rs"]
mod types;
pub(crate) use active::mark_message_as_error_guarded_with_channel;
#[allow(unused_imports)]
pub use active::{get_active_generations, interruptRequest, ActiveGeneration};
pub use connection::test_vcp_connection;
pub use foreground::acquire_stream_service;
use non_stream::handle_non_streaming_request;
pub use preprocess::{perform_vcp_request, VcpRequestError, VcpRequestOutcome};
pub(crate) use recovery::cleanup_recovery_cleanup_debt_on_startup;
pub use recovery::recover_active_generation;
use registry::message_key_from_context;
pub use registry::{ActiveRequestRegistry, ActiveRequests, CompletionLease, GuardedTransition};
pub use resume::resume_stream;
use stream::handle_streaming_request;
pub(crate) use types::VcpRequestMode;
pub use types::{StreamEvent, VcpRequestPayload};

fn require_core_state<T>(state: Option<T>) -> Result<T, String> {
    state.ok_or_else(|| CORE_NOT_READY_ERROR.to_string())
}

fn db_pool_if_ready<R: Runtime>(app: &AppHandle<R>) -> Result<sqlx::Pool<sqlx::Sqlite>, String> {
    let db = require_core_state(app.try_state::<DbState>())?;
    Ok(db.pool.clone())
}

pub(crate) fn require_full_content(response: &Value) -> Result<&str, String> {
    response["fullContent"]
        .as_str()
        .ok_or_else(|| "响应缺少 fullContent".to_string())
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

/// 群组回合取消令牌，以完整群组话题身份隔离并发接力赛。
pub struct CancelledGroupTurns(Arc<DashSet<TopicKey>>);

impl Default for CancelledGroupTurns {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized CancelledGroupTurns successfully.");
        Self(Arc::new(DashSet::new()))
    }
}

impl CancelledGroupTurns {
    pub(crate) fn cancel(&self, key: TopicKey) {
        debug_assert_eq!(key.owner_type, "group");
        self.0.insert(key);
    }

    pub(crate) fn clear(&self, key: &TopicKey) {
        self.0.remove(key);
    }

    pub(crate) fn is_cancelled(&self, key: &TopicKey) -> bool {
        self.0.contains(key)
    }
}

pub(crate) fn group_turn_key(group_id: &str, topic_id: &str) -> Result<TopicKey, String> {
    let key = TopicKey::new("group", group_id, topic_id);
    key.is_valid()
        .then_some(key)
        .ok_or_else(|| "中止群聊回合需要完整的 groupId 和 topicId".to_string())
}

/// 中止群组的整个接力赛回合
#[tauri::command]
#[allow(non_snake_case)]
pub fn interruptGroupTurn(
    state: tauri::State<'_, CancelledGroupTurns>,
    group_id: String,
    topic_id: String,
) -> Result<Value, String> {
    let key = group_turn_key(&group_id, &topic_id)?;
    log::info!(
        "[VCPClient] interruptGroupTurn called for groupId/topicId: {}/{}",
        group_id,
        topic_id,
    );
    state.cancel(key);
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
    let _request_key = message_key_from_context(context.as_ref(), &message_id)?;
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);

    let outcome =
        match perform_vcp_request(&app, state.0.clone(), payload, Some(stream_channel.clone()))
            .await
        {
            Ok(val) => val,
            Err(error) => {
                return handle_vcp_request_error(
                    &app,
                    is_stream,
                    error,
                    Some(stream_channel.clone()),
                )
                .await
            }
        };
    let preprocess::VcpRequestOutcome {
        response,
        is_aborted,
        completion_lease,
        request_guard,
    } = outcome;
    let mut res = response;
    // Keep registry cleanup alive until the caller-side finalizer completes.
    let _request_guard = request_guard;

    if is_stream {
        let pool = db_pool_if_ready(&app)?;
        finalize_vcp_stream(
            &app,
            &pool,
            &completion_lease,
            &mut res,
            is_aborted,
            context.as_ref(),
            stream_channel,
        )
        .await?;
    }

    drop(completion_lease);
    Ok(res)
}

async fn handle_vcp_request_error<R: Runtime>(
    app: &AppHandle<R>,
    is_stream: bool,
    error: VcpRequestError,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<Value, String> {
    let VcpRequestError {
        message,
        completion_lease,
        stale,
        request_guard,
    } = error;
    let message = message.clone();
    let _request_guard = request_guard;
    if stale {
        drop(completion_lease);
        return Ok(json!({"status": "skipped", "finalization": "skipped"}));
    }
    if is_stream {
        if let Some(lease) = completion_lease.as_ref() {
            let pool = db_pool_if_ready(app)?;
            let mark_result = active::mark_message_as_error_guarded_with_channel(
                app,
                &pool,
                lease,
                Some(message.clone()),
                stream_channel,
            )
            .await?;
            if matches!(mark_result, GuardedTransition::Skipped) {
                log::warn!("[VCPClient] 请求错误终结已跳过旧请求");
                drop(completion_lease);
                return Ok(json!({"status": "skipped", "finalization": "skipped"}));
            }
        }
    }
    drop(completion_lease);
    Err(message)
}

async fn finalize_vcp_stream<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    completion_lease: &CompletionLease,
    response: &mut Value,
    is_aborted: bool,
    context: Option<&Value>,
    stream_channel: Channel<StreamEvent>,
) -> Result<(), String> {
    let finish_reason = if is_aborted {
        Some("cancelled_by_user".to_string())
    } else {
        response["finishReason"].as_str().map(str::to_string)
    };
    let full_content = match require_full_content(response) {
        Ok(content) => content,
        Err(error) => {
            let mark_result = active::mark_message_as_error_guarded_with_channel(
                app,
                pool,
                completion_lease,
                Some("响应缺少 fullContent".to_string()),
                Some(stream_channel.clone()),
            )
            .await?;
            if matches!(mark_result, GuardedTransition::Skipped) {
                response["finalization"] = json!("skipped");
                return Ok(());
            }
            return Err(error);
        }
    };
    let agent_id = context.and_then(|value| value["agentId"].as_str());
    let finalization = crate::vcp_modules::chat::message_service::finalize_stream_message_guarded(
        app.clone(),
        pool,
        completion_lease,
        full_content.to_string(),
        is_aborted,
        finish_reason,
        Some(stream_channel),
        agent_id.map(str::to_string),
    )
    .await?;
    if matches!(
        finalization,
        crate::vcp_modules::chat::message_service::StreamFinalizationStatus::Skipped
    ) {
        response["finalization"] = json!("skipped");
    }
    Ok(())
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
#[path = "vcp_client_tests.rs"]
mod tests;
