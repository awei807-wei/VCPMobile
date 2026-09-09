use super::{
    ActiveRequestGuard, ActiveRequests, CompletionLease, PreparedResumeRequest, StreamEvent,
};
use serde_json::{json, Value};
use tauri::{ipc::Channel, AppHandle, Runtime};
use tokio::sync::oneshot;

pub(super) async fn prepare_resume_request<R: Runtime>(
    app: &AppHandle<R>,
    state: &tauri::State<'_, ActiveRequests>,
    request_key: &crate::vcp_modules::chat::topic_types::MessageKey,
    initial_content: Option<&str>,
    stream_channel: &Channel<StreamEvent>,
    expected_generation: u64,
) -> Result<Option<PreparedResumeRequest>, String> {
    let pool = super::db_pool_if_ready(app)?;
    #[cfg(target_os = "android")]
    super::transport::prepare_resume_with_helper(app, request_key, expected_generation).await?;
    let (abort_rx, request_epoch, guard, completion_lease) =
        match register_resume_request(state, request_key, expected_generation).await {
            Ok(value) => value,
            Err(error) => {
                return Err(fail_preparation(app, request_key, expected_generation, error).await)
            }
        };
    let Some(_initial_status) = persist_initial_resume_content(
        app,
        &pool,
        request_key,
        expected_generation,
        initial_content,
        &completion_lease,
    )
    .await?
    else {
        return Ok(None);
    };
    let client = match reqwest::Client::builder().build() {
        Ok(client) => client,
        Err(error) => {
            let error = error.to_string();
            return Err(fail_preparation(app, request_key, expected_generation, error).await);
        }
    };
    let context = build_resume_context(
        &request_key.topic.owner_id,
        &request_key.topic.owner_type,
        &request_key.topic.topic_id,
    );
    if let Err(error) =
        send_resume_thinking(stream_channel, &request_key.msg_id, &context, request_epoch)
    {
        return Err(fail_preparation(app, request_key, expected_generation, error).await);
    }
    Ok(Some(PreparedResumeRequest::new(
        pool,
        abort_rx,
        request_epoch,
        guard,
        completion_lease,
        client,
        context,
    )))
}

impl PreparedResumeRequest {
    fn new(
        pool: sqlx::Pool<sqlx::Sqlite>,
        abort_rx: oneshot::Receiver<()>,
        request_epoch: u64,
        guard: ActiveRequestGuard,
        completion_lease: CompletionLease,
        client: reqwest::Client,
        context: Value,
    ) -> Self {
        Self {
            pool,
            abort_rx,
            request_epoch,
            _guard: guard,
            completion_lease,
            client,
            context,
        }
    }
}

async fn persist_initial_resume_content<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    request_key: &crate::vcp_modules::chat::topic_types::MessageKey,
    expected_generation: u64,
    initial_content: Option<&str>,
    completion_lease: &CompletionLease,
) -> Result<Option<crate::vcp_modules::chat::message_service::StreamFinalizationStatus>, String> {
    let status =
        match super::persist_initial_content(app, pool, initial_content, completion_lease).await {
            Ok(status) => status,
            Err(error) => {
                return Err(fail_preparation(app, request_key, expected_generation, error).await)
            }
        };
    if matches!(
        status,
        crate::vcp_modules::chat::message_service::StreamFinalizationStatus::Skipped
    ) {
        let _ = fail_preparation(
            app,
            request_key,
            expected_generation,
            "初始内容持久化已跳过接续".to_string(),
        )
        .await;
        return Ok(None);
    }
    Ok(Some(status))
}

async fn register_resume_request(
    state: &tauri::State<'_, ActiveRequests>,
    key: &crate::vcp_modules::chat::topic_types::MessageKey,
    expected_generation: u64,
) -> Result<
    (
        oneshot::Receiver<()>,
        u64,
        ActiveRequestGuard,
        CompletionLease,
    ),
    String,
> {
    let (abort_tx, abort_rx) = oneshot::channel();
    let (request_epoch, previous_sender, completion_lease) = state
        .0
        .register_for_generation(key.clone(), abort_tx, expected_generation)
        .await?;
    if let Some(previous_sender) = previous_sender {
        let _ = previous_sender.send(());
    }
    let guard = ActiveRequestGuard::new(state.0.clone(), key.clone(), request_epoch);
    Ok((abort_rx, request_epoch, guard, completion_lease))
}

pub(super) fn build_resume_context(owner_id: &str, owner_type: &str, topic_id: &str) -> Value {
    json!({
        "topicId": topic_id,
        "ownerType": owner_type,
        "ownerId": owner_id,
        "groupId": if owner_type == "group" { Some(owner_id) } else { None::<&str> },
        "agentId": if owner_type == "agent" { Some(owner_id) } else { None::<&str> },
    })
}

fn send_resume_thinking(
    stream_channel: &Channel<StreamEvent>,
    msg_id: &str,
    context: &Value,
    request_epoch: u64,
) -> Result<(), String> {
    stream_channel
        .send(StreamEvent::thinking(
            msg_id.to_string(),
            Some(context.clone()),
            request_epoch,
        ))
        .map_err(|error| format!("发送接续流 thinking 事件失败: {error}"))
}

#[allow(unused_variables)]
async fn fail_preparation<R: Runtime>(
    app: &AppHandle<R>,
    request_key: &crate::vcp_modules::chat::topic_types::MessageKey,
    expected_generation: u64,
    error: String,
) -> String {
    #[cfg(target_os = "android")]
    cancel_prepared_resume(app, request_key, expected_generation, &error).await;
    error
}

#[cfg(target_os = "android")]
pub(super) async fn cancel_prepared_resume<R: Runtime>(
    app: &AppHandle<R>,
    request_key: &crate::vcp_modules::chat::topic_types::MessageKey,
    expected_generation: u64,
    primary_error: &str,
) {
    if let Err(cancel_error) =
        super::transport::cancel_resume_with_helper(app, request_key, expected_generation).await
    {
        log::error!(
            "[VCPClient] 接续准备失败后的 helper cancel_resume 失败: messageId={}, generation={}, primary_error={}, cancel_error={}",
            request_key.msg_id,
            expected_generation,
            primary_error,
            cancel_error
        );
    }
}
