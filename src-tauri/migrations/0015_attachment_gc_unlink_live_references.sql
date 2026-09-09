-- Migration 0015: persist bounded live-reference witnesses for unlink debts.
--
-- A production attachment write may retain an unlink debt when its stored path
-- is an external or ancestor symlink alias. The alias is intentionally not
-- trusted by the write-side path validator, so keep a per-debt/hash witness
-- for the later bounded transaction check instead of scanning all attachment
-- paths by basename.
CREATE TABLE IF NOT EXISTS attachment_gc_unlink_live_references (
    root_kind TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    hash TEXT NOT NULL,
    PRIMARY KEY (root_kind, relative_path, hash)
);

CREATE INDEX IF NOT EXISTS idx_attachment_gc_unlink_live_references_hash
    ON attachment_gc_unlink_live_references(hash COLLATE NOCASE);
