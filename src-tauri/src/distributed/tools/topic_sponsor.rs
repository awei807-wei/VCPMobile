// distributed/tools/topic_sponsor.rs
// [OneShot] MobileTopicSponsor — lets distributed agents create, inspect, and reply to local topics.

use async_trait::async_trait;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::{InvocationCommand, ToolManifest};
use crate::vcp_modules::db_manager::DbState;

#[path = "topic_sponsor_handlers.rs"]
mod topic_sponsor_handlers;
#[path = "topic_sponsor_read_handlers.rs"]
mod topic_sponsor_read_handlers;
#[path = "topic_sponsor_support.rs"]
mod topic_sponsor_support;
use topic_sponsor_handlers::*;
use topic_sponsor_read_handlers::*;
use topic_sponsor_support::*;

pub struct TopicSponsorTool;

const TOOL_NAME: &str = "MobileTopicSponsor";
const MAX_CHECK_NEW_TOPICS_DAYS: i64 = 3650;
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

#[async_trait]
impl OneShotTool for TopicSponsorTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: TOOL_NAME.to_string(),
            display_name: "移动端 AI 主动创建话题".to_string(),
            description: "允许 Agent 明确在 VCPMobile 本机创建、查询和回复话题。".to_string(),
            placeholder: None,
            invocation_commands: vec![InvocationCommand {
                command_identifier: TOOL_NAME.to_string(),
                description: "在 VCPMobile 本机话题库中执行话题操作。\n\
参数:\n\
- command (字符串, 必需): CreateTopic, ReadUnlockedTopics, CheckNewTopics, CheckUnreadMessages, ReplyToTopic, CheckTopicOwnership, ListUnlockedTopics, ReadTopicContent\n\
- maid (字符串, 必需): 目标或发起请求的智能体名称\n\
- topic_name (字符串, CreateTopic 必需): 新话题名称\n\
- initial_message (字符串, CreateTopic 必需): 第一条 assistant 消息\n\
- topic_id/topicId/TopicId (字符串): 指定话题 ID\n\
- message (字符串, ReplyToTopic 必需): 回复内容\n\
- sender_name (字符串, ReplyToTopic 必需): 回复者名称\n\
- caller_name (字符串, CheckTopicOwnership 必需): 调用者名称\n\
- include_read (布尔, ReadUnlockedTopics 可选): 是否包含已读话题\n\
- days (整数, CheckNewTopics 可选): 检查最近几天\n\
调用格式:\n\
<<<[TOOL_REQUEST]>>>\n\
tool_name:「始」MobileTopicSponsor「末」\n\
command:「始」CreateTopic「末」\n\
maid:「始」HANNA「末」\n\
topic_name:「始」一个新想法「末」\n\
initial_message:「始」主人，我突然想到，我们可以一起写一个故事！「末」\n\
<<<[END_TOOL_REQUEST]>>>"
                    .to_string(),
                example: "<<<[TOOL_REQUEST]>>>\ntool_name:「始」MobileTopicSponsor「末」\ncommand:「始」ReplyToTopic「末」\nmaid:「始」HANNA「末」\ntopic_id:「始」topic_1234567890「末」\nmessage:「始」这是一条回复消息「末」\nsender_name:「始」HANNA「末」\n<<<[END_TOOL_REQUEST]>>>".to_string(),
            }],
            web_socket_push: None,
        }
    }

    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String> {
        let command = get_string_arg(&args, "command")
            .ok_or_else(|| "请求中缺少 'command' 参数。".to_string())?;
        let maid_name =
            get_string_arg(&args, "maid").ok_or_else(|| "请求中缺少 'maid' 参数。".to_string())?;

        let db_state = app
            .try_state::<DbState>()
            .ok_or_else(|| "数据库尚未初始化，MobileTopicSponsor 暂不可用。".to_string())?;
        let pool = &db_state.pool;

        let agent = find_agent_info(pool, &maid_name).await?;
        match command.as_str() {
            "CreateTopic" => handle_create_topic(app, pool, &agent, &args).await,
            "ReadUnlockedTopics" => handle_read_unlocked_topics(pool, &agent, &args).await,
            "CheckNewTopics" => handle_check_new_topics(pool, &agent, &args).await,
            "CheckUnreadMessages" => handle_check_unread_messages(pool, &agent).await,
            "ReplyToTopic" => handle_reply_to_topic(app, pool, &agent, &args).await,
            "CheckTopicOwnership" => handle_check_topic_ownership(pool, &agent, &args).await,
            "ListUnlockedTopics" => handle_list_unlocked_topics(pool, &agent).await,
            "ReadTopicContent" => handle_read_topic_content(pool, &agent, &args).await,
            other => Err(format!("未知的命令: {}", other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::message_repository::ContentCompressor;
    use serde_json::json;

    #[test]
    fn topic_sponsor_manifest_exposes_create_topic() {
        let manifest = TopicSponsorTool.manifest();
        assert_eq!(manifest.name, "MobileTopicSponsor");
        assert_eq!(
            manifest.invocation_commands[0].command_identifier,
            "MobileTopicSponsor"
        );
        assert!(manifest
            .invocation_commands
            .iter()
            .any(|command| command.description.contains("CreateTopic")));
    }

    #[test]
    fn parses_legacy_topic_id_aliases() {
        assert_eq!(
            get_topic_id_arg(&json!({ "TopicId": "topic_1" })).as_deref(),
            Some("topic_1")
        );
    }

    #[test]
    fn escapes_sql_like_wildcards() {
        assert_eq!(escape_like_pattern(r"agent_%\name"), r"agent\_\%\\name");
    }

    #[tokio::test]
    async fn agent_lookup_treats_like_wildcards_literally() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                updated_at BIGINT NOT NULL,
                deleted_at BIGINT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agents (agent_id, name, updated_at, deleted_at)
             VALUES
                ('agent_1', 'Alpha', 1, NULL),
                ('agent_2', 'Beta', 2, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let wildcard = find_agent_info(&pool, "%").await;
        assert!(wildcard.is_err());

        sqlx::query(
            "INSERT INTO agents (agent_id, name, updated_at, deleted_at)
             VALUES ('agent_3', 'agent_%', 3, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let exact = find_agent_info(&pool, "agent_%").await.unwrap();
        assert_eq!(exact.id, "agent_3");
    }

    #[tokio::test]
    async fn load_messages_keeps_valid_messages_when_one_is_corrupt() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE messages (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                role TEXT NOT NULL,
                name TEXT,
                content BLOB NOT NULL,
                timestamp BIGINT NOT NULL,
                agent_id TEXT,
                finish_reason TEXT,
                deleted_at BIGINT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, role, name, content, timestamp, agent_id, finish_reason, deleted_at)
             VALUES ('agent', 'agent_1', 'topic_1', 'msg_1', 'assistant', 'Alpha', ?, 1, 'agent_1', 'completed', NULL)",
        )
        .bind(ContentCompressor::compress("valid content").unwrap())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, role, name, content, timestamp, agent_id, finish_reason, deleted_at)
             VALUES ('agent', 'agent_1', 'topic_1', 'msg_2', 'assistant', 'Alpha', ?, 2, 'agent_1', 'completed', NULL)",
        )
        .bind(vec![1_u8, 2, 3])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, role, name, content, timestamp, agent_id, finish_reason, deleted_at)
             VALUES ('group', 'group_1', 'topic_1', 'msg_group', 'assistant', 'Group', ?, 3, NULL, 'completed', NULL)",
        )
        .bind(ContentCompressor::compress("must stay isolated").unwrap())
        .execute(&pool)
        .await
        .unwrap();

        let messages = load_messages(&pool, "agent_1", "topic_1").await.unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["content"], "valid content");
        assert_eq!(messages[0]["contentCorrupted"], false);
        assert_eq!(messages[1]["contentCorrupted"], true);
        assert!(messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("解压失败"));
    }

    #[tokio::test]
    async fn first_message_name_returns_none_for_null_name() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE messages (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                name TEXT,
                timestamp BIGINT NOT NULL,
                deleted_at BIGINT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, name, timestamp, deleted_at)
             VALUES ('agent', 'agent_1', 'topic_1', 'msg_1', NULL, 1, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let name = first_message_name(&pool, "agent_1", "topic_1")
            .await
            .unwrap();

        assert_eq!(name, None);
    }

    #[tokio::test]
    async fn first_message_name_returns_first_non_null_name() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE messages (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                name TEXT,
                timestamp BIGINT NOT NULL,
                deleted_at BIGINT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, name, timestamp, deleted_at)
             VALUES
                ('agent', 'agent_1', 'topic_1', 'msg_1', 'creator', 1, NULL),
                ('agent', 'agent_1', 'topic_1', 'msg_2', NULL, 2, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let name = first_message_name(&pool, "agent_1", "topic_1")
            .await
            .unwrap();

        assert_eq!(name.as_deref(), Some("creator"));
    }
}
