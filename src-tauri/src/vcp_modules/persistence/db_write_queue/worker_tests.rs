use super::{
    apply_tasks, bubble_owners, bubble_topics, execute_batch_sync, open_connection,
    ConnectionHolder,
};
use crate::vcp_modules::persistence::db_write_queue::DbWriteTask;
use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use rusqlite::{Connection, TransactionBehavior};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn agent_topic(id: &str) -> AgentTopicSyncDTO {
    AgentTopicSyncDTO {
        id: id.to_string(),
        name: format!("Agent {id}"),
        created_at: 10,
        locked: true,
        unread: false,
        owner_id: "agent-a".to_string(),
    }
}

fn group_topic(id: &str) -> GroupTopicSyncDTO {
    GroupTopicSyncDTO {
        id: id.to_string(),
        name: format!("Group {id}"),
        created_at: 20,
        owner_id: "group-a".to_string(),
    }
}

fn setup_connection() -> Connection {
    let connection = Connection::open_in_memory().expect("open in-memory database");
    setup_schema(&connection);
    connection
}

fn setup_schema(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL DEFAULT '',
                deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL DEFAULT '',
                deleted_at INTEGER
             );
             CREATE TABLE topics (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                title TEXT, created_at INTEGER, locked INTEGER, unread INTEGER,
                unread_count INTEGER NOT NULL DEFAULT 0,
                msg_count INTEGER NOT NULL DEFAULT 0,
                last_message_updated_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER, config_hash TEXT NOT NULL DEFAULT '',
                content_hash TEXT NOT NULL DEFAULT '', deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE messages (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL, content_hash TEXT NOT NULL DEFAULT '',
                timestamp INTEGER, deleted_at INTEGER
             );
             CREATE TABLE messages_fts (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT, content TEXT
             );
             CREATE TABLE render_cache (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
             );
             CREATE TABLE message_attachments (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
             );
             CREATE TABLE active_generations (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
             );
             CREATE TABLE message_unread_receipts (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL, created_at INTEGER NOT NULL,
                counted_unread INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             INSERT INTO agents (agent_id) VALUES ('agent-a');
             INSERT INTO groups (group_id) VALUES ('group-a');",
        )
        .expect("create worker contract schema");
}

