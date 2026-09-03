use super::*;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_types::compute_merkle_root;
use serde_json::json;

const OLD_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ATTACHMENT_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

async fn fixture_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open hash initialization database");
    sqlx::raw_sql(
        "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);
         CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY, name TEXT NOT NULL, system_prompt TEXT NOT NULL,
            model TEXT NOT NULL, temperature REAL NOT NULL, context_token_limit INTEGER NOT NULL,
            max_output_tokens INTEGER NOT NULL, stream_output INTEGER NOT NULL,
            config_hash TEXT NOT NULL, content_hash TEXT NOT NULL, deleted_at INTEGER
         );
         CREATE TABLE groups (
            group_id TEXT PRIMARY KEY, name TEXT NOT NULL, mode TEXT NOT NULL,
            group_prompt TEXT, invite_prompt TEXT, use_unified_model INTEGER NOT NULL,
            unified_model TEXT, tag_match_mode TEXT, created_at INTEGER NOT NULL,
            config_hash TEXT NOT NULL, content_hash TEXT NOT NULL, deleted_at INTEGER
         );
         CREATE TABLE group_members (
            group_id TEXT NOT NULL, agent_id TEXT NOT NULL, member_tag TEXT,
            sort_order INTEGER NOT NULL
         );
         CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            title TEXT NOT NULL, created_at INTEGER NOT NULL, locked INTEGER NOT NULL,
            unread INTEGER NOT NULL, config_hash TEXT NOT NULL, content_hash TEXT NOT NULL,
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, role TEXT NOT NULL, name TEXT, agent_id TEXT,
            content TEXT NOT NULL, timestamp INTEGER NOT NULL, content_hash TEXT NOT NULL,
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE message_attachments (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, hash TEXT NOT NULL, attachment_order INTEGER NOT NULL,
            deleted_at INTEGER
         );
         CREATE TABLE render_cache (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, content_hash TEXT NOT NULL,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );",
    )
    .execute(&pool)
    .await
    .expect("create hash initialization schema");

    sqlx::query(
        "INSERT INTO agents VALUES
         ('agent-a', 'Agent', 'Prompt', 'model', 0.7, 1000, 200, 1, ?, ?, NULL)",
    )
    .bind(OLD_HASH)
    .bind(OLD_HASH)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO groups VALUES
         ('group-a', 'Group', 'fixed', NULL, NULL, 0, NULL, NULL, 3, ?, ?, NULL)",
    )
    .bind(OLD_HASH)
    .bind(OLD_HASH)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO group_members VALUES ('group-a', 'agent-a', 'lead', 0)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO topics VALUES
         ('agent', 'agent-a', 'shared', 'Agent Topic', 1, 1, 1, ?, ?, NULL),
         ('group', 'group-a', 'shared', 'Group Topic', 2, 1, 0, ?, ?, NULL),
         ('agent', 'agent-a', 'deleted', 'Deleted', 4, 1, 0, 'invalid', 'invalid', 5)",
    )
    .bind(OLD_HASH)
    .bind(OLD_HASH)
    .bind(OLD_HASH)
    .bind(OLD_HASH)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages VALUES
         ('agent', 'agent-a', 'shared', 'same-message', 'user', NULL, NULL,
          'agent-content', 10, ?, NULL),
         ('group', 'group-a', 'shared', 'same-message', 'assistant', 'Agent', 'agent-a',
          'group-content', 20, ?, NULL)",
    )
    .bind(OLD_HASH)
    .bind(OLD_HASH)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO message_attachments VALUES
         ('agent', 'agent-a', 'shared', 'same-message', ?, 0, NULL)",
    )
    .bind(ATTACHMENT_HASH.to_ascii_uppercase())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO render_cache VALUES
         ('agent', 'agent-a', 'shared', 'same-message', ?),
         ('group', 'group-a', 'shared', 'same-message', ?)",
    )
    .bind(OLD_HASH)
    .bind(OLD_HASH)
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn wire14_initializer_rehashes_legacy_rows_in_composite_scope_once() {
    let pool = fixture_pool().await;
    let stats = HashInitializer::ensure_wire14_hashes(&pool)
        .await
        .expect("rehash legacy Wire data");
    assert_eq!(
        stats,
        Wire14HashInitStats {
            full_rebuild: true,
            messages: 2,
            topics: 2,
            agents: 1,
            groups: 1,
        }
    );

    let agent_message = HashAggregator::compute_message_fingerprint_with_identity(
        "same-message",
        "user",
        None,
        "agent-content",
        10,
        None,
        &[ATTACHMENT_HASH.to_string()],
    );
    let group_message = HashAggregator::compute_message_fingerprint_with_identity(
        "same-message",
        "assistant",
        Some("Agent"),
        "group-content",
        20,
        Some("agent-a"),
        &[],
    );
    let message_hashes: Vec<String> =
        sqlx::query_scalar("SELECT content_hash FROM messages ORDER BY owner_type, owner_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        message_hashes,
        [agent_message.clone(), group_message.clone()]
    );
    let stored_attachment_hash: String = sqlx::query_scalar(
        "SELECT hash FROM message_attachments
         WHERE owner_type = 'agent' AND owner_id = 'agent-a'
           AND topic_id = 'shared' AND msg_id = 'same-message'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_attachment_hash, ATTACHMENT_HASH.to_ascii_uppercase());

    let agent_topic_config =
        HashAggregator::compute_agent_topic_metadata_hash(&AgentTopicSyncDTO {
            id: "shared".to_string(),
            name: "Agent Topic".to_string(),
            created_at: 1,
            locked: true,
            unread: true,
            owner_id: "agent-a".to_string(),
        });
    let group_topic_config =
        HashAggregator::compute_group_topic_metadata_hash(&GroupTopicSyncDTO {
            id: "shared".to_string(),
            name: "Group Topic".to_string(),
            created_at: 2,
            owner_id: "group-a".to_string(),
        });
    let agent_topic_content = compute_merkle_root(vec![HashAggregator::compute_message_leaf_hash(
        "same-message",
        &agent_message,
    )]);
    let group_topic_content = compute_merkle_root(vec![HashAggregator::compute_message_leaf_hash(
        "same-message",
        &group_message,
    )]);
    let topics: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT owner_type, config_hash, content_hash FROM topics
         WHERE deleted_at IS NULL ORDER BY owner_type",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        topics,
        [
            (
                "agent".to_string(),
                agent_topic_config.clone(),
                agent_topic_content.clone(),
            ),
            (
                "group".to_string(),
                group_topic_config.clone(),
                group_topic_content.clone(),
            ),
        ]
    );

    let agent_config = HashAggregator::compute_agent_config_hash(&AgentSyncDTO {
        name: "Agent".to_string(),
        system_prompt: "Prompt".to_string(),
        model: "model".to_string(),
        temperature: 0.7,
        context_token_limit: 1000,
        max_output_tokens: 200,
        stream_output: true,
    });
    let group_config = HashAggregator::compute_group_config_hash(&GroupSyncDTO {
        name: "Group".to_string(),
        members: vec!["agent-a".to_string()],
        mode: "fixed".to_string(),
        member_tags: Some(json!({"agent-a": "lead"})),
        group_prompt: None,
        invite_prompt: None,
        use_unified_model: false,
        unified_model: None,
        tag_match_mode: None,
        created_at: 3,
    });
    let owners: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT 'agent', config_hash, content_hash FROM agents
         UNION ALL SELECT 'group', config_hash, content_hash FROM groups
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        owners,
        [
            (
                "agent".to_string(),
                agent_config,
                compute_merkle_root(vec![HashAggregator::compute_topic_leaf_hash(
                    "shared",
                    &agent_topic_config,
                    &agent_topic_content,
                )]),
            ),
            (
                "group".to_string(),
                group_config,
                compute_merkle_root(vec![HashAggregator::compute_topic_leaf_hash(
                    "shared",
                    &group_topic_config,
                    &group_topic_content,
                )]),
            ),
        ]
    );

    let deleted_hashes: (String, String) =
        sqlx::query_as("SELECT config_hash, content_hash FROM topics WHERE topic_id = 'deleted'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        deleted_hashes,
        ("invalid".to_string(), "invalid".to_string())
    );
    let second = HashInitializer::ensure_wire14_hashes(&pool)
        .await
        .expect("repeat hash initialization");
    assert_eq!(second, Wire14HashInitStats::default());
}

