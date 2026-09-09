use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct ManagedFileCandidate {
    pub(super) path: PathBuf,
    pub(super) metadata: std::fs::Metadata,
}

/// A bounded max-heap entry. Keeping the largest selected path at the root
/// lets the traversal retain only the globally smallest `limit` paths without
/// taking a snapshot of any directory.
#[derive(Debug)]
struct SelectedCandidate {
    relative: String,
    path: PathBuf,
    metadata: std::fs::Metadata,
}

impl PartialEq for SelectedCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.relative == other.relative && self.path == other.path
    }
}

impl Eq for SelectedCandidate {}

impl Ord for SelectedCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.relative
            .cmp(&other.relative)
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for SelectedCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default)]
pub(crate) struct ScanPage {
    pub(super) candidates: Vec<ManagedFileCandidate>,
    pub(super) next_cursor: String,
    pub(super) has_more: bool,
}

/// Scan a bounded lexicographic page after a persisted relative cursor.
/// Directories are traversed without following symlinks; one look-ahead item
/// establishes `has_more` without taking an unbounded file snapshot.
pub(crate) async fn scan_managed_root_from_cursor(
    root: &Path,
    cursor: &str,
    max_entries: usize,
) -> ScanPage {
    let Ok(root) = super::canonical_managed_root(root) else {
        return ScanPage::default();
    };
    let cursor = super::normalize_scan_cursor(cursor);
    let selection_limit = max_entries.saturating_add(1);
    let mut selected = BinaryHeap::with_capacity(selection_limit);
    collect_candidates_after_cursor(&root, &root, &cursor, selection_limit, &mut selected).await;
    let mut selected = selected.into_vec();
    selected.sort_unstable();
    let has_more = selected.len() > max_entries;
    selected.truncate(max_entries);
    let next_cursor = selected
        .last()
        .map(|candidate| candidate.relative.clone())
        .unwrap_or_default();
    let candidates = selected
        .into_iter()
        .map(|candidate| ManagedFileCandidate {
            path: candidate.path,
            metadata: candidate.metadata,
        })
        .collect();
    ScanPage {
        candidates,
        next_cursor,
        has_more,
    }
}

async fn collect_candidates_after_cursor(
    root: &Path,
    directory: &Path,
    cursor: &str,
    limit: usize,
    selected: &mut BinaryHeap<SelectedCandidate>,
) {
    if limit == 0 {
        return;
    }
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Ok(metadata) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        if metadata.file_type().is_dir() {
            Box::pin(collect_candidates_after_cursor(
                root, &path, cursor, limit, selected,
            ))
            .await;
            continue;
        }
        if !metadata.file_type().is_file() {
            continue;
        }
        let Ok(relative_path) = path.strip_prefix(root) else {
            continue;
        };
        let Ok(relative) = super::relative_to_string(relative_path) else {
            continue;
        };
        if relative.as_str() <= cursor {
            continue;
        }
        retain_smallest_candidate(
            selected,
            limit,
            SelectedCandidate {
                relative,
                path,
                metadata,
            },
        );
    }
}

fn retain_smallest_candidate(
    selected: &mut BinaryHeap<SelectedCandidate>,
    limit: usize,
    candidate: SelectedCandidate,
) {
    if limit == 0 {
        return;
    }
    if selected.len() == limit {
        let Some(largest) = selected.peek() else {
            return;
        };
        if candidate.cmp(largest) != Ordering::Less {
            return;
        }
        let _ = selected.pop();
    }
    selected.push(candidate);
}
