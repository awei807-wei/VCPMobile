use std::path::PathBuf;

use super::db::IndexedAttachment;
use super::paths::{validate_indexed_path, ManagedAttachmentRoots, ManagedPathState};

#[derive(Debug, Clone, Default)]
pub(super) struct RecordPaths {
    pub(super) internal: Option<PathBuf>,
    pub(super) thumbnail: Option<PathBuf>,
    pub(super) safe_for_delete: bool,
}

pub(super) fn validate_record_paths(
    record: &IndexedAttachment,
    roots: &ManagedAttachmentRoots,
) -> RecordPaths {
    let internal = if record.internal_path.is_empty() {
        Err("附件没有可管理的 internal_path".to_string())
    } else {
        validate_indexed_path(&roots.attachments, &record.internal_path)
    };
    let thumbnail = record
        .thumbnail_path
        .as_deref()
        .filter(|path| !path.is_empty())
        .map(|path| validate_indexed_path(&roots.thumbnails, path));
    let internal_path = internal
        .as_ref()
        .ok()
        .and_then(|(path, state)| (state == &ManagedPathState::Present).then(|| path.clone()));
    let thumbnail_path = thumbnail.as_ref().and_then(|result| {
        result
            .as_ref()
            .ok()
            .and_then(|(path, state)| (state == &ManagedPathState::Present).then(|| path.clone()))
    });
    RecordPaths {
        internal: internal_path,
        thumbnail: thumbnail_path,
        safe_for_delete: internal.is_ok() && thumbnail.as_ref().is_none_or(Result::is_ok),
    }
}
