-- 决策 423/424（票 talk-cli-engine 02）：对讲台会话的执行引擎 + CLI 会话续接锚。
--
-- 形态裁决：与 kind（迁移 0030）同一条——同一套表加列，全部复用；不另起一套表。
--
-- engine：`api`（进程内 LLM，现状语义）/ `cli`（本机 CLI agent 整段执行器）。
-- 存量行全部落 `api`，行为不变。CHECK 与 kind / role 同一姿态：非法值在写入点被挡住，
-- 读取侧不防坏数据。**没有改 engine 的写路径**——决策 424④「建会话时选定、会话内
-- 锁定」的锁不是一段校验代码，是形状（CLI 会话绑死引擎，--resume 跨不了引擎）。
--
-- cli_session_id：claude `--resume` 的续接锚（决策 424③），首轮成功后回填。
-- NULL = 还没跑过 CLI 轮，或引擎是 api（「没有」不落空串，与 briefing_json /
-- traces_json 同一条「没有 ≠ 空集」口径）。
ALTER TABLE kanban_foreman_sessions ADD COLUMN engine TEXT NOT NULL DEFAULT 'api'
    CHECK (engine IN ('api', 'cli'));
ALTER TABLE kanban_foreman_sessions ADD COLUMN cli_session_id TEXT;
