-- 票 10 / 决策 100：项目级伪阶段（project_analysis）的独立观测行。
--
-- 背景：task 内伪阶段（conflict_check / validator_cross_check）有父 run 与继承游标，
-- 落库时 task_id / cursor_id 都有值；而 project_analysis 没有任务、没有游标，
-- 无法用现有 NOT NULL 外键表达归属。
--
-- 口径（详见 docs/data-model.md §4.3）：
--   * kanban_node_runs / kanban_node_conversations 的 task_id / cursor_id 改为可空，
--     新增可空 project_id（外键 → kanban_projects）；
--   * 两者恰好一个归属：CHECK ((task_id IS NOT NULL) <> (project_id IS NOT NULL))，
--     即 task 行 task_id 非空、project_id 空；项目行 task_id / cursor_id 空、project_id 非空；
--   * 不采用哨兵值：SQLite 的外键校验会让 "task_id = ''" 直接失败（决策：外键不可伪造）。
--
-- SQLite 不支持 ALTER COLUMN 去 NOT NULL / 加 CHECK，只能「建新表 → 拷旧行 → 删旧表 → 改名」。
-- 子表先删（kanban_node_commands / kanban_node_conversations 都外键引用 runs），
-- 否则 DROP 父表会触发外键约束失败；runs 的自引用外键指向新表名后再改名。
-- 整段由 sqlx 包在单个事务里执行（迁移要么全成、要么全回滚）。
-- 注意：事务内 `PRAGMA foreign_keys` 是 no-op，故不靠关外键，而靠「先删子表再删父表」。

-- ── runs：task_id / cursor_id 可空 + project_id ──
CREATE TABLE runs_new (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id                TEXT,
    cursor_id              TEXT,
    project_id             TEXT,
    stage                  TEXT NOT NULL,
    node                   TEXT NOT NULL,
    attempt                INTEGER NOT NULL DEFAULT 1,
    agent_type             TEXT NOT NULL DEFAULT 'main',
    parent_run_id          INTEGER,
    status                 TEXT NOT NULL,
    prompt_tokens          INTEGER NOT NULL DEFAULT 0,
    completion_tokens      INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens      INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens     INTEGER NOT NULL DEFAULT 0,
    duration_ms            INTEGER NOT NULL DEFAULT 0,
    error                  TEXT,
    process_group_id       INTEGER,
    last_activity_at       TEXT,
    prompt_template_hash   TEXT,
    started_at             TEXT NOT NULL,
    finished_at            TEXT,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (cursor_id) REFERENCES kanban_node_cursors(cursor_id),
    FOREIGN KEY (project_id) REFERENCES kanban_projects(id),
    FOREIGN KEY (parent_run_id) REFERENCES runs_new(id),
    CHECK ((task_id IS NOT NULL) <> (project_id IS NOT NULL))
);
INSERT INTO runs_new (id, task_id, cursor_id, project_id, stage, node, attempt, agent_type,
                      parent_run_id, status, prompt_tokens, completion_tokens, cache_read_tokens,
                      cache_write_tokens, duration_ms, error, process_group_id, last_activity_at,
                      prompt_template_hash, started_at, finished_at)
SELECT id, task_id, cursor_id, NULL, stage, node, attempt, agent_type, parent_run_id, status,
       prompt_tokens, completion_tokens, cache_read_tokens, cache_write_tokens, duration_ms,
       error, process_group_id, last_activity_at, prompt_template_hash, started_at, finished_at
FROM kanban_node_runs;

-- ── conversations：task_id 可空 + project_id ──
CREATE TABLE conversations_new (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT,
    project_id        TEXT,
    run_id            INTEGER NOT NULL,
    stage             TEXT NOT NULL,
    node              TEXT NOT NULL,
    attempt           INTEGER NOT NULL,
    agent_type        TEXT NOT NULL DEFAULT 'main',
    parent_run_id     INTEGER,
    messages_json     TEXT NOT NULL,
    metadata_json     TEXT,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL,
    archived_at       TEXT,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (project_id) REFERENCES kanban_projects(id),
    FOREIGN KEY (run_id) REFERENCES runs_new(id),
    CHECK ((task_id IS NOT NULL) <> (project_id IS NOT NULL))
);
INSERT INTO conversations_new (id, task_id, project_id, run_id, stage, node, attempt, agent_type,
                               parent_run_id, messages_json, metadata_json, prompt_tokens,
                               completion_tokens, created_at, archived_at)
SELECT id, task_id, NULL, run_id, stage, node, attempt, agent_type, parent_run_id, messages_json,
       metadata_json, prompt_tokens, completion_tokens, created_at, archived_at
FROM kanban_node_conversations;

-- ── commands：run 行被重建，外键目标需指向新表（列不变） ──
CREATE TABLE commands_new (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id        TEXT NOT NULL,
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
    FOREIGN KEY (run_id) REFERENCES runs_new(id)
);
INSERT INTO commands_new (id, task_id, run_id, stage, node, source, command, cwd, exit_code,
                          stdout_path, stdout_preview, stderr_preview, duration_ms, started_at,
                          finished_at)
SELECT id, task_id, run_id, stage, node, source, command, cwd, exit_code, stdout_path,
       stdout_preview, stderr_preview, duration_ms, started_at, finished_at
FROM kanban_node_commands;

-- 子表先删，再删父表 runs
DROP TABLE kanban_node_commands;
DROP TABLE kanban_node_conversations;
DROP TABLE kanban_node_runs;

ALTER TABLE runs_new RENAME TO kanban_node_runs;
ALTER TABLE conversations_new RENAME TO kanban_node_conversations;
ALTER TABLE commands_new RENAME TO kanban_node_commands;

-- 索引随旧表一并删除，这里原样重建 + 项目归属查询索引
CREATE INDEX idx_runs_task ON kanban_node_runs(task_id);
CREATE INDEX idx_runs_active ON kanban_node_runs(status);
CREATE INDEX idx_runs_stage ON kanban_node_runs(stage);
CREATE INDEX idx_runs_project ON kanban_node_runs(project_id);
CREATE INDEX idx_conversations_task ON kanban_node_conversations(task_id);
CREATE INDEX idx_conversations_project ON kanban_node_conversations(project_id);
CREATE INDEX idx_commands_task ON kanban_node_commands(task_id, stage, node);