#[tokio::test]
async fn wire14_initializer_keeps_duplicate_case_variant_attachment_relations() {
    let pool = fixture_pool().await;
    sqlx::query(
        "INSERT INTO message_attachments VALUES
         ('agent', 'agent-a', 'shared', 'same-message', ?, 1, NULL)",
    )
    .bind(ATTACHMENT_HASH)
    .execute(&pool)
    .await
    .expect("insert duplicate CAS relation");

    HashInitializer::ensure_wire14_hashes(&pool)
        .await
        .expect("rehash duplicate case variants");

    let expected = HashAggregator::compute_message_fingerprint_with_identity(
        "same-message",
        "user",
        None,
        "agent-content",
        10,
        None,
        &[ATTACHMENT_HASH.to_string(), ATTACHMENT_HASH.to_string()],
    );
    let actual: String = sqlx::query_scalar(
        "SELECT content_hash FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'agent-a'
           AND topic_id = 'shared' AND msg_id = 'same-message'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actual, expected);

    let relation_hashes: Vec<String> = sqlx::query_scalar(
        "SELECT hash FROM message_attachments
         WHERE owner_type = 'agent' AND owner_id = 'agent-a'
           AND topic_id = 'shared' AND msg_id = 'same-message'
         ORDER BY attachment_order",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        relation_hashes,
        vec![
            ATTACHMENT_HASH.to_ascii_uppercase(),
            ATTACHMENT_HASH.to_string()
        ]
    );
}
