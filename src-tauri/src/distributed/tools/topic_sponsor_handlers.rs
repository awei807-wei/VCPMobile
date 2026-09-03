use serde_json::{json, Value};
use sqlx::{Pool, Sqlite};
use tauri::AppHandle;

use crate::vcp_modules::chat::topic_types::TopicKey;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::message_repository::{MessageRenderCompiler, MessageRepository};
use crate::vcp_modules::sync_hash::HashAggregator;

use super::topic_sponsor_support::{
    find_topic, get_bool_arg, get_i64_arg, get_string_arg, get_topic_id_arg, load_messages,
    load_topics, AgentInfo,
};
use super::{MAX_CHECK_NEW_TOPICS_DAYS, MILLIS_PER_DAY};

type SqlitePool = Pool<Sqlite>;

pub(super) async fn handle_create_topic(
    _app: &AppHandle,
    pool: &SqlitePool,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let topic_name = required_arg(args, "topic_name")?;
    let initial_message = required_arg(args, "initial_message")?;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let topic_id = format!("topic_{}_{}", now, uuid::Uuid::new_v4().simple());
    let msg_id = format!("msg_{}_assistant_{}", now, uuid::Uuid::new_v4().simple());
    let topic_key = TopicKey::new("agent", &agent.id, &topic_id);
    let chat_message = build_chat_message(
        msg_id.clone(),
        agent,
        topic_id.clone(),
        initial_message.clone(),
        now,
    );

    persist_new_topic(pool, &topic_key, &chat_message, &topic_name, now).await?;
    Ok(json!({
        "status": "success",
        "result": {
            "message": format!("成功创建了新的话题：{}", topic_name),
            "topic_id": topic_id,
            "topic_name": topic_name,
            "agent_name": agent.name,
            "agent_id": agent.id,
            "initial_message": initial_message,
            "message_id": msg_id
        }
    }))
}

async fn persist_new_topic(
    pool: &SqlitePool,
    topic_key: &TopicKey,
    chat_message: &ChatMessage,
    topic_name: &str,
    now: i64,
) -> Result<(), String> {
    let blocks = MessageRenderCompiler::compile(&chat_message.content);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO topics (
            topic_id, owner_id, owner_type, title, created_at, updated_at,
            msg_count, locked, unread, unread_count
         ) VALUES (?, ?, 'agent', ?, ?, ?, 0, 0, 1, 1)",
    )
    .bind(&topic_key.topic_id)
    .bind(&topic_key.owner_id)
    .bind(topic_name)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    MessageRepository::upsert_message_for_topic(
        &mut tx,
        chat_message,
        topic_key,
        &render_bytes,
        true,
    )
    .await?;
    set_topic_message_count(&mut tx, topic_key, 1).await?;
    HashAggregator::bubble_from_topic_for_key(&mut tx, topic_key).await?;
    tx.commit().await.map_err(|e| e.to_string())
}

async fn set_topic_message_count(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
    count: i32,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics SET msg_count = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(count)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn build_chat_message(
    id: String,
    agent: &AgentInfo,
    topic_id: String,
    content: String,
    now: i64,
) -> ChatMessage {
    ChatMessage {
        id,
        role: "assistant".to_string(),
        name: Some(agent.name.clone()),
        content,
        timestamp: now as u64,
        updated_at: Some(now as u64),
        is_thinking: Some(false),
        agent_id: Some(agent.id.clone()),
        group_id: None,
        topic_id: Some(topic_id),
        is_group_message: Some(false),
        finish_reason: Some("completed".to_string()),
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: None,
    }
}

pub(super) async fn handle_read_unlocked_topics(
    pool: &SqlitePool,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let include_read = get_bool_arg(args, "include_read").unwrap_or(false);
    let mut topics = load_topics(pool, &agent.id, Some(false), None).await?;
    if !include_read {
        topics.retain(|topic| topic.unread);
    }

    let mut topics_with_messages = Vec::new();
    for topic in topics {
        let messages = load_messages(pool, &agent.id, &topic.id).await?;
        topics_with_messages.push(json!({
            "topic_id": topic.id,
            "topic_name": topic.name,
            "locked": topic.locked,
            "unread": topic.unread,
            "created_at": topic.created_at,
            "message_count": messages.len(),
            "messages": messages
        }));
    }

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "agent_id": agent.id,
            "topics": topics_with_messages,
            "total_topics": topics_with_messages.len()
        }
    }))
}

