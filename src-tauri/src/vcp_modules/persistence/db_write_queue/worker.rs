use super::{DbWriteQueue, DbWriteTask};

use rusqlite::Connection;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

type ConnectionHolder = Arc<Mutex<Option<Connection>>>;
type OwnerKey = (String, String);

pub(super) fn spawn(db_path: PathBuf, rx: mpsc::Receiver<DbWriteTask>) -> JoinHandle<()> {
    let holder: ConnectionHolder = Arc::new(Mutex::new(None));
    tokio::spawn(run(rx, db_path, holder))
}

async fn run(mut rx: mpsc::Receiver<DbWriteTask>, db_path: PathBuf, holder: ConnectionHolder) {
    log::info!("[DbWriteQueue] Worker started (Turbo rusqlite Mode)");
    let mut success_count = 0u32;
    let mut error_count = 0u32;
    let mut pending_errors = Vec::new();

    while let Some(first_task) = rx.recv().await {
        if let DbWriteTask::Flush { tx } = first_task {
            let _ = tx.send(DbWriteQueue::take_pending_errors(&mut pending_errors));
            continue;
        }
        let (tasks, flush_tx) = collect_batch(first_task, &mut rx).await;
        let result = execute_batch(tasks, db_path.clone(), holder.clone()).await;
        match result {
            Ok(()) => success_count += 1,
            Err(error) => {
                error_count += 1;
                log::error!("[DbWriteQueue] {error}");
                pending_errors.push(error);
            }
        }
        if let Some(tx) = flush_tx {
            let _ = tx.send(DbWriteQueue::take_pending_errors(&mut pending_errors));
        }
    }

    log::info!(
        "[DbWriteQueue] Worker stopped. Total: success={}, errors={}",
        success_count,
        error_count
    );
}

async fn collect_batch(
    first_task: DbWriteTask,
    rx: &mut mpsc::Receiver<DbWriteTask>,
) -> (
    Vec<DbWriteTask>,
    Option<oneshot::Sender<Result<(), String>>>,
) {
    let mut tasks = vec![first_task];
    let mut total_msg_count = topic_message_count(&tasks[0]);
    let mut flush_tx = None;

    while tasks.len() < 200 && total_msg_count < 5000 {
        let next = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        match next {
            Ok(Some(DbWriteTask::Flush { tx })) => {
                flush_tx = Some(tx);
                break;
            }
            Ok(Some(task)) => {
                total_msg_count += topic_message_count(&task);
                tasks.push(task);
            }
            _ => break,
        }
    }
    (tasks, flush_tx)
}

fn topic_message_count(task: &DbWriteTask) -> u32 {
    match task {
        DbWriteTask::TopicMessages { messages, .. } => messages.len() as u32,
        _ => 0,
    }
}

async fn execute_batch(
    tasks: Vec<DbWriteTask>,
    db_path: PathBuf,
    holder: ConnectionHolder,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || execute_batch_sync(tasks, db_path, holder))
        .await
        .map_err(|error| format!("write worker join error: {error}"))?
        .map_err(|error| format!("rusqlite execution error: {error}"))
}

fn execute_batch_sync(
    tasks: Vec<DbWriteTask>,
    db_path: PathBuf,
    holder: ConnectionHolder,
) -> rusqlite::Result<()> {
    let mut guard = holder.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
    if guard.is_none() {
        let conn = open_connection(&db_path)?;
        *guard = Some(conn);
    }
    let conn = guard.as_mut().ok_or(rusqlite::Error::InvalidQuery)?;
    let tx = conn.transaction()?;
    let (owners, topics) = apply_tasks(&tx, tasks)?;
    bubble_topics(&tx, topics)?;
    bubble_owners(&tx, owners)?;
    tx.commit()
}

fn open_connection(db_path: &PathBuf) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_millis(30000))?;
    Ok(conn)
}

fn apply_tasks(
    tx: &rusqlite::Transaction<'_>,
    tasks: Vec<DbWriteTask>,
) -> rusqlite::Result<(HashSet<OwnerKey>, HashSet<String>)> {
    let mut owners = HashSet::new();
    let mut topics = HashSet::new();
    for task in tasks {
        apply_task(tx, task, &mut owners, &mut topics)?;
    }
    Ok((owners, topics))
}

