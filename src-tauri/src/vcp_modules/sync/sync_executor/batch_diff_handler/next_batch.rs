use super::context::BatchContext;
use crate::vcp_modules::sync_service::SyncCommand;

pub(crate) async fn send_next_batch(ctx: BatchContext<'_>) {
    let (next, remaining) = {
        let mut pending = ctx.pending_batches.lock().await;
        let next = pending.pop_front();
        (next, pending.len())
    };
    let Some(next) = next else {
        return;
    };
    log::debug!("[SyncService] Sending next message diff batch, {remaining} remaining");
    {
        let mut expected = ctx.expected_states.lock().await;
        *expected = next.message_snapshots();
    }
    {
        let mut expected = ctx.expected_topics.lock().await;
        *expected = next.keys.clone();
    }
    let _ = ctx.tx.send(SyncCommand::SendMessageDiff {
        attempt_id: ctx.attempt_id,
        topics: next.topics,
    });
}
