use sqlx::Row;
use tauri::{AppHandle, Manager};

use crate::vcp_modules::context_sanitizer;
use crate::vcp_modules::message_repository::ContentCompressor;
use crate::vcp_modules::settings_manager::{self, SettingsState};

#[derive(Debug, Clone)]
pub(super) struct AgentInfo {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Debug, Clone)]
struct TopicInfo {
    id: String,
    title: String,
    created_at: i64,
    locked: bool,
    msg_count: i32,
}

#[derive(Debug, Clone)]
struct MessageInfo {
    role: String,
    name: Option<String>,
    content: String,
}

pub(super) fn get_string_arg(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn get_topic_id_arg(args: &serde_json::Value) -> Option<String> {
    ["topic_id", "topicId", "TopicId"]
        .iter()
        .find_map(|key| get_string_arg(args, key))
}

pub(super) async fn find_agent_info(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    maid_name: &str,
) -> Result<AgentInfo, String> {
    let row = sqlx::query(
        "SELECT agent_id, name
         FROM agents
         WHERE deleted_at IS NULL AND name = ?
         ORDER BY updated_at DESC
         LIMIT 1",
    )
    .bind(maid_name)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    let row = match row {
        Some(row) => row,
        None => {
            let pattern = format!("%{}%", escape_like_pattern(maid_name));
            sqlx::query(
                "SELECT agent_id, name
                 FROM agents
                 WHERE deleted_at IS NULL AND name LIKE ? ESCAPE '\\'
                 ORDER BY updated_at DESC
                 LIMIT 1",
            )
            .bind(pattern)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("未找到名为 \"{}\" 的智能体。", maid_name))?
        }
    };

    Ok(AgentInfo {
        id: row.get("agent_id"),
        name: row.get("name"),
    })
}

pub(super) fn escape_like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

pub(super) async fn find_user_name(app: &AppHandle) -> String {
    if let Some(settings_state) = app.try_state::<SettingsState>() {
        if let Ok(settings) = settings_manager::read_settings(app.clone(), settings_state).await {
            if !settings.user_name.trim().is_empty() {
                return settings.user_name;
            }
        }
    }
    "用户".to_string()
}

async fn load_topics(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent_id: &str,
) -> Result<Vec<TopicInfo>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, msg_count
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL
         ORDER BY updated_at DESC, created_at DESC",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|row| TopicInfo {
            id: row.get("topic_id"),
            title: row.get("title"),
            created_at: row.get("created_at"),
            locked: row.get::<i32, _>("locked") != 0,
            msg_count: row.get("msg_count"),
        })
        .collect())
}

pub(super) async fn list_topics(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
) -> Result<String, String> {
    let topics = load_topics(pool, &agent.id).await?;

    if topics.is_empty() {
        return Ok(format!("[TopicMemo] {} 暂无任何话题记录。", agent.name));
    }

    let mut result = format!("## {} 的话题列表\n\n", agent.name);
    result.push_str(&format!("共 {} 个话题：\n\n", topics.len()));

    for (index, topic) in topics.iter().enumerate() {
        let locked_tag = if topic.locked { " 🔒" } else { "" };
        result.push_str(&format!(
            "{}. **{}**{}\n",
            index + 1,
            topic.title,
            locked_tag
        ));
        result.push_str(&format!("   - ID: `{}`\n", topic.id));
        result.push_str(&format!(
            "   - 创建时间: {}\n",
            format_timestamp(topic.created_at)
        ));
        result.push_str(&format!("   - 消息数量: {} 条\n\n", topic.msg_count));
    }

    Ok(result)
}

pub(super) async fn get_topic_content(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    topic_id: &str,
    user_name: &str,
) -> Result<String, String> {
    let topic = load_topic(pool, agent, topic_id).await?;
    let messages = load_topic_messages(pool, agent, topic_id).await?;
    Ok(render_topic_content(
        &topic,
        messages,
        user_name,
        &agent.name,
    ))
}

