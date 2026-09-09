-- Migration 0013: indexes for bounded attachment GC reference probes.
--
-- GC only asks about the current bounded candidate keys.  These indexes keep
-- those exact-key probes from scanning an unrelated attachments/relations
-- table as the database grows.
CREATE INDEX IF NOT EXISTS idx_attachments_internal_path
    ON attachments(internal_path);

CREATE INDEX IF NOT EXISTS idx_attachments_thumbnail_path
    ON attachments(thumbnail_path);

CREATE INDEX IF NOT EXISTS idx_attachments_hash_nocase
    ON attachments(hash COLLATE NOCASE);

CREATE INDEX IF NOT EXISTS idx_message_attachments_hash_nocase
    ON message_attachments(hash COLLATE NOCASE);
