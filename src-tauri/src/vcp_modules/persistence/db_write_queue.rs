use crate::vcp_modules::sync_logger::SyncLogger;

use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

mod attachments;
mod entity_upserts;
mod hash_bubbles;
mod message_upserts;
mod worker;

#[derive(Debug)]
pub enum DbWriteTask {
    Agent {
        id: String,
        dto: crate::vcp_modules::sync_dto::AgentSyncDTO,
    },
    Group {
        id: String,
        dto: crate::vcp_modules::sync_dto::GroupSyncDTO,
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
    Flush {
        tx: oneshot::Sender<Result<(), String>>,
    },
}

pub struct DbWriteQueue {
    sender: mpsc::Sender<DbWriteTask>,
    logger: Option<Arc<Mutex<SyncLogger>>>,
    db_path: std::path::PathBuf,
    _worker: Option<tokio::task::JoinHandle<()>>,
}

impl Clone for DbWriteQueue {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            logger: self.logger.clone(),
            db_path: self.db_path.clone(),
            _worker: None,
        }
    }
}

impl DbWriteQueue {
    pub fn new(_pool: sqlx::SqlitePool, db_path: std::path::PathBuf) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let worker = worker::spawn(db_path.clone(), rx);
        Self {
            sender: tx,
            logger: None,
            db_path,
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

    pub(super) fn take_pending_errors(errors: &mut Vec<String>) -> Result<(), String> {
        if errors.is_empty() {
            Ok(())
        } else {
            Err(std::mem::take(errors).join(" | "))
        }
    }
}

#[cfg(test)]
#[path = "db_write_queue_contract_tests.rs"]
mod contract_tests;
