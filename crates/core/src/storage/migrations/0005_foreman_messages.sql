-- 决策 176 / 182：对讲台（与值班长对话）的会话记录。
--
-- 为什么单开一张表、而不复用 kanban_node_conversations：
-- 那张表的归属是「task_id / project_id 恰好二选一」（迁移 0004 的 CHECK），
-- 而值班长对话是**本机夜班级**的——**没有任务、也可以没有项目**（首启空 home 就是这种）。
-- 硬塞进那条 CHECK 只有两条路：伪造归属（撒谎的数据），或重开迁移放宽约束
-- （把非流水线的对话混进 metrics 的 stage_aggregation 口径）。两条都不取。
--
-- 口径边界（写在这里，避免后来者误判为疏漏）：本表的 token **不计入**
-- `GET /metrics` 的 total_tokens / total_calls——那两个字段的契约是
-- 「流水线执行 + 伪阶段，不含面向人的对话」（§12.4.1 / 决策 130②；票 05 已把
-- metrics.rs 的口径注释改正为与事实一致——项目分析的运行行今天是计入的）。
-- 对话自身的 token 由本表承载，在对讲台上按「本次会话 N tok」自报，
-- 成本仍然可见，只是不混口径。

CREATE TABLE IF NOT EXISTS kanban_foreman_messages (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    -- user（值班员说的）/ assistant（值班长回话）
    role              TEXT NOT NULL,
    content           TEXT NOT NULL,
    -- 值班长每次回话的真实读数；用户消息为 0
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    -- 该次回话注入的夜班态势快照（审计：值班长当时看到的是这份读数）。
    -- 用户消息与「无读数」时为 NULL。存下来是为了「谁说的、依据什么」可追溯——
    -- 这是主题六对 §1 原则 2 的延续（不能只有人说，没有状态）。
    briefing_json     TEXT,
    -- 该次回话调用过的只读工具痕迹（票 05）：`[{tool, args_summary, ok}]`。
    -- 与 briefing_json 同属审计：快照回答「依据哪份读数」，痕迹回答「它又自己翻了什么」。
    traces_json       TEXT,
    created_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_foreman_messages_id ON kanban_foreman_messages(id);
