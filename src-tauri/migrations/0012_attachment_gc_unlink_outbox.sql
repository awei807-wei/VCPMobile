-- Migration 0012: 持久化附件索引删除后的物理 unlink 债务。
--
-- CAS 索引与 outbox 必须在同一个 SQL 事务中写入。物理文件操作在提交后
-- 执行；失败时保留此表中的债务，后续维护会重新校验 root、相对路径和
-- 当前索引引用后再重试。
CREATE TABLE IF NOT EXISTS attachment_gc_unlink_outbox (
    root_kind TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (root_kind, relative_path)
);

CREATE INDEX IF NOT EXISTS idx_attachment_gc_unlink_outbox_created
    ON attachment_gc_unlink_outbox(created_at, root_kind, relative_path);
