use super::*;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, AttachmentSyncDTO, MessageSyncDTO,
};
use crate::vcp_modules::sync_types::compute_merkle_root;
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::json;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[test]
fn canonical_json_matches_desktop_utf8_key_order_and_escaping() {
    let value = json!({
        "memberTags": {
            "😀": "astral",
            "": "private-use",
            "a\":\"x\",\"b": "y",
            "line\nkey": "control",
            "slash\\key": "slash"
        }
    });
    assert_eq!(
        canonical_json(&value),
        r#"{"memberTags":{"a\":\"x\",\"b":"y","line\nkey":"control","slash\\key":"slash","":"private-use","😀":"astral"}}"#
    );
    assert_eq!(
        compute_canonical_hash(&value),
        "ec2a40f5180600d91ea1621b75e00db55cbbdca6ad1db1c23841c426c81e3670"
    );
}

#[test]
fn message_fingerprint_matches_desktop_golden_cases() {
    let first = HashAggregator::compute_message_fingerprint_with_identity(
        "message-unicode",
        "user",
        None,
        "你好 <section data-raw=\"yes\">raw html</section>",
        1_700_000_000,
        None,
        &[HASH_C.to_string(), HASH_A.to_string(), HASH_B.to_string()],
    );
    assert_eq!(
        first,
        "4c020f54137f92cd790eeacd0f07acbb8e02baa7ccf0328fe48f70d1ce62c526"
    );

    let second = HashAggregator::compute_message_fingerprint_with_identity(
        "message-empty",
        "assistant",
        Some("Nova"),
        "",
        1_700_000_001,
        Some("agent-nova"),
        &[],
    );
    assert_eq!(
        second,
        "c7809741b18b9f3800949599b7dac8dd3c588147f6c2a3e064857f9b7484cf7d"
    );

    let rewritten_topic = HashAggregator::compute_message_fingerprint_with_identity(
        "message-owner-conflict",
        "user",
        None,
        "owner conflict",
        1_700_000_002,
        None,
        &[],
    );
    assert_eq!(
        rewritten_topic,
        "93e09bacfaebdf9bb21f776f1046e06951df5c4e06741331e42eca1b016d5851"
    );
}

#[test]
fn message_fingerprint_ignores_attachment_order_but_keeps_message_identity() {
    let left = HashAggregator::compute_message_fingerprint_with_identity(
        "message-1",
        "user",
        None,
        "hello",
        1,
        None,
        &[HASH_B.to_string(), HASH_A.to_string(), String::new()],
    );
    let right = HashAggregator::compute_message_fingerprint_with_identity(
        "message-1",
        "user",
        None,
        "hello",
        1,
        None,
        &[HASH_A.to_string(), HASH_B.to_string()],
    );
    assert_eq!(left, right);
    assert_ne!(
        left,
        HashAggregator::compute_message_fingerprint_with_identity(
            "message-2",
            "user",
            None,
            "hello",
            1,
            None,
            &[HASH_A.to_string(), HASH_B.to_string()],
        )
    );
}

#[test]
fn message_dto_fingerprint_ignores_wire_local_fields() {
    let dto = MessageSyncDTO {
        id: "message-1".to_string(),
        role: "assistant".to_string(),
        name: Some("Nova".to_string()),
        content: "hello".to_string(),
        timestamp: 9,
        updated_at: 10,
        is_thinking: Some(true),
        agent_id: Some("agent-1".to_string()),
        group_id: Some("group-a".to_string()),
        topic_id: Some("topic-a".to_string()),
        is_group_message: Some(true),
        finish_reason: Some("stop".to_string()),
        attachments: Some(vec![AttachmentSyncDTO {
            r#type: "file".to_string(),
            name: "file.txt".to_string(),
            size: 1,
            hash: HASH_A.to_string(),
            extracted_text: None,
            image_frames: None,
            created_at: None,
            status: None,
            attachment_order: Some(0),
        }]),
        content_hash: Some("local-index-value".to_string()),
    };
    let mut changed = dto.clone();
    changed.updated_at += 100;
    changed.is_thinking = Some(false);
    changed.group_id = Some("group-b".to_string());
    changed.topic_id = Some("topic-b".to_string());
    changed.is_group_message = Some(false);
    changed.content_hash = Some("different-index-value".to_string());
    assert_eq!(
        HashAggregator::compute_message_fingerprint_for_dto(&dto),
        HashAggregator::compute_message_fingerprint_for_dto(&changed)
    );
}

#[test]
fn topic_and_owner_leaf_hashes_match_desktop_payloads() {
    assert_eq!(
        HashAggregator::compute_message_leaf_hash("message-unicode", HASH_A),
        "8705013af18d91474ed98f3ec28ef3bc1fe5fe9bf03233b787050569b7f1a09a"
    );
    assert_eq!(
        HashAggregator::compute_topic_leaf_hash("topic-golden", HASH_A, HASH_B),
        "87a004ee0eb67b6ce6331a218634ce25207d931259b59c3b7325d7a5348c6ba2"
    );
}

#[test]
fn agent_config_hash_rounds_temperature_to_two_decimals() {
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
fn topic_metadata_hash_excludes_owner_id_but_owner_root_is_scoped() {
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

#[tokio::test]
async fn topic_root_hash_is_owner_aware_and_uses_message_leaves() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            title TEXT NOT NULL, created_at INTEGER NOT NULL, locked INTEGER NOT NULL,
            unread INTEGER NOT NULL, deleted_at INTEGER, config_hash TEXT NOT NULL,
            content_hash TEXT NOT NULL, PRIMARY KEY(owner_type, owner_id, topic_id)
        );
        CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, content_hash TEXT NOT NULL, timestamp INTEGER NOT NULL,
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
        );",
    )
    .execute(&pool)
    .await
    .expect("create composite fixture");
    sqlx::query(
        "INSERT INTO topics
         (owner_type, owner_id, topic_id, title, created_at, locked, unread, config_hash, content_hash)
         VALUES
         ('agent', 'agent-a', 'shared', 'A', 1, 1, 0, ?, ''),
         ('agent', 'agent-b', 'shared', 'B', 1, 1, 0, ?, '')",
    )
    .bind(HASH_A)
    .bind(HASH_B)
    .execute(&pool)
    .await
    .expect("insert topics");
    sqlx::query(
        "INSERT INTO messages
         (owner_type, owner_id, topic_id, msg_id, content_hash, timestamp)
         VALUES
         ('agent', 'agent-a', 'shared', 'message-a', ?, 1),
         ('agent', 'agent-b', 'shared', 'message-b', ?, 1)",
    )
    .bind(HASH_A)
    .bind(HASH_B)
    .execute(&pool)
    .await
    .expect("insert messages");

    let mut tx = pool.begin().await.expect("begin transaction");
    let root_a = HashAggregator::compute_topic_root_hash_for_key(
        &mut tx,
        &TopicKey::new("agent", "agent-a", "shared"),
    )
    .await
    .expect("agent a root");
    let root_b = HashAggregator::compute_topic_root_hash_for_key(
        &mut tx,
        &TopicKey::new("agent", "agent-b", "shared"),
    )
    .await
    .expect("agent b root");
    assert_eq!(
        root_a,
        compute_merkle_root(vec![HashAggregator::compute_message_leaf_hash(
            "message-a",
            HASH_A
        )])
    );
    assert_eq!(
        root_b,
        compute_merkle_root(vec![HashAggregator::compute_message_leaf_hash(
            "message-b",
            HASH_B
        )])
    );
    assert_ne!(root_a, root_b);
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
