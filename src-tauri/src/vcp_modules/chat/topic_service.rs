//! Topic lifecycle commands, split by query, mutation, archive and regeneration duties.

#[path = "topic_service_archive.rs"]
mod archive;
#[path = "topic_service_listing.rs"]
mod listing;
#[path = "topic_service_mutations.rs"]
mod mutations;
#[path = "topic_service_regeneration.rs"]
mod regeneration;
#[path = "topic_service_unread.rs"]
mod unread;

#[allow(unused_imports)]
pub(crate) use crate::vcp_modules::topic_types::TopicKey;
pub use archive::{archive_assistant_chat, TempMessage};
pub(crate) use listing::unread_owner_key;
pub use listing::{get_topics, get_topics_streamed, get_unread_counts};
#[allow(unused_imports)]
pub use mutations::{
    create_topic, delete_topic, summarize_topic, toggle_topic_lock, update_topic_title,
};
pub use regeneration::regenerate_topic_response;
#[allow(unused_imports)]
pub use unread::{
    get_owner_unread_count, increment_topic_unread_count, set_topic_unread, OwnerUnreadState,
    TopicUnreadState,
};
#[allow(unused_imports)]
pub(crate) use unread::{
    increment_topic_unread_count_in_pool, record_topic_unread_for_message_in_pool,
    record_topic_unread_for_message_in_tx, set_topic_unread_in_pool,
};

#[cfg(test)]
#[path = "topic_service_mutation_tests.rs"]
mod mutation_tests;
#[cfg(test)]
#[path = "topic_service_tests.rs"]
mod tests;
