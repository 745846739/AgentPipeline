-- 决策 83：sync-check backtrack 时把 dev-plan.md / test-scenarios.md 标记为过期
-- （文件保留供回溯，下次执行覆盖写入时由 upsert 清除）。
ALTER TABLE kanban_stage_outputs ADD COLUMN stale INTEGER NOT NULL DEFAULT 0;
