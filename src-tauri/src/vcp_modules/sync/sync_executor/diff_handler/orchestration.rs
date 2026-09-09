use super::action_dispatch;
use super::context::DiffContext;
use super::manifest::{consume_manifest_response_type, validate_manifest_result};
use super::phase;
use crate::vcp_modules::sync_types::ManifestResultFrame;
use std::sync::atomic::Ordering;

pub(crate) async fn handle_diff(
    ctx: &DiffContext,
    result: ManifestResultFrame,
) -> Result<(), String> {
    let (manifest_type, decisions) = validate_manifest_result(result)?;
    let current_phase = ctx.manifest_phase.load(Ordering::SeqCst);
    let all_types_received =
        consume_manifest_response_type(manifest_type, current_phase, &ctx.expected_manifest_types)?;
    phase::record_manifest(
        ctx,
        manifest_type,
        &decisions,
        current_phase,
        all_types_received,
    )
    .await?;
    let buckets = action_dispatch::classify_items(manifest_type, decisions, ctx).await?;
    action_dispatch::dispatch_buckets(ctx, buckets).await;
    Ok(())
}
