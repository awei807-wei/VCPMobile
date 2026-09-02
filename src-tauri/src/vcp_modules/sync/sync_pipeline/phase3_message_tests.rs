use super::{Phase3Message, Phase3StateBudget, MAX_PHASE3_MESSAGES_PER_TOPIC};

#[test]
fn phase3_state_budget_rejects_an_oversized_single_topic_before_loading_rows() {
    let mut budget = Phase3StateBudget::default();
    let error = budget
        .observe_topic("topic", MAX_PHASE3_MESSAGES_PER_TOPIC + 1, 1)
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
            topic_id TEXT PRIMARY KEY, owner_type TEXT, owner_id TEXT,
            content_hash TEXT, deleted_at INTEGER
         );
         CREATE TABLE messages (
            topic_id TEXT, msg_id TEXT, content_hash TEXT, deleted_at INTEGER,
            PRIMARY KEY(topic_id, msg_id)
         );
         INSERT INTO topics VALUES
            ('live', 'agent', 'agent-a', 'topic-hash', NULL),
            ('deleted', 'agent', 'agent-a', 'deleted-hash', 9);",
    )
    .execute(&pool)
    .await
    .expect("create fixture");

    for missing in ["missing", "deleted"] {
        let error = Phase3Message::get_topic_message_hashes(
            &pool,
            &["live".to_string(), missing.to_string()],
        )
        .await
        .expect_err("missing or tombstoned topic must fail closed");
        assert!(error.contains(missing));
    }
}
