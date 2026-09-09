-- Migration 0008: 将 fork 旧 schema 无损升级为 Wire 1.4 复合身份。
--
-- 现有 0001-0007 已发布且 checksum 不可改变。本迁移在 SQLx 事务内重建
-- owner-aware 表。所有 active_generations 行都必须原样保留；无法映射的
-- owner identity 或复制数量变化会触发 CHECK 失败并整体回滚。不要用
-- upstream 的 fresh-install 0100 baseline 替代本文件。

CREATE TABLE _wire14_migration_guard (
    valid INTEGER NOT NULL CHECK (valid = 1)
);

-- active_generations 只记录可恢复的流式运行时状态，不承载消息正文。
-- 旧版在崩溃/强杀后可能留下缺失话题、缺失消息或已删除消息的注册项；
-- 这些行仍然拥有完整的 Wire identity，必须作为恢复日志原样迁移，不能
-- 用关系清理静默丢弃。只有 identity 本身无法映射到新复合主键时才失败。

-- 迁移前先证明旧表的单列 topic 身份仍能无损映射到完整 owner identity。
INSERT INTO _wire14_migration_guard(valid)
SELECT CASE WHEN
    NOT EXISTS (
        SELECT 1 FROM topics
        WHERE owner_type IS NULL OR owner_type NOT IN ('agent', 'group')
           OR owner_id IS NULL OR owner_id = ''
           OR topic_id IS NULL OR topic_id = ''
    )
    AND NOT EXISTS (
        SELECT 1 FROM messages m
        LEFT JOIN topics t ON t.topic_id = m.topic_id
        WHERE t.topic_id IS NULL
    )
    AND NOT EXISTS (
        SELECT 1 FROM render_cache r
        LEFT JOIN messages m
          ON m.topic_id = r.topic_id AND m.msg_id = r.msg_id
        WHERE m.msg_id IS NULL
    )
    AND NOT EXISTS (
        SELECT 1 FROM message_attachments ma
        LEFT JOIN messages m
          ON m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
        WHERE m.msg_id IS NULL
    )
    AND NOT EXISTS (
        SELECT 1 FROM active_generations
        WHERE owner_type IS NULL OR owner_type NOT IN ('agent', 'group')
           OR owner_id IS NULL OR owner_id = ''
           OR topic_id IS NULL OR topic_id = ''
           OR msg_id IS NULL OR msg_id = ''
    )
    AND NOT EXISTS (
        SELECT 1 FROM active_generations a
        JOIN topics t ON t.topic_id = a.topic_id
        WHERE t.owner_type <> a.owner_type OR t.owner_id <> a.owner_id
    )
    AND (SELECT COUNT(*) FROM messages_fts) = (
        SELECT COUNT(*)
        FROM messages_fts f
        JOIN topics t ON t.topic_id = f.topic_id
    )
THEN 1 ELSE 0 END;

-- 保留现有 FTS 中已经解压/预处理的文本，避免从压缩 BLOB 猜测内容。
CREATE TABLE _wire14_fts_backup AS
SELECT t.owner_type, t.owner_id, f.topic_id, f.msg_id, f.content
FROM messages_fts f
JOIN topics t ON t.topic_id = f.topic_id;

DROP TRIGGER IF EXISTS after_messages_physical_delete;
DROP TRIGGER IF EXISTS after_messages_logical_delete;
DROP TABLE messages_fts;

ALTER TABLE render_cache RENAME TO render_cache_legacy_wire14;
ALTER TABLE message_attachments RENAME TO message_attachments_legacy_wire14;
ALTER TABLE messages RENAME TO messages_legacy_wire14;
ALTER TABLE topics RENAME TO topics_legacy_wire14;
ALTER TABLE active_generations RENAME TO active_generations_legacy_wire14;

