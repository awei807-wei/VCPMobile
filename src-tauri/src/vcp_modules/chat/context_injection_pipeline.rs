use chrono::{Local, TimeZone};
use log::warn;
use serde_json::Value;
use sqlx::{Pool, Row, Sqlite};

use super::context_injection_rules::{fetch_active_rules, TarvenRule};

struct EnvironmentMetadata {
    now: String,
    created_at: Option<String>,
}

fn render_rule_content(rule: &TarvenRule) -> String {
    if rule.wrap {
        format!(
            "<vcp_injection description=\"由 VCPMobile 注入\">\n{}\n</vcp_injection>",
            rule.content
        )
    } else {
        rule.content.clone()
    }
}

pub async fn apply_tarven_pipeline(
    pool: &Pool<Sqlite>,
    owner_id: &str,
    topic_id: &str,
    agent_name: &str,
    scope: &str,
    messages: &mut Vec<Value>,
) -> Result<(), String> {
    validate_topic_identity(owner_id, topic_id, scope)?;
    let rules = fetch_active_rules(pool, scope).await?;
    let metadata = if has_system_metadata_rule(&rules) {
        Some(load_environment_metadata(pool, owner_id, scope, topic_id).await)
    } else {
        None
    };
    apply_rule_pipeline(messages, &rules, agent_name, metadata.as_ref(), false);
    Ok(())
}

fn validate_topic_identity(owner_id: &str, topic_id: &str, scope: &str) -> Result<(), String> {
    if owner_id.is_empty() || topic_id.is_empty() || !matches!(scope, "agent" | "group") {
        return Err("Tavern pipeline requires a complete topic owner identity".to_string());
    }
    Ok(())
}

fn has_system_metadata_rule(rules: &[TarvenRule]) -> bool {
    rules.iter().any(|rule| rule.id == "system_meta_injection")
}

