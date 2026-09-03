use super::{
    apply_tasks, bubble_owners, bubble_topics, execute_batch_sync, open_connection,
    ConnectionHolder,
};
use crate::vcp_modules::persistence::db_write_queue::DbWriteTask;
use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::sync_hash::HashAggregator;
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
                updated_at INTEGER, config_hash TEXT NOT NULL DEFAULT '',
                content_hash TEXT NOT NULL DEFAULT '', deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE messages (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL, content_hash TEXT NOT NULL DEFAULT '',
                timestamp INTEGER, deleted_at INTEGER
             );
             INSERT INTO agents (agent_id) VALUES ('agent-a');
             INSERT INTO groups (group_id) VALUES ('group-a');",
        )
        .expect("create worker contract schema");
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
