//! Wire 1.4 synchronization data-transfer objects.
//!
//! The implementation is split by contract boundary so the module root stays
//! a stable import surface while Wire 1.4 canonical DTOs evolve independently.

mod attachments;
mod canonical;
mod entities;

pub use attachments::AttachmentSyncDTO;
pub use canonical::MessageSyncDTO;
pub use entities::{
    normalize_member_tags, AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};

#[cfg(test)]
#[path = "sync_dto_tests.rs"]
mod tests;