CREATE TABLE topics (
    owner_type TEXT NOT NULL CHECK (owner_type IN ('agent', 'group')),
    owner_id TEXT NOT NULL CHECK (owner_id <> ''),
    topic_id TEXT NOT NULL CHECK (topic_id <> ''),
    title TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    last_message_updated_at BIGINT NOT NULL DEFAULT 0,
    locked INTEGER NOT NULL DEFAULT 1,
    unread INTEGER NOT NULL DEFAULT 0,
    unread_count INTEGER NOT NULL DEFAULT 0,
    msg_count INTEGER NOT NULL DEFAULT 0,
    config_hash TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL DEFAULT '',
    deleted_at BIGINT,
    PRIMARY KEY (owner_type, owner_id, topic_id)
);

CREATE TABLE messages (
    owner_type TEXT NOT NULL CHECK (owner_type IN ('agent', 'group')),
    owner_id TEXT NOT NULL CHECK (owner_id <> ''),
    topic_id TEXT NOT NULL CHECK (topic_id <> ''),
    msg_id TEXT NOT NULL CHECK (msg_id <> ''),
    role TEXT NOT NULL,
    name TEXT,
    agent_id TEXT,
    content TEXT NOT NULL,
    timestamp BIGINT NOT NULL,
    is_group_message INTEGER NOT NULL DEFAULT 0,
    group_id TEXT,
    finish_reason TEXT,
    content_hash TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    deleted_at BIGINT,
    PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
);

CREATE TABLE render_cache (
    owner_type TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    topic_id TEXT NOT NULL,
    msg_id TEXT NOT NULL,
    render_content BLOB,
    updated_at BIGINT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    renderer_schema_version INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (owner_type, owner_id, topic_id, msg_id),
    FOREIGN KEY (owner_type, owner_id, topic_id, msg_id)
        REFERENCES messages(owner_type, owner_id, topic_id, msg_id) ON DELETE CASCADE
);

CREATE TABLE message_attachments (
    owner_type TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    topic_id TEXT NOT NULL,
    msg_id TEXT NOT NULL,
    hash TEXT NOT NULL,
    attachment_order INTEGER NOT NULL,
    display_name TEXT NOT NULL,
    src TEXT,
    status TEXT,
    created_at BIGINT NOT NULL,
    deleted_at BIGINT,
    PRIMARY KEY (owner_type, owner_id, topic_id, msg_id, attachment_order),
    FOREIGN KEY (owner_type, owner_id, topic_id, msg_id)
        REFERENCES messages(owner_type, owner_id, topic_id, msg_id) ON DELETE CASCADE
);

CREATE TABLE active_generations (
    owner_type TEXT NOT NULL CHECK (owner_type IN ('agent', 'group')),
    owner_id TEXT NOT NULL CHECK (owner_id <> ''),
    topic_id TEXT NOT NULL CHECK (topic_id <> ''),
    msg_id TEXT NOT NULL CHECK (msg_id <> ''),
    created_at BIGINT NOT NULL,
    PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
);

INSERT INTO topics (
    owner_type, owner_id, topic_id, title, created_at, updated_at,
    last_message_updated_at, locked, unread, unread_count, msg_count,
    config_hash, content_hash, deleted_at
)
SELECT
    t.owner_type, t.owner_id, t.topic_id, t.title, t.created_at, t.updated_at,
    COALESCE((
        SELECT MAX(m.updated_at)
        FROM messages_legacy_wire14 m
        WHERE m.topic_id = t.topic_id AND m.deleted_at IS NULL
    ), 0),
    t.locked, t.unread, t.unread_count, t.msg_count,
    t.config_hash, t.content_hash, t.deleted_at
FROM topics_legacy_wire14 t;

INSERT INTO messages (
    owner_type, owner_id, topic_id, msg_id, role, name, agent_id, content,
    timestamp, is_group_message, group_id, finish_reason, content_hash,
    created_at, updated_at, deleted_at
)
SELECT
    t.owner_type, t.owner_id, m.topic_id, m.msg_id, m.role, m.name,
    m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id,
    m.finish_reason, m.content_hash, m.created_at, m.updated_at, m.deleted_at
FROM messages_legacy_wire14 m
JOIN topics_legacy_wire14 t ON t.topic_id = m.topic_id;

INSERT INTO render_cache (
    owner_type, owner_id, topic_id, msg_id, render_content, updated_at,
    content_hash, renderer_schema_version
)
SELECT
    t.owner_type, t.owner_id, r.topic_id, r.msg_id, r.render_content,
    r.updated_at, r.content_hash, r.renderer_schema_version
