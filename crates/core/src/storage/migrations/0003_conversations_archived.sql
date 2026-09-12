-- 决策 113 的同构姿态（§12.2 retry 会话归档）：会话行永不物理删除。
-- 重试时把旧会话标记 archived_at（保留供审计、不参与新执行），
-- 会话列表默认只返回未归档行；历史仍可按 include_archived 取回。
ALTER TABLE kanban_node_conversations ADD COLUMN archived_at TEXT;
