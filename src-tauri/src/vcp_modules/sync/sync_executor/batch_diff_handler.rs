mod context;
mod decision;
mod error;
mod next_batch;
mod operations;
mod outcome;
mod plan;
mod protocol;

pub use error::Phase3ProtocolError;
#[cfg(test)]
pub use protocol::parse_message_diff_result_frame;

use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_service::{
    Phase3DiffBatch, Phase3MessageSnapshots, Phase3Tracker, SyncCommand,
};
use crate::vcp_modules::sync_types::MessageDiffResultFrame;
use crate::vcp_modules::topic_types::TopicKey;
use context::BatchContext;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

pub struct BatchDiffHandler;

impl BatchDiffHandler {
    /// Apply one strict Wire 1.4 message-diff result and stop on the first failure.
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_diff_batch(
        app_handle: &AppHandle,
        frame: MessageDiffResultFrame,
        http_client: &reqwest::Client,
        base_url: &str,
        token: &str,
        tracker: &Arc<Phase3Tracker>,
        tx_internal: &mpsc::UnboundedSender<SyncCommand>,
        logger: &Arc<Mutex<crate::vcp_modules::sync_logger::SyncLogger>>,
        write_queue: &Arc<DbWriteQueue>,
        pending_diff_batches: &Arc<AsyncMutex<VecDeque<Phase3DiffBatch>>>,
        prerender_enabled: bool,
        expected_batch_topics: &Arc<AsyncMutex<HashSet<TopicKey>>>,
        expected_message_states: &Arc<AsyncMutex<Phase3MessageSnapshots>>,
        attempt_id: u64,
    ) -> Result<(), Phase3ProtocolError> {
        let context = BatchContext {
            app: app_handle,
            client: http_client,
            base_url,
            token,
            tracker,
            tx: tx_internal,
            logger,
            write_queue,
            pending_batches: pending_diff_batches,
            prerender_enabled,
            expected_topics: expected_batch_topics,
            expected_states: expected_message_states,
            attempt_id,
        };
        operations::handle_batch(context, frame).await
    }
}

#[cfg(test)]
#[path = "batch_diff_handler_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) use decision::{parse_topic_decision, TopicDecision};
#[cfg(test)]
pub(crate) use outcome::{validate_topic_batch_outcomes, TopicBatchOutcome};
#[cfg(test)]
pub(crate) use plan::validate_phase3_result_topics;
