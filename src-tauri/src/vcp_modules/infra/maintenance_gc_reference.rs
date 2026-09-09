use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[path = "maintenance_gc_reference_queries.rs"]
mod queries;

use super::paths::{
    canonical_managed_root, path_may_be_inside_managed_root, validate_reference_path,
    validate_reference_path_from_root, ManagedAttachmentRoots, RootKind,
};

#[derive(Debug, Clone)]
pub(super) struct IndexedAttachment {
    pub(super) hash: String,
    pub(super) internal_path: String,
    pub(super) thumbnail_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PathReferenceState {
    Unreferenced,
    Referenced,
    Uncertain,
}

const REFERENCE_QUERY_BATCH_SIZE: usize = 256;

/// A bounded snapshot of only the paths and hashes that the current GC page
/// can touch. It is intentionally not a snapshot of the attachments table:
/// callers must provide the candidate keys before loading this index.
#[derive(Debug, Default)]
pub(super) struct PathReferenceIndex {
    path_references: HashMap<PathBuf, usize>,
    indexed_hashes: HashSet<String>,
    live_hashes: HashSet<String>,
    hash_paths: HashMap<String, HashSet<PathBuf>>,
    uncertain_roots: HashSet<PathBuf>,
    uncertain_paths: HashSet<PathBuf>,
}

impl PathReferenceIndex {
    /// Load references for one attachment-index page. The hash query also
    /// picks up aliases that use a different raw path but carry the same CAS
    /// hash, while path queries account for all rows sharing an exact path.
    pub(super) async fn load_for_records(
        connection: &mut sqlx::SqliteConnection,
        roots: &ManagedAttachmentRoots,
        records: &[IndexedAttachment],
    ) -> Result<Self, String> {
        let mut keys = ReferenceKeys::default();
        for record in records {
            keys.hashes.push(record.hash.clone());
            keys.add_internal_path(&record.internal_path);
            if let Some(path) = record.thumbnail_path.as_deref() {
                keys.add_thumbnail_path(path);
            }
        }
        Self::load_for_keys(connection, roots, keys).await
    }

    /// Load references for a bounded set of physical paths and CAS hashes.
    /// This is used by the ghost-file and unlink pages after their filesystem
    /// candidates have already been bounded. Every SQL statement has at most
    /// `REFERENCE_QUERY_BATCH_SIZE` keys and grouped path queries return at
    /// most one row per requested raw path.
    pub(super) async fn load_for_paths(
        connection: &mut sqlx::SqliteConnection,
        roots: &ManagedAttachmentRoots,
        paths: &[(RootKind, PathBuf)],
        hashes: &[String],
    ) -> Result<Self, String> {
        let mut keys = ReferenceKeys::default();
        for (kind, path) in paths {
            match kind {
                RootKind::Attachment => {
                    if let Some(path) = path.to_str() {
                        keys.add_internal_path(path);
                    }
                }
                RootKind::Thumbnail => {
                    if let Some(path) = path.to_str() {
                        keys.add_thumbnail_path(path);
                    }
                }
                RootKind::MultimodalCache => {}
            }
        }
        keys.hashes.extend(hashes.iter().cloned());
        Self::load_for_keys(connection, roots, keys).await
    }

