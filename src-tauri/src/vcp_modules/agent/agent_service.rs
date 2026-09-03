//! Agent configuration facade with focused modules for reads, writes, lifecycle and snapshots.

#[path = "agent_service_lifecycle.rs"]
mod lifecycle;
#[path = "agent_service_read.rs"]
mod read;
#[path = "agent_service_snapshot.rs"]
mod snapshot;
#[path = "agent_service_state.rs"]
mod state;
#[path = "agent_service_write.rs"]
mod write;

pub use lifecycle::{create_agent, delete_agent};
pub use read::{get_agents, read_agent_config, read_agent_config_internal};
pub use snapshot::get_assistants_snapshot;
#[allow(unused_imports)]
pub use snapshot::AssistantsSnapshot;
pub use state::{create_default_config, AgentConfigState};
pub use write::{save_agent_config, update_agent_config};

#[cfg(test)]
#[path = "agent_service_tests.rs"]
mod composite_identity_tests;