async fn load_environment_metadata(
    pool: &Pool<Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
) -> EnvironmentMetadata {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string();
    let created_at = sqlx::query(
        "SELECT created_at FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(owner_type)
    .bind(owner_id)
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_or_else(
        |error| {
            warn!(
                "Failed to fetch topic created_at for topic_id {}: {:?}",
                topic_id, error
            );
            None
        },
        |row| row.and_then(|row| format_created_at(row.get("created_at"))),
    );
    EnvironmentMetadata { now, created_at }
}

fn format_created_at(timestamp: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(timestamp)
        .single()
        .map(|datetime| datetime.format("%Y-%m-%d %H:%M:%S %Z").to_string())
}

fn format_system_metadata(metadata: &EnvironmentMetadata, system_prompt: &mut String) {
    let mut rendered = format!(
        "<system_metadata>\n\
         - 当前系统时间: {}\n\
         - 运行环境: VCPMobile v{} (Android 移动端)\n",
        metadata.now,
        env!("CARGO_PKG_VERSION")
    );
    if let Some(created_at) = metadata.created_at.as_deref() {
        rendered.push_str(&format!("- 当前话题创建于: {}\n", created_at));
    }
    rendered.push_str("</system_metadata>\n\n");
    let original_prompt = std::mem::take(system_prompt);
    *system_prompt = format!("{}{}", rendered, original_prompt);
}

fn apply_rule_pipeline(
    messages: &mut Vec<Value>,
    rules: &[TarvenRule],
    agent_name: &str,
    metadata: Option<&EnvironmentMetadata>,
    insert_empty_system: bool,
) {
    apply_system_rules(messages, rules, agent_name, metadata, insert_empty_system);
    apply_user_suffix_rules(messages, rules);
    apply_context_rules(messages, rules);
}

fn apply_system_rules(
    messages: &mut Vec<Value>,
    rules: &[TarvenRule],
    agent_name: &str,
    metadata: Option<&EnvironmentMetadata>,
    insert_empty_system: bool,
) {
    let system_index = messages
        .iter()
        .position(|message| message["role"].as_str() == Some("system"));
    let mut system_content = system_index
        .and_then(|index| messages[index]["content"].as_str())
        .unwrap_or("")
        .to_string();
    if let Some(metadata) = metadata {
        format_system_metadata(metadata, &mut system_content);
    }

    let system_rules = rules
        .iter()
        .filter(|rule| rule.rule_type == "system_suffix");
    append_positioned_rules(&mut system_content, system_rules);
    let system_content = system_content
        .replace("{{AgentName}}", agent_name)
        .replace("{{VCPChatAgentName}}", agent_name);
    write_system_message(messages, system_index, system_content, insert_empty_system);
}

fn append_positioned_rules<'a, I>(content: &mut String, rules: I)
where
    I: Iterator<Item = &'a TarvenRule>,
{
    let (prepend, append) = rules.fold((Vec::new(), Vec::new()), |mut parts, rule| {
        if rule.position.as_deref() == Some("prepend") {
            parts.0.push(render_rule_content(rule));
        } else {
            parts.1.push(render_rule_content(rule));
        }
        parts
    });
    prepend_content(content, &prepend);
    append_content(content, &append);
}

fn prepend_content(content: &mut String, parts: &[String]) {
    if parts.is_empty() {
        return;
    }
    let prefix = parts.join("\n\n");
    if content.is_empty() {
        *content = prefix;
    } else {
        *content = format!("{}\n\n{}", prefix, content);
    }
}

fn append_content(content: &mut String, parts: &[String]) {
    if parts.is_empty() {
        return;
    }
    let suffix = parts.join("\n\n");
    if content.is_empty() {
        *content = suffix;
    } else {
        content.push_str("\n\n");
        content.push_str(&suffix);
    }
}

fn write_system_message(
    messages: &mut Vec<Value>,
    system_index: Option<usize>,
    system_content: String,
    insert_empty_system: bool,
) {
    if let Some(index) = system_index {
        messages[index]["content"] = Value::String(system_content);
    } else if insert_empty_system || !system_content.is_empty() {
        messages.insert(
            0,
            serde_json::json!({"role": "system", "content": system_content}),
        );
    }
}

fn apply_user_suffix_rules(messages: &mut [Value], rules: &[TarvenRule]) {
    let user_rules = rules.iter().filter(|rule| rule.rule_type == "user_suffix");
    let (prepend, append): (Vec<_>, Vec<_>) =
        user_rules.partition(|rule| rule.position.as_deref() == Some("prepend"));
    if prepend.is_empty() && append.is_empty() {
        return;
    }
    let Some(index) = messages
        .iter()
        .rposition(|message| message["role"].as_str() == Some("user"))
    else {
        return;
    };
    let mut content = messages[index]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let prepend = prepend
        .iter()
        .map(|rule| render_rule_content(rule))
        .collect::<Vec<_>>();
    let append = append
        .iter()
        .map(|rule| render_rule_content(rule))
        .collect::<Vec<_>>();
    prepend_content(&mut content, &prepend);
    append_content(&mut content, &append);
    messages[index]["content"] = Value::String(content);
}

fn apply_context_rules(messages: &mut Vec<Value>, rules: &[TarvenRule]) {
    let mut context_rules = rules
        .iter()
        .filter(|rule| rule.rule_type == "context_inject")
        .collect::<Vec<_>>();
    if context_rules.is_empty() {
        return;
    }
    let (mut system_messages, mut non_system_messages) = split_system_messages(messages);
    context_rules.sort_by_key(|rule| std::cmp::Reverse(rule.depth.unwrap_or(0)));
    for rule in context_rules {
        let depth = rule.depth.unwrap_or(0) as usize;
        let index = non_system_messages.len().saturating_sub(depth);
        non_system_messages.insert(
            index,
            serde_json::json!({
                "role": rule.role.as_deref().unwrap_or("user"),
                "content": render_rule_content(rule),
                "__tavernInjected": true
            }),
        );
    }
    messages.append(&mut system_messages);
    messages.append(&mut non_system_messages);
}

fn split_system_messages(messages: &mut Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    let mut system_messages = Vec::new();
    let mut non_system_messages = Vec::new();
    for message in messages.drain(..) {
        if message["role"].as_str() == Some("system") {
            system_messages.push(message);
        } else {
            non_system_messages.push(message);
        }
    }
    (system_messages, non_system_messages)
}

#[tauri::command]
pub async fn preview_tarven_injection(
    rules: Vec<TarvenRule>,
    mock_messages: Option<Vec<Value>>,
) -> Result<Vec<Value>, String> {
    let mut messages = mock_messages.unwrap_or_else(default_preview_messages);
    let metadata = if rules
        .iter()
        .any(|rule| rule.id == "system_meta_injection" && rule.is_enabled)
    {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string();
        Some(EnvironmentMetadata {
            now: now.clone(),
            created_at: Some(now),
        })
    } else {
        None
    };
    let enabled_rules = rules
        .into_iter()
        .filter(|rule| rule.is_enabled)
        .collect::<Vec<_>>();
    apply_rule_pipeline(
        &mut messages,
        &enabled_rules,
        "秋水智能体",
        metadata.as_ref(),
        true,
    );
    Ok(messages)
}

fn default_preview_messages() -> Vec<Value> {
    vec![
        serde_json::json!({ "role": "system", "content": "你是一个智能助手。" }),
        serde_json::json!({ "role": "user", "content": "你好，请问你是？" }),
        serde_json::json!({
            "role": "assistant",
            "content": "我是你的 AI 助手，有什么可以帮你的吗？"
        }),
        serde_json::json!({ "role": "user", "content": "帮我写一首关于秋天的诗。" }),
    ]
}
