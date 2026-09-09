use super::{create_topic, update_topic_title};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::topic_types::TopicKey;
use serde::Deserialize;
use sqlx::Row;
use tauri::{AppHandle, Manager, State};

#[derive(Deserialize, Clone, Debug)]
pub struct TempMessage {
    pub role: String,
    pub name: Option<String>,
    pub content: String,
    pub timestamp: u64,
}

#[tauri::command]
pub async fn archive_assistant_chat(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    temp_messages: Vec<TempMessage>,
) -> Result<String, String> {
    if temp_messages.is_empty() {
        return Err("No messages to archive".to_string());
    }

    let now_millis = crate::vcp_modules::infra::utils::now_millis();
    let default_title = format!("划词助手 {}", chrono::Local::now().format("%m-%d %H:%M"));
    let topic = create_topic(
        app_handle.clone(),
        db_state.clone(),
        owner_id.clone(),
        owner_type.clone(),
        default_title,
    )
    .await?;
    let topic_key = TopicKey::new(owner_type.clone(), owner_id.clone(), topic.id.clone());
    write_archive_messages(
        &db_state.pool,
        &topic_key,
        &owner_type,
        &owner_id,
        &topic.id,
        &temp_messages,
        now_millis,
    )
    .await?;
    update_archive_count(&db_state.pool, &topic_key, temp_messages.len()).await?;
    spawn_archive_title_task(app_handle, owner_id, owner_type, topic.id.clone());
    Ok(topic.id)
}

async fn write_archive_messages(
    pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    temp_messages: &[TempMessage],
    now_millis: i64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    for (index, temp_msg) in temp_messages.iter().enumerate() {
        let chat_msg =
            archive_chat_message(owner_type, owner_id, topic_id, temp_msg, now_millis, index);
        let blocks =
            crate::vcp_modules::persistence::message_repository::MessageRenderCompiler::compile(
                &temp_msg.content,
            );
        let render_content =
            crate::vcp_modules::persistence::message_repository::MessageRenderCompiler::serialize(
                &blocks,
            )?;
        crate::vcp_modules::persistence::message_repository::MessageRepository::upsert_message_for_topic(
            &mut tx,
            &chat_msg,
            topic_key,
            &render_content,
            true,
        )
        .await?;
    }
    crate::vcp_modules::sync_hash::HashAggregator::bubble_from_topic_for_key(&mut tx, topic_key)
        .await?;
    tx.commit().await.map_err(|e| e.to_string())
}

fn archive_chat_message(
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    temp_msg: &TempMessage,
    now_millis: i64,
    index: usize,
) -> ChatMessage {
    ChatMessage {
        id: format!("assistant_msg_{now_millis}_{index}"),
        role: temp_msg.role.clone(),
        name: temp_msg.name.clone(),
        content: temp_msg.content.clone(),
        timestamp: temp_msg.timestamp,
        updated_at: Some(temp_msg.timestamp),
        is_thinking: Some(false),
        agent_id: (owner_type == "agent").then(|| owner_id.to_string()),
        group_id: (owner_type == "group").then(|| owner_id.to_string()),
        topic_id: Some(topic_id.to_string()),
        is_group_message: Some(owner_type == "group"),
        finish_reason: None,
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: None,
    }
}

async fn update_archive_count(
    pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    message_count: usize,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics SET msg_count = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(message_count as i32)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn spawn_archive_title_task(
    app_handle: AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
) {
    let pool = app_handle.state::<DbState>().pool.clone();
    tauri::async_runtime::spawn(async move {
        let agent_name = archive_agent_name(&pool, &owner_type, &owner_id).await;
        let Ok(title) = crate::vcp_modules::chat::topic_summary_service::summarize_topic(
            app_handle.clone(),
            app_handle.state::<SettingsState>(),
            owner_id.clone(),
            owner_type.clone(),
            topic_id.clone(),
            agent_name,
        )
        .await
        else {
            return;
        };
        let _ = update_topic_title(
            app_handle.clone(),
            app_handle.state::<DbState>(),
            owner_id,
            owner_type,
            topic_id,
            title,
        )
        .await;
    });
}

async fn archive_agent_name(pool: &sqlx::SqlitePool, owner_type: &str, owner_id: &str) -> String {
    if owner_type != "agent" {
        return "Group".to_string();
    }
    sqlx::query("SELECT name FROM agents WHERE agent_id = ?")
        .bind(owner_id)
        .fetch_one(pool)
        .await
        .map(|row| row.get::<String, _>("name"))
        .unwrap_or_else(|_| "Agent".to_string())
}
