-- 为 active_generations 记录原生 helper 的真实 session generation。
-- 允许 NULL 仅用于升级前的历史活动记录；新 Android handshake 会以
-- 完整身份和当前请求 epoch 原子补齐，恢复流程不会猜测 NULL 的值。
ALTER TABLE active_generations ADD COLUMN helper_generation BIGINT;