#[tokio::test]
async fn real_worker_topic_read_sync_clears_stale_receipt_before_delete() {
    let path = std::env::temp_dir().join(format!(
        "vcp-mobile-write-queue-unread-{}-{}.sqlite",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let connection = Connection::open(&path).expect("open worker unread database");
    setup_schema(&connection);
    connection
        .execute(
            "INSERT INTO topics
                (owner_type, owner_id, topic_id, title, created_at, locked,
                 unread, unread_count, updated_at)
             VALUES ('agent', 'agent-a', 'topic-read', 'stale unread', 1, 1, 1, 7, 1)",
            [],
        )
        .expect("seed stale unread topic");
    connection
        .execute_batch(
            "INSERT INTO messages
                (owner_type, owner_id, topic_id, msg_id, content_hash, timestamp, deleted_at)
             VALUES
                ('agent', 'agent-a', 'topic-read', 'message-a', 'hash-a', 10, NULL),
                ('agent', 'agent-a', 'topic-read', 'message-b', 'hash-b', 20, NULL);
             INSERT INTO message_unread_receipts
                (owner_type, owner_id, topic_id, msg_id, created_at, counted_unread)
             VALUES ('agent', 'agent-a', 'topic-read', 'message-a', 1, 1);",
        )
        .expect("seed old unread message and receipt");
    drop(connection);

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open queue coordination pool");
    let mut queue = super::DbWriteQueue::new(pool, path.clone());
    queue
        .submit(DbWriteTask::AgentTopic {
            topic_id: "topic-read".to_string(),
            dto: AgentTopicSyncDTO {
                id: "topic-read".to_string(),
                name: "read from sync".to_string(),
                created_at: 1,
                locked: true,
                unread: false,
                owner_id: "agent-a".to_string(),
            },
        })
        .await
        .expect("submit topic read sync");
    queue
        .flush()
        .await
        .expect("worker should commit topic read");

    let verification = Connection::open(&path).expect("open verification connection");
    let state: (i64, i64) = verification
        .query_row(
            "SELECT unread, unread_count FROM topics
             WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'topic-read'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read synced topic");
    assert_eq!(state, (0, 0));
    let old_receipt: i64 = verification
        .query_row(
            "SELECT counted_unread FROM message_unread_receipts
             WHERE owner_type = 'agent' AND owner_id = 'agent-a'
               AND topic_id = 'topic-read' AND msg_id = 'message-a'",
            [],
            |row| row.get(0),
        )
        .expect("read cleared old unread receipt");
    assert_eq!(old_receipt, 0);

    verification
        .execute(
            "UPDATE topics SET unread = 1, unread_count = 1
             WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'topic-read'",
            [],
        )
        .expect("seed new unread message count");
    verification
        .execute(
            "INSERT INTO message_unread_receipts
                (owner_type, owner_id, topic_id, msg_id, created_at, counted_unread)
             VALUES ('agent', 'agent-a', 'topic-read', 'message-b', 2, 1)",
            [],
        )
        .expect("seed new unread receipt");
    drop(verification);

    queue
        .submit(DbWriteTask::DeleteMessage {
            message: MessageKey::new(TopicKey::new("agent", "agent-a", "topic-read"), "message-a"),
            deleted_at: 3,
        })
        .await
        .expect("submit old message delete");
    queue
        .flush()
        .await
        .expect("worker should delete old message");

    let verification = Connection::open(&path).expect("reopen verification connection");
    let final_state: (i64, i64) = verification
        .query_row(
            "SELECT unread, unread_count FROM topics
             WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'topic-read'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read final topic unread state");
    assert_eq!(final_state, (1, 1));
    let deleted_at: Option<i64> = verification
        .query_row(
            "SELECT deleted_at FROM messages
             WHERE owner_type = 'agent' AND owner_id = 'agent-a'
               AND topic_id = 'topic-read' AND msg_id = 'message-a'",
            [],
            |row| row.get(0),
        )
        .expect("read deleted old message");
    assert_eq!(deleted_at, Some(3));
    let deleted_receipt_count: i64 = verification
        .query_row(
            "SELECT COUNT(*) FROM message_unread_receipts
             WHERE owner_type = 'agent' AND owner_id = 'agent-a'
               AND topic_id = 'topic-read' AND msg_id = 'message-a'",
            [],
            |row| row.get(0),
        )
        .expect("count deleted old unread receipt");
    assert_eq!(deleted_receipt_count, 0);
    let remaining_receipt: i64 = verification
        .query_row(
            "SELECT counted_unread FROM message_unread_receipts
             WHERE owner_type = 'agent' AND owner_id = 'agent-a'
               AND topic_id = 'topic-read' AND msg_id = 'message-b'",
            [],
            |row| row.get(0),
        )
        .expect("read new unread receipt");
    assert_eq!(remaining_receipt, 1);

    if let Some(worker) = queue._worker.take() {
        drop(queue);
        worker.await.expect("worker should stop cleanly");
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
fn topic_upserts_are_queued_for_hash_bubbling() {
    let mut connection = setup_connection();
    let agent_single = agent_topic("agent-single");
    let agent_batch = agent_topic("agent-batch");
    let group_single = group_topic("group-single");
    let group_batch = group_topic("group-batch");
    let tx = connection.transaction().expect("begin worker transaction");
    let (owners, topics) = apply_tasks(
        &tx,
        vec![
            DbWriteTask::AgentTopic {
                topic_id: agent_single.id.clone(),
                dto: agent_single.clone(),
            },
            DbWriteTask::AgentTopicBatch {
                topics: vec![(agent_batch.id.clone(), agent_batch.clone())],
            },
            DbWriteTask::GroupTopic {
                topic_id: group_single.id.clone(),
                dto: group_single.clone(),
            },
            DbWriteTask::GroupTopicBatch {
                topics: vec![(group_batch.id.clone(), group_batch.clone())],
            },
        ],
        None,
    )
    .expect("apply topic tasks");

    assert_eq!(topics.len(), 4);
    bubble_topics(&tx, topics).expect("bubble every changed topic");
    bubble_owners(&tx, owners).expect("bubble both changed owners");

    let hashes = [
        (
            "agent-single",
            HashAggregator::compute_agent_topic_metadata_hash(&agent_single),
        ),
        (
            "agent-batch",
            HashAggregator::compute_agent_topic_metadata_hash(&agent_batch),
        ),
        (
            "group-single",
            HashAggregator::compute_group_topic_metadata_hash(&group_single),
        ),
        (
            "group-batch",
            HashAggregator::compute_group_topic_metadata_hash(&group_batch),
        ),
    ];
    for (topic_id, expected) in hashes {
        let actual: String = tx
            .query_row(
                "SELECT config_hash FROM topics WHERE topic_id = ?",
                [topic_id],
                |row| row.get(0),
            )
            .expect("read topic config hash");
        assert_eq!(actual, expected);
    }
}

#[test]
fn write_batch_waits_for_concurrent_writer_before_validation_reads() {
    let path = std::env::temp_dir().join(format!(
        "vcp-mobile-write-queue-lock-{}-{}.sqlite",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let seed = Connection::open(&path).expect("open file database");
    seed.busy_timeout(Duration::from_secs(2))
        .expect("configure seed busy timeout");
    seed.pragma_update(None, "journal_mode", "WAL")
        .expect("enable WAL");
    setup_schema(&seed);
    drop(seed);

    let worker_connection = open_connection(&path).expect("open worker connection");
    let holder: ConnectionHolder = Arc::new(Mutex::new(Some(worker_connection)));
    let mut blocker = Connection::open(&path).expect("open competing writer");
    blocker
        .busy_timeout(Duration::from_secs(2))
        .expect("configure competing writer timeout");
    let blocker_tx = blocker
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("reserve competing writer");
    blocker_tx
        .execute(
            "UPDATE agents SET content_hash = 'concurrent-write' WHERE agent_id = 'agent-a'",
            [],
        )
        .expect("hold a real WAL write transaction");

    let task = DbWriteTask::AgentTopic {
        topic_id: "topic-after-lock".to_string(),
        dto: agent_topic("topic-after-lock"),
    };
    let worker_path = path.clone();
    let worker_holder = holder.clone();
    let worker =
        std::thread::spawn(move || execute_batch_sync(vec![task], worker_path, worker_holder));
    std::thread::sleep(Duration::from_millis(100));
    blocker_tx.commit().expect("release competing writer");

    worker
        .join()
        .expect("write worker thread should not panic")
        .expect("write batch should wait and commit after lock release");
    let topic_hash: String = blocker
        .query_row(
            "SELECT config_hash FROM topics
             WHERE owner_type = 'agent' AND owner_id = 'agent-a'
               AND topic_id = 'topic-after-lock'",
            [],
            |row| row.get(0),
        )
        .expect("read persisted topic hash");
    assert!(!topic_hash.is_empty());

    drop(blocker);
    drop(holder);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
