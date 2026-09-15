-- 会话续接（决策 180，票 13）：pending → resume 时可选择续接上一 attempt 的对话。
--
-- ## ① `kanban_node_cursors.resumed_from_pending`——一次性「刚被 resume」标记
--
-- 续接**只作用于 pending → resume 边界**，不作用于 `agent_retry_max` 的干净重试
-- （决策 33 的语义保持不变）。这两条路都必须分开，而 `attempt > 1` 分不开——重试也会让
-- attempt 变大。故由 resume 的那一侧（`clear_cursor_pending`，它只在该游标确实是 pending 时
-- 才改动行）置位，由执行节点在进入 attempt 循环前**取走并清零**（一次性）。
--
-- ## ② `kanban_node_runs.continued_from_run_id`——续接指向被续接的历史 run
--
-- 续接会把上一轮的对话重新发一遍，于是历史 run 报过的输入 token 在新 run 里**再报一次**。
-- 任务画像对 run 行是盲求和的（`metrics::total_tokens`），不排除被续接的历史就是双算。
-- 指针落在这里而不是反向标记：一个 run 最多续接一条历史，但一条历史可能被多轮续接——
-- 从「续接者」指回去，排除集就是简单的「被任何 run 指到的那些」。
ALTER TABLE kanban_node_cursors ADD COLUMN resumed_from_pending INTEGER NOT NULL DEFAULT 0;
ALTER TABLE kanban_node_runs ADD COLUMN continued_from_run_id INTEGER;

-- ## ③ `stage_configs.resume_continuation`——阶段级开关（节点级覆盖走 node_overrides_json）
--
-- 默认 NULL = 关。照 `idle_timeout_sec` 的既有分层（全局默认 → 阶段级 → 节点级覆盖）。
ALTER TABLE stage_configs ADD COLUMN resume_continuation INTEGER;