FROM render_cache_legacy_wire14 r
JOIN topics_legacy_wire14 t ON t.topic_id = r.topic_id;

INSERT INTO message_attachments (
    owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
    display_name, src, status, created_at, deleted_at
)
SELECT
    t.owner_type, t.owner_id, ma.topic_id, ma.msg_id, ma.hash,
    ma.attachment_order, ma.display_name, ma.src, ma.status,
    ma.created_at, ma.deleted_at
FROM message_attachments_legacy_wire14 ma
JOIN topics_legacy_wire14 t ON t.topic_id = ma.topic_id;

INSERT INTO active_generations (
    owner_type, owner_id, topic_id, msg_id, created_at
)
SELECT owner_type, owner_id, topic_id, msg_id, created_at
FROM active_generations_legacy_wire14;

-- 复制后再核对各业务表数量；失败会回滚所有 rename/create/copy。
INSERT INTO _wire14_migration_guard(valid)
SELECT CASE WHEN
    (SELECT COUNT(*) FROM topics) = (SELECT COUNT(*) FROM topics_legacy_wire14)
    AND (SELECT COUNT(*) FROM messages) = (SELECT COUNT(*) FROM messages_legacy_wire14)
    AND (SELECT COUNT(*) FROM render_cache) = (SELECT COUNT(*) FROM render_cache_legacy_wire14)
    AND (SELECT COUNT(*) FROM message_attachments) = (SELECT COUNT(*) FROM message_attachments_legacy_wire14)
    AND (SELECT COUNT(*) FROM active_generations) = (SELECT COUNT(*) FROM active_generations_legacy_wire14)
THEN 1 ELSE 0 END;

DROP TABLE render_cache_legacy_wire14;
DROP TABLE message_attachments_legacy_wire14;
DROP TABLE messages_legacy_wire14;
DROP TABLE topics_legacy_wire14;
DROP TABLE active_generations_legacy_wire14;

CREATE INDEX idx_topics_owner
    ON topics(owner_type, owner_id, created_at DESC);
CREATE INDEX idx_messages_topic_time
    ON messages(owner_type, owner_id, topic_id, timestamp DESC, msg_id DESC);
CREATE INDEX idx_messages_updated_at ON messages(updated_at);
CREATE INDEX idx_messages_agent_id ON messages(agent_id);
CREATE INDEX idx_messages_role ON messages(role);
CREATE INDEX idx_message_attachments_hash ON message_attachments(hash);
CREATE INDEX idx_message_attachments_msg
    ON message_attachments(owner_type, owner_id, topic_id, msg_id);
CREATE INDEX idx_render_cache_msg
    ON render_cache(owner_type, owner_id, topic_id, msg_id);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    msg_id UNINDEXED,
    topic_id UNINDEXED,
    content,
    owner_type UNINDEXED,
    owner_id UNINDEXED,
    tokenize = 'trigram'
);

INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
SELECT msg_id, topic_id, content, owner_type, owner_id
FROM _wire14_fts_backup;

INSERT INTO _wire14_migration_guard(valid)
SELECT CASE WHEN
    (SELECT COUNT(*) FROM messages_fts) = (SELECT COUNT(*) FROM _wire14_fts_backup)
THEN 1 ELSE 0 END;

CREATE TRIGGER after_messages_physical_delete
AFTER DELETE ON messages
BEGIN
    DELETE FROM messages_fts
    WHERE owner_type = old.owner_type AND owner_id = old.owner_id
      AND topic_id = old.topic_id AND msg_id = old.msg_id;
END;

CREATE TRIGGER after_messages_logical_delete
AFTER UPDATE OF deleted_at ON messages
WHEN new.deleted_at IS NOT NULL
BEGIN
    DELETE FROM messages_fts
    WHERE owner_type = new.owner_type AND owner_id = new.owner_id
      AND topic_id = new.topic_id AND msg_id = new.msg_id;
END;

DROP TABLE _wire14_fts_backup;
DROP TABLE _wire14_migration_guard;
