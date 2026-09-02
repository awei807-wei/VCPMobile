use super::*;
use crate::vcp_modules::sync_dto::{AgentSyncDTO, AgentTopicSyncDTO, GroupTopicSyncDTO};

#[test]
fn test_message_fingerprint_ignores_attachment_order() {
    let a = HashAggregator::compute_message_fingerprint(
        "hello",
        &["hash-b".to_string(), "hash-a".to_string()],
    );
    let b = HashAggregator::compute_message_fingerprint(
        "hello",
        &["hash-a".to_string(), "hash-b".to_string()],
    );

    assert_eq!(a, b);
    assert_ne!(
        a,
        HashAggregator::compute_message_fingerprint("hello!", &["hash-a".to_string()])
    );
}

#[test]
fn test_agent_config_hash_rounds_temperature_to_two_decimals() {
    let base = AgentSyncDTO {
        name: "Nova".to_string(),
        system_prompt: "system".to_string(),
        model: "model-a".to_string(),
        temperature: 0.704,
        context_token_limit: 1000,
        max_output_tokens: 2000,
        stream_output: true,
    };
    let mut rounded_same = base.clone();
    rounded_same.temperature = 0.70;
    let mut rounded_diff = base.clone();
    rounded_diff.temperature = 0.706;

    assert_eq!(
        HashAggregator::compute_agent_config_hash(&base),
        HashAggregator::compute_agent_config_hash(&rounded_same)
    );
    assert_ne!(
        HashAggregator::compute_agent_config_hash(&base),
        HashAggregator::compute_agent_config_hash(&rounded_diff)
    );
}

#[test]
fn test_topic_metadata_hash_excludes_owner_id() {
    let topic_a = AgentTopicSyncDTO {
        id: "topic-1".to_string(),
        name: "Topic".to_string(),
        created_at: 123,
        locked: true,
        unread: false,
        owner_id: "agent-a".to_string(),
    };
    let mut topic_b = topic_a.clone();
    topic_b.owner_id = "agent-b".to_string();

    assert_eq!(
        HashAggregator::compute_agent_topic_metadata_hash(&topic_a),
        HashAggregator::compute_agent_topic_metadata_hash(&topic_b)
    );
}

#[test]
fn test_group_topic_metadata_hash_excludes_locked_unread_conceptually() {
    let group_topic = GroupTopicSyncDTO {
        id: "topic-1".to_string(),
        name: "Topic".to_string(),
        created_at: 123,
        owner_id: "group-a".to_string(),
    };
    let mut same_metadata_other_owner = group_topic.clone();
    same_metadata_other_owner.owner_id = "group-b".to_string();

    assert_eq!(
        HashAggregator::compute_group_topic_metadata_hash(&group_topic),
        HashAggregator::compute_group_topic_metadata_hash(&same_metadata_other_owner)
    );
}

#[tokio::test]
async fn hash_initializer_rolls_back_and_surfaces_row_errors() {
    let agent_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open agent database");
    sqlx::query(
        "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, config_hash TEXT, deleted_at INTEGER
             );
             INSERT INTO agents VALUES ('broken-agent', 'PENDING', NULL);",
    )
    .execute(&agent_pool)
    .await
    .expect("create broken agent fixture");
    let agent_error = HashInitializer::ensure_all_agent_hashes(&agent_pool)
        .await
        .expect_err("per-agent query errors must abort initialization");
    assert!(agent_error.contains("broken-agent"));

    let group_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open group database");
    sqlx::query(
        "CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, config_hash TEXT, name TEXT, mode TEXT,
                group_prompt TEXT, invite_prompt TEXT, use_unified_model INTEGER,
                unified_model TEXT, tag_match_mode TEXT, created_at INTEGER,
                deleted_at INTEGER
             );
             CREATE TABLE group_members (
                group_id TEXT, agent_id TEXT, sort_order INTEGER
             );
             INSERT INTO groups VALUES
                ('broken-group', 'PENDING', 'Group', 'fixed', NULL, NULL, 0, NULL, NULL, 1, NULL);
             INSERT INTO group_members VALUES ('broken-group', 'agent', 0);",
    )
    .execute(&group_pool)
    .await
    .expect("create broken group fixture");
    let group_error = HashInitializer::ensure_all_group_hashes(&group_pool)
        .await
        .expect_err("member-tag query errors must abort initialization");
    assert!(group_error.contains("broken-group"));
}

#[tokio::test]
async fn hash_initializer_accepts_legacy_null_hashes_without_panicking() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, name TEXT, system_prompt TEXT, model TEXT,
                temperature REAL, context_token_limit INTEGER, max_output_tokens INTEGER,
                stream_output INTEGER, config_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, name TEXT, mode TEXT, group_prompt TEXT,
                invite_prompt TEXT, use_unified_model INTEGER, unified_model TEXT,
                tag_match_mode TEXT, created_at INTEGER, config_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE group_members (
                group_id TEXT, agent_id TEXT, member_tag TEXT, sort_order INTEGER
             );
             INSERT INTO agents VALUES
                ('legacy-agent', 'Agent', '', 'model', 1, 100, 20, 1, NULL, NULL);
             INSERT INTO groups VALUES
                ('legacy-group', 'Group', 'fixed', NULL, NULL, 0, NULL, NULL, 1, NULL, NULL);",
    )
    .execute(&pool)
    .await
    .expect("create legacy fixture");

    HashInitializer::ensure_all_agent_hashes(&pool)
        .await
        .expect("initialize agent hash");
    HashInitializer::ensure_all_group_hashes(&pool)
        .await
        .expect("initialize group hash");
    let agent_hash: String =
        sqlx::query_scalar("SELECT config_hash FROM agents WHERE agent_id = 'legacy-agent'")
            .fetch_one(&pool)
            .await
            .expect("read agent hash");
    let group_hash: String =
        sqlx::query_scalar("SELECT config_hash FROM groups WHERE group_id = 'legacy-group'")
            .fetch_one(&pool)
            .await
            .expect("read group hash");
    assert!(!agent_hash.is_empty());
    assert!(!group_hash.is_empty());
}
