use super::action_dispatch;
use super::context::DiffContext;
use super::diff_item_validation::{validate_and_filter_diff_items, validate_diff_frame};
use super::manifest::consume_manifest_response_type;
use super::phase;
use serde_json::Value;
use std::sync::atomic::Ordering;

pub(crate) async fn handle_diff(ctx: &DiffContext, payload: &Value) -> Result<(), String> {
    let items = validate_diff_frame(payload, &ctx.data_type)?;
    let (items, exempt_default_topics) = validate_and_filter_diff_items(items, &ctx.data_type)?;
    log_default_topic_exemptions(ctx, exempt_default_topics);

    let current_phase = ctx.manifest_phase.load(Ordering::SeqCst);
    let all_types_received = consume_manifest_response_type(
        payload,
        &ctx.data_type,
        current_phase,
        &ctx.expected_manifest_types,
    )?;
    phase::record_manifest(ctx, &items, current_phase, all_types_received).await?;
    let buckets = action_dispatch::classify_items(items, ctx).await?;
    action_dispatch::dispatch_buckets(ctx, buckets).await;
    Ok(())
}

fn log_default_topic_exemptions(ctx: &DiffContext, count: u32) {
    if count > 0 {
        log::info!(
            "[Sync] Exempted {count} default-topic action(s) from {} diff results (contract: default topics are not synced)",
            ctx.data_type
        );
    }
}
