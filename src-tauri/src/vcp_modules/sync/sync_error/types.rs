use serde::{Deserialize, Serialize};

pub const MAX_ERROR_MESSAGE_CHARS: usize = 1024;
pub const MAX_FAILED_TOPIC_IDS: usize = 8;
pub const MAX_TOPIC_ID_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncErrorOrigin {
    MobileUi,
    MobileNative,
    MobileSync,
    DesktopPlugin,
    DesktopCds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncErrorStage {
    Preflight,
    Startup,
    Connect,
    Handshake,
    OwnerMetadata,
    TopicMetadata,
    TopicValidation,
    Messages,
    Finalize,
    Shutdown,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncErrorCategory {
    Device,
    Configuration,
    Connection,
    Compatibility,
    Protocol,
    Data,
    Storage,
    Internal,
}

pub type SyncErrorKind = SyncErrorCategory;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRetryAction {
    Automatic,
    AfterUserAction,
    Manual,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireSyncError {
    pub code: String,
    pub origin: SyncErrorOrigin,
    pub stage: SyncErrorStage,
    pub kind: SyncErrorCategory,
    pub retry: SyncRetryAction,
    pub message: String,
    pub failed_topic_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncErrorPayload {
    pub code: String,
    pub category: SyncErrorCategory,
    pub origin: SyncErrorOrigin,
    pub stage: SyncErrorStage,
    pub retry_action: SyncRetryAction,
    pub message: String,
    pub guidance: String,
    pub failed_topic_ids: Vec<String>,
    pub log_file: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct ErrorDefinition {
    pub(crate) category: SyncErrorCategory,
    pub(crate) origin: SyncErrorOrigin,
    pub(crate) stage: SyncErrorStage,
    pub(crate) retry: SyncRetryAction,
}
