//! Group configuration facade with focused read, write and lifecycle modules.

#[path = "group_service_lifecycle.rs"]
mod lifecycle;
#[path = "group_service_listing.rs"]
mod listing;
#[path = "group_service_read.rs"]
mod read;
#[path = "group_service_state.rs"]
mod state;
#[path = "group_service_write.rs"]
mod write;

pub use lifecycle::{create_group, delete_group};
pub use listing::get_groups;
pub use read::read_group_config;
#[allow(unused_imports)]
pub use read::read_group_config_internal;
pub(crate) use read::read_group_config_locked;
pub use state::GroupManagerState;
#[allow(unused_imports)]
pub(crate) use write::internal_write_group_config;
pub use write::{save_group_config, update_group_config};

#[cfg(test)]
#[path = "group_service_tests.rs"]
mod composite_identity_tests;

#[cfg(test)]
#[path = "group_service_lifecycle_tests.rs"]
mod lifecycle_tests;
