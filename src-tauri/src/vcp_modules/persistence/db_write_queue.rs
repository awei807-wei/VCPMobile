use crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots;
use crate::vcp_modules::owner_lock::OwnerLockRegistry;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_types::MessageVersionState;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

mod attachments;
mod entity_group_upserts;
mod entity_upserts;
mod hash_bubbles;
mod message_batch;
mod message_deletes;
mod message_fts;
mod message_upserts;
mod worker;

pub(crate) const SNAPSHOT_STALE_MARKER: &str = "SYNC_SNAPSHOT_STALE";
pub(crate) type ExpectedMessageStates = BTreeMap<String, Option<MessageVersionState>>;

#[derive(Debug)]
pub enum DbWriteTask {
    Agent {
        id: String,
        dto: crate::vcp_modules::sync_dto::AgentSyncDTO,
        expected_config_hash: Option<String>,
    },
    Group {
        id: String,
        dto: crate::vcp_modules::sync_dto::GroupSyncDTO,
        expected_config_hash: Option<String>,
    },
    Avatar {
        owner_type: String,
        owner_id: String,
        bytes: Vec<u8>,
    },
    AgentTopic {
        topic_id: String,
        dto: crate::vcp_modules::sync_dto::AgentTopicSyncDTO,
    },
    AgentTopicBatch {
        topics: Vec<(String, crate::vcp_modules::sync_dto::AgentTopicSyncDTO)>,
    },
    GroupTopic {
        topic_id: String,
        dto: crate::vcp_modules::sync_dto::GroupTopicSyncDTO,
    },
    GroupTopicBatch {
        topics: Vec<(String, crate::vcp_modules::sync_dto::GroupTopicSyncDTO)>,
    },
    TopicMessages {
        topic_id: String,
        messages: Vec<crate::vcp_modules::chat_manager::ChatMessage>,
        compressed_contents: Vec<Vec<u8>>,
        render_bytes: Vec<Vec<u8>>,
        content_hashes: Vec<String>,
        skip_bubble: bool,
    },
    /// Canonical Wire 1.4 message write. The compressed body remains an
    /// explicit queue payload; the DTO is used for identity/hash metadata.
    TopicMessagesCanonical {
        topic: TopicKey,
        messages: Vec<crate::vcp_modules::sync_dto::MessageSyncDTO>,
        compressed_contents: Vec<Vec<u8>>,
        render_bytes: Vec<Vec<u8>>,
        expected_states: Option<ExpectedMessageStates>,
        skip_bubble: bool,
    },
    /// Composite-identity tombstone operations kept in the queue so a batch
    /// cannot accidentally delete another owner's same-named topic/message.
    DeleteTopic { topic: TopicKey, deleted_at: i64 },
    DeleteMessage {
        message: MessageKey,
        deleted_at: i64,
    },
    Flush {
        tx: oneshot::Sender<Result<(), String>>,
    },
}

pub struct DbWriteQueue {
    sender: mpsc::Sender<DbWriteTask>,
    logger: Option<Arc<Mutex<SyncLogger>>>,
    db_path: std::path::PathBuf,
    attachment_roots: Option<ManagedAttachmentRoots>,
    _worker: Option<tokio::task::JoinHandle<()>>,
}

impl Clone for DbWriteQueue {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            logger: self.logger.clone(),
            db_path: self.db_path.clone(),
            attachment_roots: self.attachment_roots.clone(),
            _worker: None,
        }
    }
}

impl DbWriteQueue {
    pub fn new(pool: sqlx::SqlitePool, db_path: std::path::PathBuf) -> Self {
        Self::new_with_owner_locks(pool, db_path, OwnerLockRegistry::new())
    }

    pub fn new_with_owner_locks(
        pool: sqlx::SqlitePool,
        db_path: std::path::PathBuf,
        owner_locks: std::sync::Arc<OwnerLockRegistry>,
    ) -> Self {
        Self::new_with_owner_locks_and_attachment_roots(pool, db_path, owner_locks, None)
    }

    pub fn new_with_owner_locks_and_attachment_roots(
        _pool: sqlx::SqlitePool,
        db_path: std::path::PathBuf,
        owner_locks: std::sync::Arc<OwnerLockRegistry>,
        attachment_roots: Option<ManagedAttachmentRoots>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let worker = worker::spawn(db_path.clone(), rx, owner_locks, attachment_roots.clone());
        Self {
            sender: tx,
            logger: None,
            db_path,
            attachment_roots,
            _worker: Some(worker),
        }
    }

    pub fn set_logger(&mut self, logger: Arc<Mutex<SyncLogger>>) {
        self.logger = Some(logger);
    }

    pub async fn submit(&self, task: DbWriteTask) -> Result<(), String> {
        self.sender
            .send(task)
            .await
            .map_err(|error| format!("DbWriteQueue submit failed: {error}"))
    }

    pub async fn flush(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(DbWriteTask::Flush { tx })
            .await
            .map_err(|error| format!("DbWriteQueue flush submit failed: {error}"))?;
        rx.await
            .map_err(|error| format!("DbWriteQueue flush acknowledgement failed: {error}"))??;
        log::debug!("[DbWriteQueue] Flush completed");
        Ok(())
    }

    pub(super) fn sync_contract_error(message: impl Into<String>) -> rusqlite::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.into(),
        )))
    }

    pub(super) fn sync_snapshot_stale_error(
        message: impl Into<String>,
        stage: crate::vcp_modules::sync_error::SyncErrorStage,
    ) -> rusqlite::Error {
        let message = message.into();
        Self::sync_contract_error(crate::vcp_modules::sync_error::encode_local_sync_error(
            SNAPSHOT_STALE_MARKER,
            stage,
            &message,
            Vec::new(),
        ))
    }

    pub(super) fn take_pending_errors(errors: &mut Vec<String>) -> Result<(), String> {
        if errors.is_empty() {
            Ok(())
        } else {
            Err(std::mem::take(errors).join(" | "))
        }
    }

    /// Resolve a legacy topic-only queue call only when the id maps to one
    /// live composite topic. Ambiguous ids fail closed instead of guessing.
    pub(super) fn rusqlite_resolve_topic_key(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
    ) -> rusqlite::Result<TopicKey> {
        if topic_id.is_empty() {
            return Err(Self::sync_contract_error(
                "Topic identity requires a non-empty topic id",
            ));
        }
        let mut statement = tx.prepare_cached(
            "SELECT owner_type, owner_id FROM topics
             WHERE topic_id = ? AND deleted_at IS NULL
             ORDER BY owner_type, owner_id",
        )?;
        let rows = statement
            .query_map([topic_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let [(owner_type, owner_id)] = rows.as_slice() else {
            return Err(Self::sync_contract_error(if rows.is_empty() {
                format!("Topic {topic_id} is missing or deleted")
            } else {
                format!("Topic {topic_id} is ambiguous across {} owners", rows.len())
            }));
        };
        let key = TopicKey::new(owner_type.clone(), owner_id.clone(), topic_id);
        if key.is_valid() {
            Ok(key)
        } else {
            Err(Self::sync_contract_error(format!(
                "Topic {topic_id} has invalid owner identity"
            )))
        }
    }
}

#[cfg(test)]
#[path = "db_write_queue_contract_tests.rs"]
mod contract_tests;