fn apply_task(
    tx: &rusqlite::Transaction<'_>,
    task: DbWriteTask,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<String>,
) -> rusqlite::Result<()> {
    match task {
        DbWriteTask::Agent { id, dto } => {
            DbWriteQueue::rusqlite_upsert_agent(tx, &id, &dto)?;
            owners.insert((id, "agent".to_string()));
        }
        DbWriteTask::Group { id, dto } => {
            DbWriteQueue::rusqlite_upsert_group(tx, &id, &dto)?;
            owners.insert((id, "group".to_string()));
        }
        DbWriteTask::Avatar {
            owner_type,
            owner_id,
            bytes,
        } => DbWriteQueue::rusqlite_upsert_avatar(tx, &owner_type, &owner_id, &bytes)?,
        DbWriteTask::AgentTopic { topic_id, dto } => {
            DbWriteQueue::rusqlite_upsert_agent_topic(tx, &topic_id, &dto)?;
            owners.insert((dto.owner_id, "agent".to_string()));
        }
        DbWriteTask::AgentTopicBatch { topics: batch } => {
            apply_agent_topics(tx, batch, owners)?;
        }
        DbWriteTask::GroupTopic { topic_id, dto } => {
            DbWriteQueue::rusqlite_upsert_group_topic(tx, &topic_id, &dto)?;
            owners.insert((dto.owner_id, "group".to_string()));
        }
        DbWriteTask::GroupTopicBatch { topics: batch } => {
            apply_group_topics(tx, batch, owners)?;
        }
        DbWriteTask::TopicMessages {
            topic_id,
            messages,
            compressed_contents,
            render_bytes,
            content_hashes,
            skip_bubble,
        } => {
            if !skip_bubble {
                topics.insert(topic_id.clone());
            }
            DbWriteQueue::rusqlite_upsert_messages_batch(
                tx,
                &topic_id,
                messages,
                compressed_contents,
                render_bytes,
                content_hashes,
            )?;
        }
        DbWriteTask::Flush { .. } => return Err(rusqlite::Error::InvalidQuery),
    }
    Ok(())
}

fn apply_agent_topics(
    tx: &rusqlite::Transaction<'_>,
    topics: Vec<(String, crate::vcp_modules::sync_dto::AgentTopicSyncDTO)>,
    owners: &mut HashSet<OwnerKey>,
) -> rusqlite::Result<()> {
    for (topic_id, dto) in topics {
        DbWriteQueue::rusqlite_upsert_agent_topic(tx, &topic_id, &dto)?;
        owners.insert((dto.owner_id, "agent".to_string()));
    }
    Ok(())
}

fn apply_group_topics(
    tx: &rusqlite::Transaction<'_>,
    topics: Vec<(String, crate::vcp_modules::sync_dto::GroupTopicSyncDTO)>,
    owners: &mut HashSet<OwnerKey>,
) -> rusqlite::Result<()> {
    for (topic_id, dto) in topics {
        DbWriteQueue::rusqlite_upsert_group_topic(tx, &topic_id, &dto)?;
        owners.insert((dto.owner_id, "group".to_string()));
    }
    Ok(())
}

fn bubble_topics(
    tx: &rusqlite::Transaction<'_>,
    topic_ids: HashSet<String>,
) -> rusqlite::Result<()> {
    for topic_id in topic_ids {
        DbWriteQueue::rusqlite_bubble_topic_hash(tx, &topic_id)?;
    }
    Ok(())
}

fn bubble_owners(
    tx: &rusqlite::Transaction<'_>,
    owners: HashSet<OwnerKey>,
) -> rusqlite::Result<()> {
    let (mut agents, mut groups) = split_owners(owners);
    agents.sort();
    groups.sort();
    bubble_agent_owners(tx, &agents)?;
    bubble_group_owners(tx, &groups)
}

fn split_owners(owners: HashSet<OwnerKey>) -> (Vec<String>, Vec<String>) {
    let mut agents = Vec::new();
    let mut groups = Vec::new();
    for (id, owner_type) in owners {
        match owner_type.as_str() {
            "agent" => agents.push(id),
            "group" => groups.push(id),
            _ => {}
        }
    }
    (agents, groups)
}

fn bubble_agent_owners(
    tx: &rusqlite::Transaction<'_>,
    requested: &[String],
) -> rusqlite::Result<()> {
    validate_owner_ids(tx, requested, "agents", "agent_id", "Agent")?;
    for id in requested {
        DbWriteQueue::rusqlite_bubble_agent_hash(tx, id)?;
    }
    Ok(())
}

fn bubble_group_owners(
    tx: &rusqlite::Transaction<'_>,
    requested: &[String],
) -> rusqlite::Result<()> {
    validate_owner_ids(tx, requested, "groups", "group_id", "Group")?;
    for id in requested {
        DbWriteQueue::rusqlite_bubble_group_hash(tx, id)?;
    }
    Ok(())
}

fn validate_owner_ids(
    tx: &rusqlite::Transaction<'_>,
    requested: &[String],
    table: &str,
    id_column: &str,
    label: &str,
) -> rusqlite::Result<()> {
    if requested.is_empty() {
        return Ok(());
    }
    let mut valid_ids = HashSet::new();
    for chunk in requested.chunks(400) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT {id_column} FROM {table} WHERE {id_column} IN ({placeholders}) AND deleted_at IS NULL"
        );
        let mut stmt = tx.prepare(&sql)?;
        let decoded = stmt
            .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        valid_ids.extend(decoded);
    }
    let expected = requested.iter().cloned().collect::<HashSet<_>>();
    if valid_ids != expected {
        let mut missing = expected.difference(&valid_ids).cloned().collect::<Vec<_>>();
        missing.sort();
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{label} hash bubble is missing live owners {missing:?}"
        )));
    }
    Ok(())
}