pub(super) async fn handle_check_new_topics(
    pool: &SqlitePool,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let days = get_i64_arg(args, "days")
        .unwrap_or(3)
        .clamp(1, MAX_CHECK_NEW_TOPICS_DAYS);
    let cutoff = chrono::Utc::now()
        .timestamp_millis()
        .saturating_sub(days.saturating_mul(MILLIS_PER_DAY));
    let topics = load_topics(pool, &agent.id, Some(false), Some(cutoff)).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let mapped: Vec<Value> = topics
        .iter()
        .map(|topic| {
            json!({
                "topic_id": topic.id,
                "topic_name": topic.name,
                "created_at": topic.created_at,
                "age_hours": now.saturating_sub(topic.created_at) as f64 / 3_600_000.0,
                "locked": topic.locked
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "has_new_topics": !mapped.is_empty(),
            "new_topics_count": mapped.len(),
            "topics": mapped
        }
    }))
}

pub(super) async fn handle_check_unread_messages(
    pool: &SqlitePool,
    agent: &AgentInfo,
) -> Result<Value, String> {
    let topics = load_topics(pool, &agent.id, None, None)
        .await?
        .into_iter()
        .filter(|topic| topic.unread)
        .collect::<Vec<_>>();
    let mut unread_topics = Vec::new();
    for topic in topics {
        let last_message_time = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT timestamp FROM messages
             WHERE owner_type = 'agent' AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NULL
             ORDER BY timestamp DESC, msg_id DESC LIMIT 1",
        )
        .bind(&agent.id)
        .bind(&topic.id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?
        .flatten()
        .unwrap_or(topic.created_at);

        unread_topics.push(json!({
            "topic_id": topic.id,
            "topic_name": topic.name,
            "locked": topic.locked,
            "unread": topic.unread,
            "last_message_time": last_message_time
        }));
    }

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "has_unread": !unread_topics.is_empty(),
            "unread_topics": unread_topics
        }
    }))
}

pub(super) async fn handle_reply_to_topic(
    _app: &AppHandle,
    pool: &SqlitePool,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let topic_id =
        get_topic_id_arg(args).ok_or_else(|| "请求中缺少 'topic_id' 参数。".to_string())?;
    let message = required_arg(args, "message")?;
    let sender_name = get_string_arg(args, "sender_name").unwrap_or_else(|| agent.name.clone());
    let topic = find_topic(pool, &agent.id, &topic_id).await?;
    validate_reply_target(&topic, &topic_id)?;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let chat_message = build_chat_message(
        format!("msg_{}_assistant_{}", now, uuid::Uuid::new_v4().simple()),
        &AgentInfo {
            id: agent.id.clone(),
            name: sender_name.clone(),
        },
        topic_id.clone(),
        message,
        now,
    );

    persist_topic_reply(pool, &chat_message, &topic, now).await?;
    let topic = find_topic(pool, &agent.id, &topic_id).await?;
    Ok(json!({
        "status": "success",
        "result": {
            "message": format!("成功在 {} 的话题 \"{}\" 中添加回复。", agent.name, topic.name),
            "topic_id": topic_id,
            "topic_name": topic.name,
            "sender": sender_name,
            "message_id": chat_message.id,
            "timestamp": now,
            "agent_name": agent.name,
            "agent_id": agent.id
        }
    }))
}

fn validate_reply_target(
    topic: &super::topic_sponsor_support::TopicRow,
    topic_id: &str,
) -> Result<(), String> {
    if topic.locked && !topic.unread {
        return Err(format!(
            "话题 {} 已锁定且未标记为未读，无法添加回复。",
            topic_id
        ));
    }
    Ok(())
}

async fn persist_topic_reply(
    pool: &SqlitePool,
    chat_message: &ChatMessage,
    topic: &super::topic_sponsor_support::TopicRow,
    now: i64,
) -> Result<(), String> {
    let topic_key = TopicKey::new(&topic.owner_type, &topic.owner_id, &topic.id);
    let blocks = MessageRenderCompiler::compile(&chat_message.content);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message_for_topic(
        &mut tx,
        chat_message,
        &topic_key,
        &render_bytes,
        true,
    )
    .await?;
    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or(0);
    update_reply_topic(&mut tx, &topic_key, msg_count, now).await?;
    HashAggregator::bubble_from_topic_for_key(&mut tx, &topic_key).await?;
    tx.commit().await.map_err(|e| e.to_string())
}

async fn update_reply_topic(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
    msg_count: i32,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics
         SET unread = 1, unread_count = unread_count + 1, msg_count = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(msg_count)
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn required_arg(args: &Value, key: &str) -> Result<String, String> {
    get_string_arg(args, key).ok_or_else(|| format!("请求中缺少 '{}' 参数。", key))
}
