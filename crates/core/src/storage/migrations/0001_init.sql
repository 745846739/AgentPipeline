-- AgentPipeline 初始 schema（docs/implementation.md §11.5 + docs/operations.md §12.4）
--
-- 时间戳一律 TEXT（RFC3339），由应用层序列化（storage::ts / storage::parse_ts）。
-- 游标行永不物理删除（决策 113）：合并 / 回退 / 重试把旧行置 archived 后插入新行。

-- 任务主表。current_stage / current_node / validate_attempts 是「焦点游标」投影（决策 80），
-- 执行状态的唯一事实来源是 kanban_node_cursors。
CREATE TABLE IF NOT EXISTS kanban_tasks (
    id                  TEXT PRIMARY KEY,
    title               TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    project_id          TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'queued',  -- 决策 98：一律 queued/waiting 落库
    current_stage       TEXT NOT NULL,
    current_node        TEXT NOT NULL DEFAULT 'execute',
    validate_attempts   INTEGER NOT NULL DEFAULT 0,
    pending_reason_json TEXT,
    worktree_path       TEXT,
    branch_name         TEXT,
    total_tokens        INTEGER NOT NULL DEFAULT 0,
    total_calls         INTEGER NOT NULL DEFAULT 0,
    review_mode         TEXT NOT NULL DEFAULT 'agent',
    model_override      TEXT,
    archived_at         TEXT,
    stalled             INTEGER NOT NULL DEFAULT 0,
    executor_owner      TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON kanban_tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_project ON kanban_tasks(project_id, status);

-- 活跃游标表（决策 80）：执行状态的唯一事实来源。
CREATE TABLE IF NOT EXISTS kanban_node_cursors (
    cursor_id           TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL,
    branch              TEXT NOT NULL DEFAULT 'main',   -- main | develop-design | test-design
    stage               TEXT NOT NULL,
    node                TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'active', -- active | waiting_join | pending | archived
    validate_attempts   INTEGER NOT NULL DEFAULT 0,
    skipped_to_join     INTEGER NOT NULL DEFAULT 0,
    pending_reason_json TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id)
);
-- partial unique：只约束活跃行，archived 行让位于同分支新行（决策 113）
CREATE UNIQUE INDEX IF NOT EXISTS uq_node_cursors_active_branch
    ON kanban_node_cursors(task_id, branch) WHERE status != 'archived';
CREATE INDEX IF NOT EXISTS idx_node_cursors_task ON kanban_node_cursors(task_id, status);

-- 任务依赖表。
CREATE TABLE IF NOT EXISTS kanban_task_deps (
    task_id       TEXT NOT NULL,
    depends_on_id TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on_id),
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (depends_on_id) REFERENCES kanban_tasks(id)
);

-- 阶段产出（决策 30）：文件路径与路由元数据同表 upsert。
CREATE TABLE IF NOT EXISTS kanban_stage_outputs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id       TEXT NOT NULL,
    stage         TEXT NOT NULL,
    output_type   TEXT NOT NULL,
    file_path     TEXT NOT NULL,
    metadata_json TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    UNIQUE(task_id, stage, output_type)
);

-- 节点执行记录（决策 63 / 99 / 114）。
CREATE TABLE IF NOT EXISTS kanban_node_runs (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id                TEXT NOT NULL,
    cursor_id              TEXT NOT NULL,
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
    FOREIGN KEY (parent_run_id) REFERENCES kanban_node_runs(id)
);
CREATE INDEX IF NOT EXISTS idx_runs_task ON kanban_node_runs(task_id);
CREATE INDEX IF NOT EXISTS idx_runs_active ON kanban_node_runs(status);
CREATE INDEX IF NOT EXISTS idx_runs_stage ON kanban_node_runs(stage);

-- 项目管理。
CREATE TABLE IF NOT EXISTS kanban_projects (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    local_path      TEXT NOT NULL,
    default_branch  TEXT NOT NULL DEFAULT 'main',
    language        TEXT,
    test_framework  TEXT,
    lint_command    TEXT,
    agents_md_path  TEXT,
    created_at      TEXT NOT NULL
);

-- 项目静态分析（决策 130 ⑦：异步 202 + 轮询）。
CREATE TABLE IF NOT EXISTS kanban_project_analyses (
    analysis_id  TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'running',  -- running | done | failed
    result_json  TEXT,
    error        TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    FOREIGN KEY (project_id) REFERENCES kanban_projects(id)
);

-- provider（决策 111 / 112）：一行 = 一个 (vendor, model, context_window)，api_key 明文。
CREATE TABLE IF NOT EXISTS providers (
    id             TEXT PRIMARY KEY,
    vendor         TEXT NOT NULL,
    model          TEXT NOT NULL,
    context_window INTEGER NOT NULL,
    base_url       TEXT,
    api_key        TEXT,
    enabled        INTEGER NOT NULL DEFAULT 1,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

-- 阶段级 agent 配置（决策 22 / 46 / 66 / 111）。
CREATE TABLE IF NOT EXISTS stage_configs (
    stage              TEXT PRIMARY KEY,
    provider_id        TEXT,
    temperature        REAL,
    max_tokens         INTEGER,
    persona_path       TEXT,
    persona_append     TEXT,
    tools_json         TEXT,
    skills_json        TEXT,
    idle_timeout_sec   INTEGER,
    max_duration_sec   INTEGER,
    node_overrides_json TEXT,
    updated_at         TEXT NOT NULL,
    FOREIGN KEY (provider_id) REFERENCES providers(id)
);

-- 流转记录（§12.4.2）。
CREATE TABLE IF NOT EXISTS kanban_transitions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id    TEXT NOT NULL,
    branch     TEXT NOT NULL DEFAULT 'main',
    from_stage TEXT,
    from_node  TEXT,
    to_stage   TEXT NOT NULL,
    to_node    TEXT NOT NULL,
    trigger    TEXT NOT NULL,
    reason     TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id)
);
CREATE INDEX IF NOT EXISTS idx_transitions_task ON kanban_transitions(task_id);

-- 节点会话（§12.4.3）。1:1 只对调 LLM 的 run 成立（决策 99）。
CREATE TABLE IF NOT EXISTS kanban_node_conversations (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
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
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (run_id) REFERENCES kanban_node_runs(id)
);
CREATE INDEX IF NOT EXISTS idx_conversations_task ON kanban_node_conversations(task_id);

-- 命令日志（§12.4.4）。
CREATE TABLE IF NOT EXISTS kanban_node_commands (
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
    FOREIGN KEY (run_id) REFERENCES kanban_node_runs(id)
);
CREATE INDEX IF NOT EXISTS idx_commands_task ON kanban_node_commands(task_id, stage, node);
