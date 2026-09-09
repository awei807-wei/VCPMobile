use super::{DbWriteQueue, DbWriteTask};

use crate::vcp_modules::owner_lock::{OwnerLockHandle, OwnerLockRegistry};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use rusqlite::{Connection, TransactionBehavior};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

#[path = "worker_apply.rs"]
mod apply;

type ConnectionHolder = Arc<Mutex<Option<Connection>>>;

pub(super) fn spawn(
    db_path: PathBuf,
    rx: mpsc::Receiver<DbWriteTask>,
    owner_locks: Arc<OwnerLockRegistry>,
    attachment_roots: Option<
        crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots,
    >,
) -> JoinHandle<()> {
    let holder: ConnectionHolder = Arc::new(Mutex::new(None));
    tokio::spawn(run(rx, db_path, holder, owner_locks, attachment_roots))
}

async fn run(
    mut rx: mpsc::Receiver<DbWriteTask>,
    db_path: PathBuf,
    holder: ConnectionHolder,
    owner_locks: Arc<OwnerLockRegistry>,
    attachment_roots: Option<
        crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots,
    >,
) {
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
        let result = execute_batch(
            tasks,
            db_path.clone(),
            holder.clone(),
            owner_locks.clone(),
            attachment_roots.clone(),
        )
        .await;
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
        DbWriteTask::TopicMessagesCanonical { messages, .. } => messages.len() as u32,
        _ => 0,
    }
}

async fn execute_batch(
    tasks: Vec<DbWriteTask>,
    db_path: PathBuf,
    holder: ConnectionHolder,
    owner_locks: Arc<OwnerLockRegistry>,
    attachment_roots: Option<
        crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots,
    >,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        execute_batch_sync_with_owner_locks(tasks, db_path, holder, owner_locks, attachment_roots)
    })
    .await
    .map_err(|error| format!("write worker join error: {error}"))?
    .map_err(|error| format!("rusqlite execution error: {error}"))
}

fn execute_batch_sync(
    tasks: Vec<DbWriteTask>,
    db_path: PathBuf,
    holder: ConnectionHolder,
) -> rusqlite::Result<()> {
    execute_batch_sync_with_owner_locks(tasks, db_path, holder, OwnerLockRegistry::new(), None)
}

fn execute_batch_sync_with_owner_locks(
    tasks: Vec<DbWriteTask>,
    db_path: PathBuf,
    holder: ConnectionHolder,
    owner_locks: Arc<OwnerLockRegistry>,
    attachment_roots: Option<
        crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots,
    >,
) -> rusqlite::Result<()> {
    let _attachment_gate = crate::vcp_modules::file_manager::attachment_gc_gate_blocking_read();
    let owner_handles = acquire_owner_locks(&owner_locks, &tasks);
    let _owner_guards = owner_handles
        .iter()
        .map(OwnerLockHandle::blocking_lock)
        .collect::<Vec<_>>();
    let mut guard = holder.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
    if guard.is_none() {
        let conn = open_connection(&db_path)?;
        *guard = Some(conn);
    }
    let conn = guard.as_mut().ok_or(rusqlite::Error::InvalidQuery)?;
    // Topic/entity upserts validate their parent rows before writing. A deferred
    // transaction can therefore acquire a read snapshot, lose a race to a
    // SQLx writer, and fail immediately with SQLITE_BUSY_SNAPSHOT when it is
    // upgraded to a writer. Reserve the WAL writer slot before any validation
    // reads so SQLite's busy handler can wait safely instead of failing the
    // whole sync drain.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (owners, topics) = apply_tasks(&tx, tasks, attachment_roots.as_ref())?;
    bubble_topics(&tx, topics)?;
    bubble_owners(&tx, owners)?;
    tx.commit()
}

fn acquire_owner_locks(
    registry: &Arc<OwnerLockRegistry>,
    tasks: &[DbWriteTask],
) -> Vec<OwnerLockHandle> {
    let mut keys = tasks
        .iter()
        .filter_map(|task| match task {
            DbWriteTask::Agent { id, .. } => Some(("agent", id.as_str())),
            DbWriteTask::Group { id, .. } => Some(("group", id.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .map(|(owner_type, owner_id)| registry.acquire_owner(owner_type, owner_id))
        .collect()
}

fn open_connection(db_path: &PathBuf) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_millis(30000))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

fn apply_tasks(
    tx: &rusqlite::Transaction<'_>,
    tasks: Vec<DbWriteTask>,
    attachment_roots: Option<
        &crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots,
    >,
) -> rusqlite::Result<(HashSet<OwnerKey>, HashSet<TopicKey>)> {
    let mut owners = HashSet::new();
    let mut topics = HashSet::new();
    for task in tasks {
        apply_task(tx, task, &mut owners, &mut topics, attachment_roots)?;
    }
    Ok((owners, topics))
}

fn apply_task(
    tx: &rusqlite::Transaction<'_>,
    task: DbWriteTask,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
    attachment_roots: Option<
        &crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots,
    >,
) -> rusqlite::Result<()> {
    apply::apply_task(tx, task, owners, topics, attachment_roots)
}

fn apply_agent_topics(
    tx: &rusqlite::Transaction<'_>,
    batch: Vec<(String, crate::vcp_modules::sync_dto::AgentTopicSyncDTO)>,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    for (topic_id, dto) in batch {
        let key = TopicKey::new("agent", dto.owner_id.clone(), topic_id);
        DbWriteQueue::rusqlite_upsert_agent_topic_for_key(tx, &key, &dto)?;
        owners.insert(key.owner_key());
        topics.insert(key);
    }
    Ok(())
}

fn apply_group_topics(
    tx: &rusqlite::Transaction<'_>,
    batch: Vec<(String, crate::vcp_modules::sync_dto::GroupTopicSyncDTO)>,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    for (topic_id, dto) in batch {
        let key = TopicKey::new("group", dto.owner_id.clone(), topic_id);
        DbWriteQueue::rusqlite_upsert_group_topic_for_key(tx, &key, &dto)?;
        owners.insert(key.owner_key());
        topics.insert(key);
    }
    Ok(())
}

fn bubble_topics(
    tx: &rusqlite::Transaction<'_>,
    topics: HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    for topic in topics {
        DbWriteQueue::rusqlite_bubble_topic_hash_for_key(tx, &topic)?;
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
    for owner in owners {
        match owner.owner_type.as_str() {
            "agent" => agents.push(owner.owner_id),
            "group" => groups.push(owner.owner_id),
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

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
