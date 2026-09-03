use super::{
    MessageDeletedState, MessageLiveState, MessageVersionState, Phase3Message, Phase3StateBudget,
    MAX_PHASE3_MESSAGES_PER_TOPIC,
};
use crate::vcp_modules::topic_types::TopicKey;

fn topic(topic_id: &str) -> TopicKey {
    TopicKey::new("agent", "agent-a", topic_id)
}

#[test]
fn phase3_state_budget_rejects_an_oversized_single_topic() {
    let mut budget = Phase3StateBudget::default();
    let error = budget
        .observe_message("topic", MAX_PHASE3_MESSAGES_PER_TOPIC + 1, 1)
        .expect_err("oversized topic must fail before hash materialization");
    assert!(error.contains("topic"));
    assert!(error.contains("message limit"));
}

#[tokio::test]
async fn requested_topic_hashes_require_exact_live_coverage() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT, owner_id TEXT, topic_id TEXT,
            content_hash TEXT, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
            content_hash TEXT, updated_at INTEGER, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         INSERT INTO topics VALUES
            ('agent', 'agent-a', 'live', 'topic-hash', NULL),
            ('agent', 'agent-a', 'deleted', 'deleted-hash', 9);",
    )
    .execute(&pool)
    .await
    .expect("create fixture");

    for missing in ["missing", "deleted"] {
        let error =
            Phase3Message::get_topic_message_hashes(&pool, &[topic("live"), topic(missing)])
                .await
                .expect_err("missing or tombstoned topic must fail closed");
        assert!(error.contains(missing));
    }
}

#[tokio::test]
async fn message_states_use_live_update_time_and_tombstone_time() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT, owner_id TEXT, topic_id TEXT,
            content_hash TEXT, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
            content_hash TEXT, updated_at INTEGER, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         INSERT INTO topics VALUES ('agent', 'agent-a', 'topic', 'topic-hash', NULL);
         INSERT INTO messages VALUES
            ('agent', 'agent-a', 'topic', 'live', 'live-hash', 123, NULL),
            ('agent', 'agent-a', 'topic', 'deleted', 'old-hash', 50, 456);",
    )
    .execute(&pool)
    .await
    .expect("create fixture");

    let key = topic("topic");
    let states = Phase3Message::get_topic_message_hashes(&pool, std::slice::from_ref(&key))
        .await
        .expect("load message states");
    assert_eq!(
        states[&key].messages["live"],
        MessageVersionState::Live(MessageLiveState {
            message_hash: "live-hash".to_string(),
            updated_at: 123,
        })
    );
    assert_eq!(
        states[&key].messages["deleted"],
        MessageVersionState::Deleted(MessageDeletedState { deleted_at: 456 })
    );
}

#[tokio::test]
async fn message_state_snapshot_keeps_same_ids_isolated_by_owner() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT, owner_id TEXT, topic_id TEXT,
            content_hash TEXT, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
            content_hash TEXT, updated_at INTEGER, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         INSERT INTO topics VALUES
            ('agent', 'shared-owner', 'shared-topic', 'agent-topic-hash', NULL),
            ('group', 'shared-owner', 'shared-topic', 'group-topic-hash', NULL);
         INSERT INTO messages VALUES
            ('agent', 'shared-owner', 'shared-topic', 'same-message', 'agent-message-hash', 11, NULL),
            ('group', 'shared-owner', 'shared-topic', 'same-message', 'group-message-hash', 22, NULL);",
    )
    .execute(&pool)
    .await
    .expect("create fixture");

    let agent = TopicKey::new("agent", "shared-owner", "shared-topic");
    let group = TopicKey::new("group", "shared-owner", "shared-topic");
    let states = Phase3Message::get_topic_message_hashes(&pool, &[agent.clone(), group.clone()])
        .await
        .expect("load owner-isolated message states");
    assert_eq!(states.len(), 2);
    assert_eq!(states[&agent].content_hash, "agent-topic-hash");
    assert_eq!(states[&group].content_hash, "group-topic-hash");
    assert_eq!(
        states[&agent].messages["same-message"],
        MessageVersionState::Live(MessageLiveState {
            message_hash: "agent-message-hash".to_string(),
            updated_at: 11,
        })
    );
    assert_eq!(
        states[&group].messages["same-message"],
        MessageVersionState::Live(MessageLiveState {
            message_hash: "group-message-hash".to_string(),
            updated_at: 22,
        })
    );
}