async fn load_topic(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    topic_id: &str,
) -> Result<TopicInfo, String> {
    let row = sqlx::query(
        "SELECT topic_id, title, created_at, locked, msg_count
         FROM topics
         WHERE owner_type = 'agent'
           AND owner_id = ?
           AND topic_id = ?
           AND deleted_at IS NULL
         LIMIT 1",
    )
    .bind(&agent.id)
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| {
        format!(
            "未找到 ID 为 \"{}\" 的话题。可用的话题ID请先使用 ListTopics 指令查询。",
            topic_id
        )
    })?;

    Ok(TopicInfo {
        id: row.get("topic_id"),
        title: row.get("title"),
        created_at: row.get("created_at"),
        locked: row.get::<i32, _>("locked") != 0,
        msg_count: row.get("msg_count"),
    })
}

async fn load_topic_messages(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    topic_id: &str,
) -> Result<Vec<MessageInfo>, String> {
    let rows = sqlx::query(
        "SELECT role, name, content
         FROM messages
         WHERE owner_type = 'agent'
           AND owner_id = ?
           AND topic_id = ?
           AND deleted_at IS NULL
         ORDER BY timestamp ASC, msg_id ASC",
    )
    .bind(&agent.id)
    .bind(topic_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    rows.into_iter()
        .map(|row| {
            let content_bytes: Vec<u8> = row.get("content");
            let content = ContentCompressor::decompress(&content_bytes)
                .map_err(|e| format!("话题 {} 的消息内容解压失败: {}", topic_id, e))?;
            Ok(MessageInfo {
                role: row.get("role"),
                name: row.get("name"),
                content,
            })
        })
        .collect()
}

fn render_topic_content(
    topic: &TopicInfo,
    messages: Vec<MessageInfo>,
    user_name: &str,
    agent_name: &str,
) -> String {
    if messages.is_empty() {
        return format!("## 话题：{}\n\n该话题暂无聊天记录。", topic.title);
    }

    let mut result = format!("## 话题：{}\n", topic.title);
    result.push_str(&format!(
        "创建时间：{}\n",
        format_timestamp(topic.created_at)
    ));
    result.push_str(&format!("消息数量：{} 条\n\n", messages.len()));
    result.push_str("---\n\n");

    for message in messages {
        let speaker_name = speaker_name(&message, user_name, agent_name);
        let clean_content = clean_message_content(&message.content);
        if !clean_content.is_empty() {
            result.push_str(&format!("**{}**: {}\n\n", speaker_name, clean_content));
        }
    }

    result
}

fn speaker_name(message: &MessageInfo, user_name: &str, agent_name: &str) -> String {
    if message.role == "user" {
        user_name.to_string()
    } else {
        message
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(agent_name)
            .to_string()
    }
}

pub(super) fn clean_message_content(content: &str) -> String {
    let without_executable_blocks = strip_executable_html_blocks(content);
    let cleaned_source = without_executable_blocks.as_deref().unwrap_or(content);

    if !context_sanitizer::contains_html(cleaned_source) {
        return cleaned_source.trim().to_string();
    }

    context_sanitizer::html_to_vcp_markdown(cleaned_source, false)
        .replace(['\r', '\t'], " ")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_executable_html_blocks(content: &str) -> Option<String> {
    let lower_content = content.to_ascii_lowercase();
    if !(lower_content.contains("<script") || lower_content.contains("<style")) {
        return None;
    }

    let mut output = String::with_capacity(content.len());
    let mut rest = content;

    while let Some(start) = find_script_or_style_start(rest) {
        output.push_str(&rest[..start]);

        let tag_end = match rest[start..].find('>') {
            Some(end) => start + end + 1,
            None => return Some(output),
        };
        let lower_tag_start = rest[start..].to_ascii_lowercase();
        let tag = if lower_tag_start.starts_with("<script") {
            "script"
        } else {
            "style"
        };
        let close_tag = format!("</{}>", tag);
        let lower_after_open = rest[tag_end..].to_ascii_lowercase();

        if let Some(close_start) = lower_after_open.find(&close_tag) {
            let close_end = tag_end + close_start + close_tag.len();
            rest = &rest[close_end..];
        } else {
            return Some(output);
        }
    }

    output.push_str(rest);
    Some(output)
}

fn find_script_or_style_start(input: &str) -> Option<usize> {
    let lower = input.to_ascii_lowercase();
    match (lower.find("<script"), lower.find("<style")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn format_timestamp(timestamp_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y/%m/%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| timestamp_ms.to_string())
}
