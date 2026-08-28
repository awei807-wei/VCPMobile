use super::registry_contexts::CONTEXTS;
use super::registry_semantics::SEMANTICS;
use super::types::{
    ErrorDefinition, SyncErrorCategory as Category, SyncErrorOrigin as Origin,
    SyncErrorStage as Stage, SyncRetryAction as Retry,
};

pub(crate) fn error_definition(code: &str) -> Option<ErrorDefinition> {
    let (_, category, retry) = SEMANTICS
        .iter()
        .find(|(registered, _, _)| *registered == code)
        .copied()?;
    let (_, origin, stage) = CONTEXTS
        .iter()
        .find(|(registered, _, _)| *registered == code)
        .copied()
        .unwrap_or((code, Origin::DesktopPlugin, Stage::Startup));
    Some(ErrorDefinition {
        category,
        origin,
        stage,
        retry,
    })
}

pub(crate) fn fallback_definition() -> ErrorDefinition {
    ErrorDefinition {
        category: Category::Internal,
        origin: Origin::MobileSync,
        stage: Stage::Startup,
        retry: Retry::Manual,
    }
}
