mod action_dispatch;
mod context;
mod delete_dispatch;
mod diff_item_validation;
mod manifest;
mod orchestration;
mod phase;
mod topic_push;

pub struct DiffHandler;
pub(crate) use context::{DiffContext, DiffContextParams};

#[cfg(test)]
pub(crate) use diff_item_validation::{
    parse_delete_timestamp, validate_and_filter_diff_items, validate_diff_frame,
};
#[cfg(test)]
pub(crate) use manifest::{consume_manifest_response_type, next_manifest_command};

impl DiffHandler {
    pub async fn handle_diff(
        context: context::DiffContext,
        payload: &serde_json::Value,
    ) -> Result<(), String> {
        orchestration::handle_diff(&context, payload).await
    }
}

#[cfg(test)]
#[path = "diff_handler_tests.rs"]
mod tests;
