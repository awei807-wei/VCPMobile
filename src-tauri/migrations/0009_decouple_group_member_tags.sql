-- Migration 0009: 将群成员标签从活动成员表拆出。
--
-- group_members 只表示当前活动成员；标签属于 group/agent 身份，成员暂时移除
-- 后仍需保留，以便再次加入时恢复。旧 member_tag 列保留用于旧库兼容，但新
-- 读写路径不再依赖它。

CREATE TABLE IF NOT EXISTS group_member_tags (
    group_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    member_tag TEXT NOT NULL CHECK (length(trim(member_tag)) > 0),
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (group_id, agent_id)
);

INSERT INTO group_member_tags (group_id, agent_id, member_tag, updated_at)
SELECT group_id, agent_id, member_tag, updated_at
FROM group_members
WHERE member_tag IS NOT NULL AND length(trim(member_tag)) > 0
ON CONFLICT(group_id, agent_id) DO NOTHING;

CREATE INDEX IF NOT EXISTS idx_group_member_tags_group
    ON group_member_tags(group_id, updated_at DESC);
