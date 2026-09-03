//! Topic lifecycle commands, split by query, mutation, archive and regeneration duties.

#[path = "topic_service_archive.rs"]
mod archive;
#[path = "topic_service_listing.rs"]
mod listing;
#[path = "topic_service_mutations.rs"]
mod mutations;
#[path = "topic_service_regeneration.rs"]
mod regeneration;

#[allow(unused_imports)]
pub(crate) use crate::vcp_modules::topic_types::TopicKey;
pub use archive::{archive_assistant_chat, TempMessage};
pub use listing::{get_topics, get_topics_streamed, get_unread_counts};
#[allow(unused_imports)]
pub(crate) use mutations::set_topic_unread_in_pool;
pub use mutations::{
    create_topic, delete_topic, set_topic_unread, summarize_topic, toggle_topic_lock,
    update_topic_title,
};
pub use regeneration::regenerate_topic_response;

#[cfg(test)]
#[path = "topic_service_tests.rs"]
mod tests;
