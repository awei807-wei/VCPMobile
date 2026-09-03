// distributed/tools/topic_memo.rs
// [OneShot] TopicMemo — exposes local agent topics to the distributed server.

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::{InvocationCommand, ToolManifest};
use crate::vcp_modules::db_manager::DbState;

#[path = "topic_memo_support.rs"]
mod topic_memo_support;
use topic_memo_support::*;

pub struct TopicMemoTool;

#[async_trait]
impl OneShotTool for TopicMemoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "TopicMemo".to_string(),
            display_name: "话题回忆插件".to_string(),
            description:
                "获取 VCPMobile 本机智能体的话题列表和完整聊天记录，实现 AI 的话题级回忆功能。"
                    .to_string(),
            placeholder: None,
            invocation_commands: vec![InvocationCommand {
                command_identifier: "TopicMemo".to_string(),
                description: "列出指定智能体的话题，或根据话题 ID 读取完整聊天记录。\n\
参数:\n\
- command (字符串, 可选): ListTopics 或 GetTopicContent，默认 ListTopics\n\
- maid (字符串, 必需): 智能体名称或名称片段\n\
- topic_id/topicId/TopicId (字符串, GetTopicContent 必需): 话题 ID\n\
调用格式:\n\
<<<[TOOL_REQUEST]>>>\n\
tool_name:「始」TopicMemo「末」\n\
command:「始」ListTopics「末」\n\
maid:「始」小克「末」\n\
<<<[END_TOOL_REQUEST]>>>"
                    .to_string(),
                example: "<<<[TOOL_REQUEST]>>>\ntool_name:「始」TopicMemo「末」\ncommand:「始」GetTopicContent「末」\nmaid:「始」小克「末」\ntopic_id:「始」topic_1766100505346「末」\n<<<[END_TOOL_REQUEST]>>>".to_string(),
            }],
            web_socket_push: None,
        }
    }

    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String> {
        let command = get_string_arg(&args, "command").unwrap_or_else(|| "ListTopics".to_string());
        let maid_name =
            get_string_arg(&args, "maid").ok_or_else(|| "请求中缺少 'maid' 参数。".to_string())?;

        let db_state = app
            .try_state::<DbState>()
            .ok_or_else(|| "数据库尚未初始化，TopicMemo 暂不可用。".to_string())?;
        let pool = &db_state.pool;

        let agent = find_agent_info(pool, &maid_name).await?;
        let result = match command.as_str() {
            "ListTopics" => list_topics(pool, &agent).await?,
            "GetTopicContent" => {
                let topic_id = get_topic_id_arg(&args)
                    .ok_or_else(|| "请求中缺少 'topic_id' 参数。".to_string())?;
                let user_name = find_user_name(app).await;
                get_topic_content(pool, &agent, &topic_id, &user_name).await?
            }
            other => {
                return Err(format!(
                    "未知的指令: {}，支持的指令: ListTopics, GetTopicContent",
                    other
                ));
            }
        };

        Ok(json!({
            "status": "success",
            "result": result
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_topic_id_aliases() {
        assert_eq!(
            get_topic_id_arg(&json!({ "topicId": "topic_1" })),
            Some("topic_1".to_string())
        );
        assert_eq!(
            get_topic_id_arg(&json!({ "TopicId": "topic_2" })),
            Some("topic_2".to_string())
        );
        assert_eq!(
            get_topic_id_arg(&json!({ "topic_id": "topic_3" })),
            Some("topic_3".to_string())
        );
    }

    #[test]
    fn escapes_sql_like_wildcards() {
        assert_eq!(escape_like_pattern(r"50%_done\ok"), r"50\%\_done\\ok");
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
             VALUES ('agent_3', '100% real', 3, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let exact = find_agent_info(&pool, "100% real").await.unwrap();
        assert_eq!(exact.id, "agent_3");
    }

    #[tokio::test]
    async fn topic_content_reports_decompression_errors() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE topics (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                title TEXT NOT NULL,
                created_at BIGINT NOT NULL,
                locked INTEGER NOT NULL DEFAULT 0,
                msg_count INTEGER NOT NULL DEFAULT 0,
                deleted_at BIGINT,
                PRIMARY KEY (owner_type, owner_id, topic_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
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
                deleted_at BIGINT,
                PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, locked, msg_count, deleted_at)
             VALUES ('topic_1', 'agent', 'agent_1', 'Broken', 1, 0, 1, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, role, name, content, timestamp, deleted_at)
             VALUES ('agent', 'agent_1', 'topic_1', 'msg_1', 'assistant', 'Alpha', ?, 1, NULL)",
        )
        .bind(vec![1_u8, 2, 3])
        .execute(&pool)
        .await
        .unwrap();

        let agent = AgentInfo {
            id: "agent_1".to_string(),
            name: "Alpha".to_string(),
        };
        let error = get_topic_content(&pool, &agent, "topic_1", "用户")
            .await
            .unwrap_err();

        assert!(error.contains("解压失败"));
    }

    #[test]
    fn cleans_html_message_content() {
        let cleaned =
            clean_message_content("<p>Hello <strong>Topic</strong></p><script>x</script>");
        assert!(cleaned.contains("Hello **Topic**"));
        assert!(!cleaned.contains("<p>"));
        assert!(!cleaned.contains('x'));

        let upper = clean_message_content("<STYLE>.a{}</STYLE><p>Ok</p>");
        assert_eq!(upper, "Ok");

        let mixed_script = clean_message_content("<SCRIPT>alert(1)</SCRIPT><p>After</p>");
        assert_eq!(mixed_script, "After");
    }
}
