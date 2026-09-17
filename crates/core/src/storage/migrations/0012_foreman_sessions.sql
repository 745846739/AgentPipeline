-- 决策 204：对讲台的会话（班次）——一条长会话拆成一排可管理的东西；命令日志同时拿到会话归属。
--
-- 三件事一张作业（决策 204④），因为它们回答的是同一个问题「这句话 / 这条命令属于哪一次值班」：
--   1. 新表 kanban_foreman_sessions；
--   2. kanban_foreman_messages 加 session_id，既有行回填进第一个会话；
--   3. kanban_node_commands 加 session_id，并把 task_id 改为可空。
--
-- 第 3 件事的理由：值班长没有 task_id（`pipeline/foreman.rs` 给 `ToolCallContext` 的是空串），
-- 而空串会被外键校验拒掉——迁移 0004:11 的原话「不采用哨兵值：SQLite 的外键校验会让
-- task_id = '' 直接失败」。手法照 0004 给 runs / conversations 做过的同一套：
-- 建新表 → 拷旧行 → 删旧表 → 改名（SQLite 不支持 ALTER COLUMN 去 NOT NULL / 加 CHECK）。
--
-- 口径（决策 204②⑦）：
--   * 会话隔离的是**上下文**（喂给模型的 transcript 与页头 token 合计按会话过滤），
--     不隔离权限——`read_task` / `read_conversation` 照旧全局，态势快照照旧全局；
--   * 归档 = 置 archived_at，**不物理删除**，且**不保护消息**：消息照旧按
--     `conversation_retention_days` 的年龄清理。「从列表里收起来」不是永久保存。

CREATE TABLE IF NOT EXISTS kanban_foreman_sessions (
    id             TEXT PRIMARY KEY,
    title          TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    -- 最近一次说话的时间：会话列表按它倒序（决策 204⑦）。追加消息时同事务刷新。
    last_active_at TEXT NOT NULL,
    archived_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_foreman_sessions_recent
    ON kanban_foreman_sessions(archived_at, last_active_at DESC);

-- ── messages：加 session_id（可空外键），既有行回填进第一个会话 ──
--
-- 外键列只允许以 NULL 缺省值追加（SQLite 的 ADD COLUMN 限制），故这里先加列、再回填。
ALTER TABLE kanban_foreman_messages
    ADD COLUMN session_id TEXT REFERENCES kanban_foreman_sessions(id);

-- 第一个会话只在**库里真有话**时建立：没有消息就没有「第一次值班」这回事，
-- 新库应当从零开始，首个会话在第一次说话时由应用层创建（`say` 的懒创建）。
--
-- 标题规则与 `storage/foreman.rs::session_title_from` **逐字等价**（同一份可读性判据：
-- TRIM → 取前 24 个字符 → 截断时补省略号；空则中性标题）。SQLite 的 SUBSTR / LENGTH
-- 对 TEXT 计数的是**字符**而不是字节，故中文不会被切坏。SQL 里写不出「空白折叠」，
-- 所以那条规则两边都没有——标题里的换行在界面上按 HTML 既有规则渲染成一个空白。
WITH first_user AS (
    SELECT TRIM(content) AS text
    FROM kanban_foreman_messages WHERE role = 'user' ORDER BY id LIMIT 1
), agg AS (
    SELECT MIN(created_at) AS created_at, MAX(created_at) AS last_active_at, COUNT(*) AS n
    FROM kanban_foreman_messages
)
INSERT INTO kanban_foreman_sessions (id, title, created_at, last_active_at)
SELECT 'legacy-foreman-session',
       COALESCE(
           NULLIF(
               SUBSTR((SELECT text FROM first_user), 1, 24)
               || CASE WHEN LENGTH((SELECT text FROM first_user)) > 24 THEN '…' ELSE '' END,
               ''),
           '新班次'),
       (SELECT created_at FROM agg),
       (SELECT last_active_at FROM agg)
WHERE (SELECT n FROM agg) > 0;

UPDATE kanban_foreman_messages
SET session_id = 'legacy-foreman-session'
WHERE session_id IS NULL;

-- ── commands：加 session_id + task_id 改可空 ──
--
-- 归属约定与迁移 0004 的 runs / conversations 同一条：**恰好一个归属**，
-- 即流水线命令 task_id 非空、值班长命令 session_id 非空。空串不是合法归属
-- （存储层在 `record_start` 里把空串归一成 NULL，哨兵值无处可藏）。
CREATE TABLE commands_new (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id        TEXT,
    session_id     TEXT,
    run_id         INTEGER,
    stage          TEXT NOT NULL,
    node           TEXT NOT NULL,
    source         TEXT NOT NULL,
    command        TEXT NOT NULL,
    cwd            TEXT NOT NULL,
    exit_code      INTEGER,
    stdout_path    TEXT,
    stdout_preview TEXT,
    stderr_preview TEXT,
    duration_ms    INTEGER,
    started_at     TEXT NOT NULL,
    finished_at    TEXT,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (session_id) REFERENCES kanban_foreman_sessions(id),
    FOREIGN KEY (run_id) REFERENCES kanban_node_runs(id),
    CHECK ((task_id IS NOT NULL) <> (session_id IS NOT NULL))
);
INSERT INTO commands_new (id, task_id, session_id, run_id, stage, node, source, command, cwd,
                          exit_code, stdout_path, stdout_preview, stderr_preview, duration_ms,
                          started_at, finished_at)
SELECT id, task_id, NULL, run_id, stage, node, source, command, cwd, exit_code, stdout_path,
       stdout_preview, stderr_preview, duration_ms, started_at, finished_at
FROM kanban_node_commands;

DROP TABLE kanban_node_commands;
ALTER TABLE commands_new RENAME TO kanban_node_commands;

-- 索引随旧表一并删除，这里原样重建 + 会话归属查询索引
CREATE INDEX idx_commands_task ON kanban_node_commands(task_id, stage, node);
CREATE INDEX idx_commands_session ON kanban_node_commands(session_id, id);
