-- 恢复终结前记录已占用文件的清理义务；提交后 unlink 失败可由启动维护继续处理。
CREATE TABLE IF NOT EXISTS recovery_cleanup_outbox (
    claimed_path TEXT NOT NULL PRIMARY KEY,
    owner_type TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    topic_id TEXT NOT NULL,
    msg_id TEXT NOT NULL,
    helper_generation BIGINT NOT NULL CHECK (helper_generation > 0),
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_recovery_cleanup_outbox_created
    ON recovery_cleanup_outbox(created_at, claimed_path);
