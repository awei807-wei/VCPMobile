-- Migration 0007: 为头像同步保留逻辑删除墓碑。
ALTER TABLE avatars ADD COLUMN deleted_at BIGINT;
