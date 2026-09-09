mod action_dispatch;
mod context;
mod delete_dispatch;
mod manifest;
mod orchestration;
mod phase;
mod topic_push;

use crate::vcp_modules::sync_types::ManifestResultFrame;

pub struct DiffHandler;
pub(crate) use context::{DiffContext, DiffContextParams};

#[cfg(test)]
pub(crate) use manifest::{
    consume_manifest_response_type, next_manifest_command, validate_manifest_result,
};

impl DiffHandler {
    pub async fn handle_diff(
        context: context::DiffContext,
        result: ManifestResultFrame,
    ) -> Result<(), String> {
        orchestration::handle_diff(&context, result).await
    }
}

#[cfg(test)]
#[path = "diff_handler_tests.rs"]
mod tests;
