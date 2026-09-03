//! Message-service facade for Wire 1.4 composite message identity.
//!
//! The implementation is split by responsibility so query, attachment,
//! mutation, deletion and stream paths can evolve independently. Existing
//! public paths are re-exported here to keep callers source-compatible while
//! owner-aware `*_for_key` APIs are available for new callers.

#[path = "message_service_batch.rs"]
mod message_service_batch;
#[path = "message_service_context.rs"]
mod message_service_context;
#[path = "message_service_deletions.rs"]
mod message_service_deletions;
#[path = "message_service_history.rs"]
mod message_service_history;
#[path = "message_service_mutations.rs"]
mod message_service_mutations;
#[path = "message_service_stream.rs"]
mod message_service_stream;
#[path = "message_service_support.rs"]
mod message_service_support;

#[allow(unused_imports)]
pub use message_service_batch::{load_multi_topic_messages, load_multi_topic_messages_for_keys};
#[allow(unused_imports)]
pub use message_service_context::{
    load_chat_text_history_for_context, load_chat_text_history_for_topic,
};
#[allow(unused_imports)]
pub use message_service_deletions::{
    delete_message_attachment, delete_message_attachment_for_key, delete_messages,
    delete_messages_for_topic, truncate_history_after_timestamp,
    truncate_history_after_timestamp_for_topic,
};
pub use message_service_history::load_chat_history_internal;
#[allow(unused_imports)]
pub use message_service_mutations::{
    append_single_message, fetch_raw_message_content, fetch_raw_message_content_for_key,
    patch_single_message, re_render_message, re_render_message_for_key,
};
pub use message_service_stream::finalize_stream_message;

#[cfg(test)]
#[path = "message_service_tests.rs"]
mod message_service_tests;
