use serde_json::{json, Value};
use sqlx::{Pool, Sqlite};

use super::topic_sponsor_support::{
    find_topic, first_message_name, get_string_arg, get_topic_id_arg, load_messages, load_topics,
    AgentInfo,
};

type SqlitePool = Pool<Sqlite>;

pub(super) async fn handle_check_topic_ownership(
    pool: &SqlitePool,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let topic_id =
        get_topic_id_arg(args).ok_or_else(|| "请求中缺少 'topic_id' 参数。".to_string())?;
    let caller_name = required_arg(args, "caller_name")?;
    let topic = find_topic(pool, &agent.id, &topic_id).await?;
    let creator_name = first_message_name(pool, &agent.id, &topic_id)
        .await?
        .unwrap_or_else(|| "unknown".to_string());

    Ok(json!({
        "status": "success",
        "result": {
            "is_owner": creator_name == caller_name,
            "creator_name": creator_name,
            "topic_name": topic.name
        }
    }))
}

pub(super) async fn handle_list_unlocked_topics(
    pool: &SqlitePool,
    agent: &AgentInfo,
) -> Result<Value, String> {
    let topics = load_topics(pool, &agent.id, Some(false), None).await?;
    let mapped: Vec<Value> = topics
        .iter()
        .map(|topic| {
            json!({
                "topic_id": topic.id,
                "topic_name": topic.name,
                "locked": topic.locked,
                "unread": topic.unread,
                "created_at": topic.created_at,
                "message_count": topic.msg_count
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "agent_id": agent.id,
            "has_unlocked_topics": !mapped.is_empty(),
            "unlocked_topics_count": mapped.len(),
            "topics": mapped
        }
    }))
}

pub(super) async fn handle_read_topic_content(
    pool: &SqlitePool,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let topic_id =
        get_topic_id_arg(args).ok_or_else(|| "请求中缺少 'topic_id' 参数。".to_string())?;
    let topic = find_topic(pool, &agent.id, &topic_id).await?;
    let messages = load_messages(pool, &agent.id, &topic_id).await?;

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "agent_id": agent.id,
            "topic_id": topic.id,
            "topic_name": topic.name,
            "topic_info": {
                "locked": topic.locked,
                "unread": topic.unread,
                "created_at": topic.created_at
            },
            "message_count": messages.len(),
            "messages": messages
        }
    }))
}

fn required_arg(args: &Value, key: &str) -> Result<String, String> {
    get_string_arg(args, key).ok_or_else(|| format!("请求中缺少 '{}' 参数。", key))
}
