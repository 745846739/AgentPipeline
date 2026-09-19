-- 决策 231：模型请求落一张请求级表。
--
-- 契约只有一句：**一次请求一行，请求开始就写、`finished_at IS NULL` 即「在飞」，结束补全**。
-- 它同时解两件事：
--   * **归位**——`kanban_node_commands` 有 `run_id`（节点命令挂在 run 上）、值班长自己的命令有
--     `session_id`，而模型请求那一层此前**完全没有身份**（全库没有请求级表、`kanban_node_runs`
--     也没有请求级字段）。于是值班长会把一条活栈记在错的 run 名下，且没有任何断言拦得住
--     （决策 230 的「证据归错 run 与没有证据同判失败」）。
--   * **量速**——`bytes_received` + `last_byte_at` 才分得开「流快但 prompt 本身大」与
--     「流被压到极慢」（实测里它明说自己量不出 bytes/s）。
--
-- 归属三态，每个字段都可为 NULL，因为它们各自对应一条真实的调用路径：
--   * 流水线节点 / 子代理 / 伪阶段 / 项目级 run → `run_id` 指向 `kanban_node_runs`；
--   * 值班长（决策 182⑨：没有 run 行）→ `session_id` 指向 `kanban_foreman_sessions`；
--   * 项目分析那一次调用两样都没有（`RunContext { run_id: 0, task_id: "" }`）——它仍旧落账，
--     只是没有所属，故两个归属列都为空。
-- 代码里 `run_id = 0` 是「没有 run 行」的哨兵值，写入前归一成 NULL（0 会撞外键）。
--
-- `bytes_received` / `last_byte_at` **可空**，且 NULL 与 0 是两件事：NULL = 这次调用没有拿到
-- 收场读数（流半途断了 / 被丢掉），0 = 真的一个字节都没收到。决策 226③ 刚把「用 0 冒充读数」
-- 从 run 记账里挖掉，这张表不能原地再造一个。
-- 四个 token 列**同一条道理**：用量是随流的最后一个 usage 事件到的，流半途断掉时它根本没到过，
-- 那时写 0 会把「没有读数」说成「一个 token 都没烧」——而 2026-09-19 那次实测里，
-- 「一千万 prompt token 却记 0」正是把值班长带偏四轮的那个假读数。
CREATE TABLE IF NOT EXISTS kanban_model_requests (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id             INTEGER,
    session_id         TEXT,
    task_id            TEXT,
    agent_type         TEXT NOT NULL,
    stage              TEXT NOT NULL,
    node               TEXT NOT NULL,
    attempt            INTEGER NOT NULL DEFAULT 1,
    -- 序号：**同一次 run（或同一个班次）内**的第几次请求，1 起。归位时用它说「第几次调用」。
    seq                INTEGER NOT NULL,
    -- running 只出现在 `finished_at IS NULL` 的那些行上；其余四个是终态（决策 231）。
    status             TEXT NOT NULL,
    prompt_tokens      INTEGER,
    completion_tokens  INTEGER,
    cache_read_tokens  INTEGER,
    cache_write_tokens INTEGER,
    bytes_received     INTEGER,
    last_byte_at       TEXT,
    error              TEXT,
    started_at         TEXT NOT NULL,
    finished_at        TEXT,
    CHECK (status IN ('running', 'ok', 'error', 'cancelled', 'timeout')),
    FOREIGN KEY (run_id) REFERENCES kanban_node_runs(id),
    FOREIGN KEY (session_id) REFERENCES kanban_foreman_sessions(id)
);
-- 按 run 读（诊断包逐 run 列出它发过几次请求）。
CREATE INDEX IF NOT EXISTS idx_model_requests_run ON kanban_model_requests(run_id, seq);
-- 「现在在飞什么」：`finished_at IS NULL` 是这张表的现状读数，故它是索引的键。
CREATE INDEX IF NOT EXISTS idx_model_requests_inflight ON kanban_model_requests(finished_at, id);
CREATE INDEX IF NOT EXISTS idx_model_requests_session ON kanban_model_requests(session_id, id);