    async fn load_for_keys(
        connection: &mut sqlx::SqliteConnection,
        roots: &ManagedAttachmentRoots,
        keys: ReferenceKeys,
    ) -> Result<Self, String> {
        let attachments_root = canonical_managed_root(&roots.attachments)?;
        let thumbnails_root = canonical_managed_root(&roots.thumbnails)?;
        let internal_keys = keys.internal_paths.iter().cloned().collect::<HashSet<_>>();
        let thumbnail_keys = keys.thumbnail_paths.iter().cloned().collect::<HashSet<_>>();
        let mut index = Self::default();

        for chunk in keys.hashes.chunks(REFERENCE_QUERY_BATCH_SIZE) {
            let rows = queries::query_attachment_rows_by_hash(connection, chunk).await?;
            for (hash, internal_path, thumbnail_path) in rows {
                index.indexed_hashes.insert(hash.to_ascii_lowercase());
                index.add_hash_target(&attachments_root, &hash, &internal_path);
                // Path queries below return exact raw-path counts. Avoid
                // double-counting rows that were already found by hash, but
                // retain non-key aliases so symlink/ancestor uncertainty is
                // still represented in the bounded index.
                if !internal_keys.contains(&internal_path) {
                    index.add_record_path_count(&attachments_root, &internal_path, 1);
                }
                if let Some(thumbnail_path) = thumbnail_path {
                    index.add_hash_target(&thumbnails_root, &hash, &thumbnail_path);
                    if !thumbnail_keys.contains(&thumbnail_path) {
                        index.add_record_path_count(&thumbnails_root, &thumbnail_path, 1);
                    }
                }
            }
            for hash in queries::query_live_hashes(connection, chunk).await? {
                index.live_hashes.insert(hash.to_ascii_lowercase());
            }
        }

        for chunk in keys.internal_paths.chunks(REFERENCE_QUERY_BATCH_SIZE) {
            for (raw_path, count) in
                queries::query_path_counts(connection, "internal_path", chunk).await?
            {
                index.add_record_path_count(&attachments_root, &raw_path, count);
            }
        }
        for chunk in keys.thumbnail_paths.chunks(REFERENCE_QUERY_BATCH_SIZE) {
            for (raw_path, count) in
                queries::query_path_counts(connection, "thumbnail_path", chunk).await?
            {
                index.add_record_path_count(&thumbnails_root, &raw_path, count);
            }
        }
        Ok(index)
    }

    fn add_record_path_count(&mut self, root: &Path, raw_path: &str, count: usize) {
        // Only a genuinely empty value is a legacy empty path. Do not trim:
        // leading/trailing spaces are valid filename bytes.
        if raw_path.is_empty() || count == 0 {
            return;
        }
        let candidate = Path::new(raw_path.strip_prefix("file://").unwrap_or(raw_path));
        // A path that currently resolves outside this managed root is
        // provably unrelated to a candidate inside the root. Do not let such
        // legacy/out-of-scope records block otherwise safe GC work. If it
        // cannot be resolved, retain uncertainty below instead of treating it
        // as unreferenced.
        let canonical = std::fs::canonicalize(candidate).ok();
        if let Some(canonical) = canonical.as_ref() {
            if !canonical.starts_with(root) {
                return;
            }
        }
        match validate_reference_path_from_root(root, candidate) {
            Ok((path, _)) => {
                *self.path_references.entry(path).or_default() += count;
            }
            Err(_) => match canonical {
                Some(canonical) if canonical.starts_with(root) => {
                    // A lexical path outside the managed root can still be a
                    // symlink alias into it. It cannot be safely classified as
                    // unrelated because the physical target is managed.
                    self.uncertain_paths.insert(canonical);
                }
                Some(_) => {}
                None if std::fs::symlink_metadata(candidate)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink()) =>
                {
                    // A broken root-external symlink has no physical target to
                    // key yet. Treat the entire corresponding managed root as
                    // uncertain instead of silently classifying it as unrelated.
                    self.uncertain_roots.insert(root.to_path_buf());
                }
                _ if path_may_be_inside_managed_root(root, raw_path) => {
                    self.uncertain_roots.insert(root.to_path_buf());
                }
                _ => {}
            },
        }
    }

    #[cfg(test)]
    pub(super) fn loaded_key_counts(&self) -> (usize, usize, usize) {
        (
            self.path_references.len(),
            self.indexed_hashes.len(),
            self.live_hashes.len(),
        )
    }

    pub(super) fn remove_record(
        &mut self,
        roots: &ManagedAttachmentRoots,
        record: &IndexedAttachment,
    ) {
        let attachments_root = canonical_managed_root(&roots.attachments);
        let thumbnails_root = canonical_managed_root(&roots.thumbnails);
        self.remove_record_path(attachments_root.ok().as_deref(), &record.internal_path);
        if let Some(path) = record.thumbnail_path.as_deref() {
            self.remove_record_path(thumbnails_root.ok().as_deref(), path);
        }
        self.indexed_hashes
            .remove(&record.hash.to_ascii_lowercase());
        self.hash_paths.remove(&record.hash.to_ascii_lowercase());
    }

