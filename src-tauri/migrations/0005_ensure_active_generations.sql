-- Migration 0005: 补建旧版数据库可能缺失的活跃生成注册表
--
-- 部分存量数据库在引入 sqlx 迁移追踪前已经存在 messages 表，旧版桥接会将
-- Migration 0001 记为已执行，但这些数据库不一定包含后来纳入初始快照的表。
-- 使用独立的幂等迁移修复，避免修改已发布 Migration 0001 的 checksum。
CREATE TABLE IF NOT EXISTS active_generations (
    msg_id TEXT PRIMARY KEY,
    topic_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    owner_type TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