    fn remove_record_path(&mut self, root: Option<&Path>, raw_path: &str) {
        if raw_path.is_empty() {
            return;
        }
        let Some(root) = root else { return };
        let candidate = Path::new(raw_path.strip_prefix("file://").unwrap_or(raw_path));
        if let Ok((path, _)) = validate_reference_path_from_root(root, candidate) {
            if let Some(count) = self.path_references.get_mut(&path) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.path_references.remove(&path);
                }
            }
        }
    }

    pub(super) fn has_indexed_hash(&self, hash: &str) -> bool {
        self.indexed_hashes.contains(&hash.to_ascii_lowercase())
    }

    pub(super) fn has_live_reference(&self, hash: &str) -> bool {
        self.live_hashes.contains(&hash.to_ascii_lowercase())
    }

    pub(super) fn has_live_reference_at_path(&self, hash: &str, root: &Path, path: &Path) -> bool {
        if !self.has_live_reference(hash) {
            return false;
        }
        let Ok(canonical_root) = canonical_managed_root(root) else {
            return false;
        };
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !target.starts_with(&canonical_root) {
            return false;
        }
        self.hash_paths
            .get(&hash.to_ascii_lowercase())
            .is_some_and(|paths| paths.contains(&target))
    }

    pub(super) fn path_reference_state(
        &self,
        root: &Path,
        path: &Path,
    ) -> Result<PathReferenceState, String> {
        let raw_path = path
            .to_str()
            .ok_or_else(|| "附件路径不是有效 UTF-8".to_string())?;
        let canonical_root = canonical_managed_root(root)?;
        let (target, _) = validate_reference_path(&canonical_root, raw_path)?;
        if self
            .path_references
            .get(&target)
            .copied()
            .unwrap_or_default()
            > 0
        {
            return Ok(PathReferenceState::Referenced);
        }
        if self.uncertain_paths.contains(&target) {
            return Ok(PathReferenceState::Uncertain);
        }
        if self.uncertain_roots.contains(&canonical_root) {
            return Ok(PathReferenceState::Uncertain);
        }
        Ok(PathReferenceState::Unreferenced)
    }

    fn add_hash_target(&mut self, root: &Path, hash: &str, raw_path: &str) {
        let clean_path = raw_path.strip_prefix("file://").unwrap_or(raw_path);
        let candidate = Path::new(clean_path);
        let Ok(canonical_root) = canonical_managed_root(root) else {
            return;
        };
        let Ok(target) = std::fs::canonicalize(candidate) else {
            return;
        };
        if !target.starts_with(&canonical_root) {
            return;
        }
        self.hash_paths
            .entry(hash.to_ascii_lowercase())
            .or_default()
            .insert(target);
    }
}

#[derive(Debug, Default)]
struct ReferenceKeys {
    internal_paths: Vec<String>,
    thumbnail_paths: Vec<String>,
    hashes: Vec<String>,
}

impl ReferenceKeys {
    fn add_internal_path(&mut self, raw_path: &str) {
        add_raw_path_key(&mut self.internal_paths, raw_path);
    }

    fn add_thumbnail_path(&mut self, raw_path: &str) {
        add_raw_path_key(&mut self.thumbnail_paths, raw_path);
    }
}

fn add_raw_path_key(keys: &mut Vec<String>, raw_path: &str) {
    if raw_path.is_empty() {
        return;
    }
    if !keys.iter().any(|key| key == raw_path) {
        keys.push(raw_path.to_string());
    }
    let alternate = if let Some(path) = raw_path.strip_prefix("file://") {
        path.to_string()
    } else {
        format!("file://{raw_path}")
    };
    if !keys.iter().any(|key| key == &alternate) {
        keys.push(alternate);
    }
}
