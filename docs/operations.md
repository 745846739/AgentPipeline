# 并发隔离与运维设计

> 拆分自 agent-pipeline.md（原 §12）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

## 12. 并发隔离与运维设计

### 12.1 并发任务隔离（git worktree）

多个任务并行时，各自在独立的 git worktree 中工作，从物理上避免文件互相覆盖：

```
{project.local_path}/                    # 用户项目的本地仓库（唯一事实来源，决策 29）
├── .git/
~/.agentpipeline/worktrees/
├── {task-a-id}/                 # 任务 A 的工作区（分支 kanban/{task-a-id}）
├── {task-b-id}/                 # 任务 B 的工作区（分支 kanban/{task-b-id}）
└── {task-c-id}/
~/.agentpipeline/tasks/
├── {task-a-id}/                 # 任务 A 的设计文档目录（不进 git）
│   ├── design.md
│   ├── dev-plan.md
│   └── test-scenarios.md
```

> worktree 的 `.git` 文件指向 `{project.local_path}/.git`。系统不在 `~/.agentpipeline/` 下维护任何中心化仓库副本。

**生命周期：**

| 时机 | 操作 |
|---|---|
| init.execute | `git worktree add {worktree_path} -b kanban/{task_id} {base}`（`{base}` = `{base_ref}` 在 init 时的 HEAD，有 remote 时先 fetch） |
| 各阶段执行 | 在 worktree 内读写代码 |
| merge 阶段 B | git2 内存合入（ff 优先，否则双亲 merge commit），结果直接写回 `{default_branch}`（决策 97，2026-09-12 git2 重写，不再建临时 worktree） |
| merge 成功 / done | `git worktree remove {worktree_path}`，删除分支 |
| 任务取消 | 同上，强制删除（`--force`） |
| 任务重试 | 复用已有 worktree，不重建 |

**优势：**
- 每个任务有独立的文件系统视图，不存在写冲突
- 共享同一个 `.git` 对象库，磁盘占用远小于多次 clone
- 分支天然隔离，merge 通过 rebase 到 `default_branch` 统一收敛

**冲突收敛：** 所有并行任务最终都要 rebase 到 `default_branch`。冲突在 merge 阶段暴露，无法自动解决则打回 develop（见 §6 merge），由 develop agent 基于最新 `default_branch` 修改代码。语义层面的重复（不同文件实现同类功能）由 architect 阶段的两层检测尽量前置发现（决策 60）。

### 12.2 成本控制

**单次调用上限：** 每次 LLM 调用的 `max_tokens` 取自阶段配置 `StageAgentConfig.provider.max_tokens`（存 DB、界面可改，决策 22 / 46），防止单次调用失控。全局不再有 `node_max_tokens` 配置项（决策 56），阶段未配置时由 provider 默认值兜底。

**成本记录：** 每次 LLM 调用写入 `kanban_node_runs`，记录 `prompt_tokens` 和 `completion_tokens`，按任务/阶段/节点聚合，供用户在前端查看成本统计。

### 12.3 任务取消与终止

**取消流程：**

```
POST /tasks/{id}/cancel
  │
  ├─ 1. status → cancelled
  ├─ 2. executor 停止该任务的执行循环（设置 cancelled flag）
  ├─ 3. 清理 worktree（git worktree remove --force）
  ├─ 4. 删除分支（git branch -D kanban/{task_id}）
  ├─ 5. 保留任务目录产出文件（供审计）
  ├─ 6. 通知依赖该任务的任务（status → pending(dependency_failed)）
  └─ 7. 推送 SSE 事件 task_cancelled
```

**终止 vs 取消：**
- **取消（cancelled）：** 用户主动，任务从头到尾放弃
- **终止（failed）：** 执行失败，可重试
- **归档（archived）：** 终态任务的软删除——写 `archived_at` 时间戳（决策 34，不新增 TaskStatus 枚举值），不在看板显示但保留数据

**清理的幂等性：** worktree 已不存在时跳过删除，不报错。

**取消/重试时的会话隔离：** 任务取消后重新执行，**不复用之前的会话和 checkpoint**：

| 场景 | 处理 |
|---|---|
| 取消后重新创建任务 | 新 task_id，全新游标和会话，与旧任务无关联 |
| 失败后点击"重试" | 同一 task_id，但需清理：旧游标全部归档、插入单条 main 游标指向 `init.execute`（决策 90 / 113）、`kanban_node_conversations`（旧会话写 `archived_at` 归档保留供审计，列表默认过滤，不参与新执行，决策 113 同构）；任务置回 `queued` 重新走准入（决策 117）。`failed` 状态经此回到 `init`（决策 70） |
| 阶段内 validate 重试 | 节点级独立对话（§10.4），天然不复用 |
| merge 闸门打回 test | test 游标重入 `test.execute`，`test_result.gate_recheck = true`；`gate_failures` 保留在 merge metadata（决策 85 / 108 / 109） |

```rust
// crates/core/src/storage/tasks.rs

pub async fn reset_task_for_retry(db: &SqlitePool, task_id: &str) -> Result<()> {
    // 重试前重置执行态，历史产出保留但不复用
    db.archive_conversations(task_id).await?;     // 旧会话写 archived_at（不物理删除）
    db.reset_cursors(task_id).await?;             // 归档全部游标行，插入单条 main 指向 init.execute（决策 90 / 113）
    db.reset_task_state(task_id, "init", "execute").await?;  // validate_attempts 归零
    reset_worktree_to_base(task_id).await?;       // git reset --hard {base_ref} + git clean -fdx（决策 125）
    Ok(())
}
```

> **重试的会话归档（决策 113 同构）：** `kanban_node_conversations.archived_at` 是会话行的归档标记列（迁移 `0003_conversations_archived.sql`）。重试把该任务全部旧会话标记 `archived_at`，**行仍物理保留**（`run_id` 外键因此不悬空，历史随时可查）；`GET /tasks/{id}/conversations` 默认只返回未归档行，传 `?include_archived=true` 可取回历次 attempt。终态任务的会话保留策略（`conversation_retention_days` 到期清理，§12.4.3）不受影响：清理按 `created_at` 删除，与归档标记互不干扰。

> **游标模型的表述（决策 90 / 113）：** checkpoint 现在就是 `kanban_node_cursors` 的行集合，所以"重置 checkpoint"的准确说法是**把游标重置为单条 main 行**（`stage=init, node=execute`）。终态任务保留游标行供审计；"重试"将旧行归档（`status=archived`）后插入新的单条 main 行——游标行**永不物理删除**，`kanban_node_runs.cursor_id` 的外键因此不悬空。

> **retry 的 worktree 重置（决策 125）：** failed 可在任意阶段终止，worktree 里留有半成品；init.execute 的幂等策略是"已存在则复用"，不会清场。因此 retry 的 reset 事务显式把工作区恢复到任务分支起点（reset --hard + clean -fdx，2026-09-12 起走 git2 实现，`kanban_node_commands` 仍记录对应逻辑命令行供审计）——否则 develop agent 会在脏工作区上开工，与"重试 = 从头走"的语义冲突。

### 12.4 可观测性

可观测性覆盖三个维度：**指标**（跑了多久、花了多少）、**流转状态**（现在在哪、怎么走到这的）、**会话内容**（每个节点 agent 说了什么、做了什么）。

#### 12.4.1 指标采集

**指标采集：** 每次节点执行写入 `kanban_node_runs`，记录：

| 字段 | 用途 |
|---|---|
| prompt_tokens / completion_tokens | 成本分析 |
| duration_ms | 性能分析，找出慢节点 |
| attempt | 重试率分析 |
| status | 成功率分析 |
| error | 失败原因归类 |
| prompt_template_hash | 按 prompt 版本对比指标（决策 137） |

> **不调 LLM 的节点也落 run 行（决策 99 / 114）：** 纯代码阶段（`init` / `sync-check` / `merge` / `done`）与纯代码 `validate_output`（`develop` / `test`）都没有 agent 调用，但**同样写入 `kanban_node_runs`**（`agent_type = "system"`，token 为 0，`last_activity_at` 由系统命令刷新），否则超时检测与耗时统计会漏掉这几段、系统测试命令（最长 600s）没有所属 run 供命令表挂靠与心跳刷新，且"每个节点执行都有记录"不再成立。因此决策 63 的 1:1 关系要**收窄为：调用 LLM 的 run 才与会话 1:1**；`agent_type = "system"` 的 run 没有会话行。

> **伪阶段也有独立的 run 与会话行（决策 100）：** `conflict_check` 与 `project_analysis` 落独立 run 行（`agent_type = "pseudo:conflict_check"` / `"pseudo:project_analysis"`，`parent_run_id` 指向父 run，`cursor_id` 继承父游标）并与自己的一行会话对应。这样用户能看到"为什么判了 `duplicate_risk`"——而这恰恰是唯一需要人工裁决的 pending。其失败视为**父节点失败**，不触发独立的节点级重试；心跳写父 run 的 `last_activity_at`（决策 88）。

**关键指标：**

| 指标 | 计算方式 | 用途 |
|---|---|---|
| 阶段平均耗时 | `AVG(duration_ms) GROUP BY stage` | 找瓶颈 |
| 阶段重试率 | `attempt > 1 的比例` | 找 prompt 质量问题 |
| validate 通过率 | `validate_output 首次通过的比例` | 找上游质量问题 |
| 各闸门逃逸率 | 下游质量事件数 ÷ 上游闸门放行数（按阶段聚合，决策 137） | 定位"哪个闸门在漏检"：review 打回 / merge 闸门失败归属到本应拦住它的上游 validate 闸门，用于校准 validator prompt 与模型档位（决策 133 / 134） |
| prompt 版本对比 | 按 `prompt_template_hash` 分组的重试率 / validate 通过率（决策 137） | 验证 prompt 改动是否真的有效 |
| 单任务成本 | `SUM(prompt_tokens + completion_tokens)` | 成本核算 |
| 任务成功率 | `done / (done + failed + cancelled)` | 整体健康度 |
| 打回次数分布 | 按打回来源统计 | 找流程薄弱环节 |

**查询示例：**

```sql
-- 各阶段平均耗时和重试率
SELECT
    stage,
    AVG(duration_ms) AS avg_duration,
    AVG(CASE WHEN attempt > 1 THEN 1.0 ELSE 0.0 END) AS retry_rate,
    COUNT(*) AS total_runs
FROM kanban_node_runs
WHERE started_at > datetime('now', '-7 days')
GROUP BY stage
ORDER BY avg_duration DESC;

-- 各闸门逃逸率（决策 137）：下游质量事件相对上游放行量的比例
-- 下游质量事件 = review 打回（kanban_transitions.trigger='kickback'，to_stage='develop'，
--               来源 review）+ merge 闸门失败（metadata_json 的 gate='fail'）；
-- 上游放行量 = 对应 validate_output 的 normal 流转次数。
-- v1 不做自动归因到具体上游闸门（escaped_from 推断列留 v2），只按阶段对比悬殊度。
SELECT
    t.from_stage AS escaped_from_hint,
    COUNT(*) AS escape_events
FROM kanban_transitions t
WHERE t.trigger = 'kickback'
  AND t.created_at > datetime('now', '-30 days')
GROUP BY t.from_stage;
```

**Trace 关联：** 使用 `tracing` + `tracing-subscriber`，节点 span 带上 `task_id` / `stage` / `node`，全链路可追溯。

#### 12.4.2 任务流转状态展示

**流转记录表：** 每次节点切换写入一条记录，形成完整的时间线：

```sql
CREATE TABLE IF NOT EXISTS kanban_transitions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    branch TEXT NOT NULL DEFAULT 'main',  -- 并行分支消歧（决策 84）
    from_stage TEXT,                    -- 来源阶段（init 时为 NULL）
    from_node TEXT,                     -- 来源节点
    to_stage TEXT NOT NULL,             -- 目标阶段
    to_node TEXT NOT NULL,              -- 目标节点
    trigger TEXT NOT NULL,              -- 触发方式
    reason TEXT,                        -- 触发原因描述
    created_at TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id)
);
```

**trigger 取值：**

| trigger | 说明 |
|---|---|
| `normal` | 正常流转（validate_output 通过 → 下一阶段） |
| `retry` | validate_output 判定不通过 → 回到 execute 重试（validate_attempts +1） |
| `node_retry` | 节点级 agent loop 整体失败（元数据校验失败 / 超时 / 崩溃）→ 干净对话重试同一节点（决策 33） |
| `kickback` | 跨阶段打回（review 不通过 → develop、sync-check backtrack → architect、merge 冲突 → develop、merge 闸门 lint 失败 → develop（决策 139））；`retry_exhausted` 的 `goto architect-design` 同记为本类（决策 138） |
| `user_resume` | 用户操作后恢复（补充信息 / 跳过 / 回退） |
| `auto_resume` | 调度器自动恢复（冲突任务终态 / 依赖恢复） |
| `timeout` | 超时触发重试 |
| `start` | 任务首次启动 |

**并行分支的展示（决策 84）：** 所有 SSE 事件体带 `branch` 字段（`"main"` / `"develop-design"` / `"test-design"`），前端据此把事件归到对应分支；`kanban_transitions` 同样记录 `branch`，因此时间线在并行区间会出现两条交错记录（用 `∥` 标识）。看板卡片在并行区间渲染两个"当前节点"药丸，可分别展开；pending 药丸按分支着色，两个分支各自的 `allowed_actions` 独立下发。串行区间 `branch` 恒为 `"main"`，展示与单游标时一致。

**前端展示 —— 流水线视图：**

```
┌─────────────────────────────────────────────────────────────────────┐
│  任务: 实现用户登录                       状态: running  ⏱ 12m 34s    │
│  成本: 45.2k token（金额展示延后 v2，决策 131）                        │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ✓ init → ✓ architect-design → ● develop-design                     │
│                                    ╲                                │
│                                     ✓ test-design                   │
│  ○ develop → ○ review → ○ test → ○ merge → ○ done                   │
│                                                                     │
│  ┌─ 当前节点 ────────────────────────────────┐                       │
│  │ develop-design.execute （第 2 次尝试）      │                       │
│  │ ████████████░░░░░░░  62%                  │                       │
│  └────────────────────────────────────────────┘                      │
└─────────────────────────────────────────────────────────────────────┘
```

**节点状态图例：** `✓` 已完成 / `●` 执行中 / `○` 未开始 / `⏸` pending / `✗` 失败 / `↩` 已打回重跑

> **sync-check 不作为位置展示（2026-09-11 前端评审确认）：** sync-check 不占游标行（决策 107），任务永远不会"停在"它上面——看板不设 sync-check 列，流水线视图、轨道图、卡片迷你轨一律不渲染该节点；并行双轨在图上直接合流进 develop，汇合状态由分支药丸的 `waiting_join`（"等待汇合"）表达，backtrack 显示为回到 architect-design 的自动流转记录，其 join run 不进入时间线展示。

**流转时间线（可展开）：**

```
12:00:01  start            → init.execute
12:00:03  normal           → architect-design.validate_input
12:00:15  normal           → architect-design.execute
12:01:40  normal           → architect-design.validate_output
12:01:52  retry            → architect-design.execute      （validate_output 不通过：缺少数据流定义）
12:03:20  normal           → architect-design.validate_output
12:03:28  normal           → develop-design.validate_input ∥ test-design.validate_input
12:04:55  node_retry       → develop-design.execute        （submit_metadata 格式错误，干净对话重来）
12:06:10  normal           → develop-design.validate_output
...
```

**API：**

| 接口 | 说明 |
|---|---|
| `GET /tasks/{id}/flow` | 返回流转时间线 + 当前节点 + 节点状态汇总 |
| `GET /tasks/{id}/stream` | SSE 实时推送，流转事件作为其中一类事件（决策 76） |

#### 12.4.3 节点会话内容存储与查看

每个节点执行的完整 LLM 对话需要落库，用于排查"agent 为什么这么判断"：

```sql
CREATE TABLE IF NOT EXISTS kanban_node_conversations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT,                       -- 任务级会话的所属任务；项目级伪阶段为 NULL
    project_id TEXT,                    -- 项目级伪阶段（project_analysis）的归属（票 10）；任务级为 NULL
    run_id INTEGER NOT NULL,            -- 关联 kanban_node_runs.id（决策 77）
    stage TEXT NOT NULL,
    node TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    agent_type TEXT NOT NULL DEFAULT 'main',  -- main | code_searcher | test_runner | doc_writer（子代理，决策 77）
    parent_run_id INTEGER,              -- 子代理关联父 run（决策 77）
    messages_json TEXT NOT NULL,        -- 完整对话：system / user / assistant / tool
    metadata_json TEXT,                 -- submit_metadata 提交的内容
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    archived_at TEXT,                   -- 重试归档标记（决策 113 同构）：非 NULL = 历史 attempt，列表默认过滤
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (project_id) REFERENCES kanban_projects(id),
    FOREIGN KEY (run_id) REFERENCES kanban_node_runs(id),
    CHECK ((task_id IS NOT NULL) <> (project_id IS NOT NULL))   -- 归属恰好其一（决策 100 / 票 10）
);
```

> **1:1 的确切含义（决策 77 / 99 / 100）：** `kanban_node_runs` ↔ `kanban_node_conversations` 的 1:1 **只对"调用 LLM 的 run"成立**（每个节点尝试一行 run + 一行会话）。据此：
> - **子代理**各有一行 run（`agent_type` / stage / node 继承父级，`attempt` 沿用父级），再对应自己那一行会话，用 `parent_run_id` 指向父 run；
> - **伪阶段**（`conflict_check` / `project_analysis`）同样是独立的 run + 会话行（`agent_type = "pseudo:*"`，`parent_run_id` 指向父 run）；
> - **不调 LLM 的节点**（`init` / `sync-check` / `merge` / `done`，及 `develop` / `test` 的纯代码 `validate_output`）有 run 行（`agent_type = "system"`）但**没有**会话行（决策 99 / 114）。
>
> 因此 `run_id` 对所有会话恒非空，无需为任何形态破例。

**messages_json 结构**（与 LLM 调用时的 messages 一致）：

```json
[
  {
    "role": "system",
    "content": "你是架构设计 agent。...",
    "cache_control": {"type": "ephemeral"}
  },
  {
    "role": "user",
    "content": "任务标题：实现用户登录\n任务描述：..."
  },
  {
    "role": "assistant",
    "content": "我先分析需求...",
    "tool_calls": [
      {
        "id": "call_1",
        "function": {
          "name": "write_file",
          "arguments": "{\"path\": \"design.md\", \"content\": \"# 实现用户登录\\n...\"}"
        }
      }
    ]
  },
  {
    "role": "tool",
    "tool_call_id": "call_1",
    "content": "{\"success\": true, \"path\": \"design.md\"}"
  },
  {
    "role": "assistant",
    "tool_calls": [
      {
        "id": "call_2",
        "function": {
          "name": "submit_metadata",
          "arguments": "{\"readiness\": true, \"affected_files\": [\"src/auth.py\"], \"conflict_warnings\": []}"
        }
      }
    ]
  }
]
```

**注意：** 重试采用节点级独立对话（§10.4），所以每次 attempt 是**一条独立记录**，不包含上一次重试的历史。一个节点尝试内的多轮 LLM 调用（assistant + tool 往返）累积进同一个 `messages_json`，因此能清晰看到每次尝试的完整上下文。子代理的会话是**独立的行**（自己的 `run_id`，`agent_type` 非 `main`，`parent_run_id` 指向父 run），不与父会话混在同一个 `messages_json` 里（决策 77）。

**前端展示 —— 会话查看器：**

```
┌─────────────────────────────────────────────────────────────────────┐
│  develop-design.execute · 尝试 2/3 · 12.4k tokens · 38.2s           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  [System Prompt]  ▼ 展开（2.1k token）                              │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │ 你是开发方案 agent。根据设计文档输出详细开发方案...            │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                     │
│  [User]  设计文档路径：design.md ...                                │
│                                                                     │
│  [Assistant]  我先读取设计文档...                                   │
│    🔧 read_file(path="design.md")          ✓ 3.2k chars            │
│    分析后，我准备输出开发方案...                                    │
│    🔧 write_file(path="dev-plan.md")       ✓ 5.8k chars            │
│    🔧 submit_metadata(readiness=true, ...) ✓                       │
│                                                                     │
│  ┌─ 提交的元数据 ─────────────────────────┐                          │
│  │ readiness: true                        │                          │
│  │ file_changes: [3 项]                   │                          │
│  └────────────────────────────────────────┘                          │
│                                                                     │
│  [尝试 1/3]  ✗ submit_metadata 校验失败：缺少 readiness 字段         │
│             点击查看完整对话 ▸                                       │
└─────────────────────────────────────────────────────────────────────┘
```

**API：**

| 接口 | 说明 |
|---|---|
| `GET /tasks/{id}/conversations` | 列出节点会话摘要；默认只返回未归档行，`?include_archived=true` 取回含重试归档的历史 attempt（§12.2 / 决策 113 同构） |
| `GET /tasks/{id}/conversations/{run_id}` | 单个节点会话完整内容 |
| `GET /tasks/{id}/conversations/{run_id}/messages` | 仅返回 messages 数组（供前端渲染） |

> **messages 端点的隔离语义：** 响应体直接是 messages 数组；查询恒以 `task_id + run_id` 为条件，run 不存在、或存在但不属于该 task 时一律 404，不泄露其他任务数据（与 `GET /tasks/{id}/commands/{cmd_id}` 同姿态）。前端会话查看器仍走整条会话端点 `{run_id}`——它需要同一行里的 `metadata_json` 渲染「提交的元数据」卡片，且已按 run 惰性加载（选中才拉取，不进列表）；`/messages` 端点供只需要消息数组的按需场景使用。

**存储与保留策略：**

| 策略 | 说明 |
|---|---|
| 落库时机 | 每次节点尝试结束后写入（成功或失败都写；一次尝试 = 一行，决策 63） |
| 内容裁剪 | 超过 `conversation_max_chars`（默认 200k 字符）的 messages 截断，保留首尾 |
| 保留期限 | 终态任务保留 `conversation_retention_days`（默认 30 天），到期清理 |
| 重试归档 | 重试把旧会话写 `archived_at`（不物理删除），列表默认过滤、`?include_archived=true` 取回（§12.2，决策 113 同构） |
| 进行中任务 | 不清理，保证随时可查 |
| 敏感信息 | tool 调用参数中的 API key / token 用 `***` 脱敏后存储 |

**与 tracing 的关系：** `tracing` 输出记录节点的起止时间、入参出参摘要；`kanban_node_conversations` 记录完整 LLM 对话。前者轻量用于全局分析，后者重量用于单点排查，两者通过 `run_id` 关联。

**可观测性与 pending 的联动：** 用户看到 pending 卡片时，可直接展开"为什么 pending"——查看触发 pending 的那个节点的完整会话，理解 agent 的判断依据，而不只是看到一句 `message`。

#### 12.4.4 命令与输出记录

**问题：** 不是所有阶段都有 agent。`init` / `sync-check` / `merge` / `done` 是纯代码逻辑，`develop.validate_output` 等节点由框架直接执行测试。这些操作同样需要可观测——用户要能看到"系统实际跑了什么命令、结果如何"。

**统一命令日志表：**

```sql
CREATE TABLE IF NOT EXISTS kanban_node_commands (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    run_id INTEGER,                     -- 所属节点 run 行（决策 99 / 114 / 131）：节点内命令恒非空；仅取消/归档的任务级清理命令可为 NULL（不在任何节点 run 内）
    stage TEXT NOT NULL,
    node TEXT NOT NULL,
    source TEXT NOT NULL,               -- "agent"（run_command 工具）| "system"（框架执行）
    command TEXT NOT NULL,              -- **实际执行**的完整命令行（脱敏后）
    original_command TEXT,              -- 改写前的原串（迁移 0034）。**只有真的发生过改写才写**（决策 297）
    cwd TEXT NOT NULL,                  -- 执行目录（worktree 绝对路径）
    exit_code INTEGER,                  -- NULL = 执行中
    stdout_path TEXT,                   -- 完整输出文件路径（超过阈值时卸载）
    stdout_preview TEXT,                -- 首尾预览
    stderr_preview TEXT,
    duration_ms INTEGER,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (run_id) REFERENCES kanban_node_runs(id)
);
```

**记录范围 —— 系统驱动的命令：**

| 阶段.节点 | 系统执行的命令 |
|---|---|
| init.execute | `git worktree add`、`git branch` |
| develop.validate_output | `lint_command`（如已配置，决策 139）+ 按 `test_framework` 构建的单元测试命令（如 `cargo test` / `pytest` / `npm test`） |
| review.validate_output | 无（纯代码判断 approved） |
| test.execute | 按 `test_framework` 构建的集成测试命令（由 agent 通过 `run_command` 触发，agent 驱动的命令） |
| test.validate_output | 无（读 `test_result` 元数据判断） |
| merge.execute | `git fetch`、`git rebase`、lint（如已配置）+ 单元测试 + 集成测试命令、生成 diff、合入（ff / `--no-ff`） |
| done.execute | `git worktree remove`、`git branch -D` |
| 取消 / 归档 | `git worktree remove --force`、`git branch -D` |

系统命令的 `run_id` 挂在其所属节点**自己的 run 行**上——纯代码节点按决策 99 / 114 落 system run 行（`sync-check` 的 run 指向推进事务内新建的 main 游标，决策 113）。

**两列的分工（决策 297 / 迁移 0034）：**`command` 记**实际执行**的那条串，`original_command` 记**改写前**的原串——且**只有真的发生过改写才写**。
三处「按原样跑」（开关关着 / 闸门 / `run_readonly`）对台账是同一件事：`original_command` 为 `NULL`。
只记实际执行的串会丢掉原串；只记原串则 `cat X` 与 `rtk read X` 的输出不一样、排障会看错；记两行会混淆「跑了几条命令」这个计数。
故「加一列」是这里唯一正确的形状。**台账的折叠行显示原串**（改写过的行带一枚「改写」小标），**展开时原串与实际执行的那条都摆出来**——排障的人先要看到的是「模型想干什么」而不是「这条命令被换成了什么」。

**四条写路径收在同一条管道里（决策 297）：**agent 的 `run_command` / `run_readonly`、执行器的闸门、修复的闸门都走
`crates/core/src/exec.rs::CommandRunner`——启动 → 流式采集 → 超时收口（杀**进程组**，连子孙一起）→ 脱敏 → 台账。
收口之前两个闸门是裸 `sh -c` 旁路：没有进程组（超时只丢 future、不杀进程）、从不回填 `process_group_id`（调度器那条
超时收口够不着它）、没有心跳，而修复闸门**完全没有超时**（一条挂住的测试命令能把这一班永久钉住）、命令串还漏了脱敏。
判决顺序是**不变量**：`check(原命令) → 改写 → 落台账 → spawn`——被拒的命令既不改写也不启动。

**命令改写与它的降级（决策 297）：**设置页「命令执行」那一颗开关打开后，`run_command` 的命令经本机 rtk 改写
（`cat X` → `rtk read X` 之类）；**闸门与 `run_readonly` 一律不改写**。**rtk 不在场时命令原样执行**、exit code 照常，
每条命令现读一次库里的开关，找不到二进制时**留一条 `tracing::warn`** 并按原样跑——优化器不可用不该升级成整条命令失败。
开关那一行住 `kanban_rtk`（迁移 0035，单行 + `CHECK (id = 1)`，行缺席 = 缺省关），`GET /rtk` 带一次**活体探测**，
探测失败**不拦保存**（界面把失败原样摆出来）。**关掉时 shim 目录一并拆掉**（不留残迹：留着那条链接是「这台机器还在用 rtk」
的假证据，二进制被卸掉之后它还是一条悬空链接）；**同一个不可用原因只留一条 `tracing::warn`**（命令是热路径，这类失败是持续性的，
按条报只会把日志刷成噪声）；启用时把解析到的绝对路径钉成 `{home}/rtk-shim/` 里那**唯一**一个符号链接、前置进**子进程**的 PATH
——同一目标上这一步是**幂等**的（不重建目录，故并发命令下不会开一个「shim 里暂时没有 rtk」的空窗）。
**改写这一跳有自己的预算，到点就放行**：热路径上给 2s（`rtk hook claude` 是一次本地进程调用，实测 ~0.7s；机器忙时它会到点，
那一刻**这条命令按原样跑**——少省一次 token，而不是失败）；`GET /rtk` 那次探测给 5s，因为它的结论是**显示在设置页上的**，
拿短预算去判等于把「机器忙」说成「这台机器的 rtk 不能改写」。

**记录范围 —— agent 驱动的命令：** agent 通过 `run_command` 工具执行的所有命令（写代码后的自测、跑 lint 等）同样写入该表，`source = "agent"`，并通过 `run_id` 关联到对应会话。这样用户在一个地方看到**所有**实际执行的命令，不用在会话和系统日志之间切换。

**统一的执行封装：**

```rust
// crates/core/src/agent/tools.rs

pub async fn run_recorded_command(
    task_id: &str, stage: &str, node: &str,
    cmd: &[String], cwd: &str, source: &str,
    run_id: Option<i64>,
) -> Result<CommandResult> {
    // 所有命令执行的统一入口，自动记录、脱敏、卸载长输出
    let sanitized = sanitize_command(cmd);          // 脱敏 token / credentials
    let record = db.insert_command(task_id, stage, node, source, &sanitized, cwd, run_id).await?;

    push_sse(task_id, SseEvent::CommandStarted { command_id: record.id, command: sanitized.clone() }).await?;

    // 使用 tokio::process::Command 执行命令
    let output = tokio::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd)
        .output()
        .await?;

    // 长输出卸载（复用 §12.13 L2 策略）
    let stdout_path = if count_tokens(&output.stdout) > settings.offload_threshold_tokens {
        Some(offload_command_output(task_id, record.id, &output.stdout).await?)
    } else {
        None
    };

    db.finish_command(
        record.id,
        output.status.code(),
        stdout_path.as_deref(),

        head_tail_excerpt(&output.stdout, 50, 100),
        head_tail_excerpt(&output.stderr, 50, 100),
        elapsed_ms,
    ).await?;

    push_sse(task_id, SseEvent::CommandFinished {
        command_id: record.id,
        exit_code: output.status.code(),
        duration_ms: elapsed_ms,
    }).await?;

    Ok(CommandResult {
        exit_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        stdout_path,
    })
}
```

**实时流式输出：** 长命令执行期间按行推送，不等到结束：

```
command_started (git rebase origin/main)
command_output  (chunk: "First, rewinding head to replay your work...")
command_output  (chunk: "Applying: feat: add login")
command_finished (exit_code=0, duration_ms=3200)
```

流式策略复用 §12.11 的分级：**错误行实时全推，正常输出按行采样推送**（如每 10 行推一行 + 首尾），避免海量日志刷屏。完整输出始终落库，用户可展开查看。

**前端展示 —— 节点详情的"命令与输出"页签：**

```
┌─────────────────────────────────────────────────────────────────────┐
│  merge.execute                              ⏱ 3m 12s  · 6 条命令    │
├─────────────────────────────────────────────────────────────────────┤
│  [会话]  [命令与输出 ●]                                             │
├─────────────────────────────────────────────────────────────────────┤
│  ✓ 12:01:03  sys  git fetch origin                       0.8s  exit 0 │
│  ✓ 12:01:04  sys  git rebase origin/main                 3.2s  exit 0 │
│  ✓ 12:01:08  sys  cargo test -- --test-threads=1        18.4s  exit 0 │
│  ✓ 12:01:27  sys  git diff main..kanban/abc123           0.3s  exit 0 │
│                                                                     │
│  ┌─ git rebase origin/main ─────────────────────────────────────┐   │
│  │ $ git rebase origin/main                                     │   │
│  │ First, rewinding head to replay your work on top of it...    │   │
│  │ Applying: feat: add login endpoint                           │   │
│  │ [exit 0]  3.2s                                               │   │
│  └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

**API：**

| 接口 | 说明 |
|---|---|
| `GET /tasks/{id}/commands` | 列出所有命令（可按 stage/node 过滤），返回摘要 |
| `GET /tasks/{id}/commands/{cmd_id}` | 单条命令详情（含完整输出或卸载路径） |
| `GET /tasks/{id}/commands/{cmd_id}/output` | 完整 stdout（从卸载文件读取） |

**安全：**
- 命令**脱敏**：URL 中的 credentials、`--token` 参数、**敏感名环境变量**（名含 `TOKEN`/`SECRET`/`KEY`/`PASSWORD`/`PASSWD`/`CREDENTIAL`/`AUTH` 的 `NAME=value`、`export NAME=value`、`$NAME`/`${NAME}` 展开）替换为 `***` 后存储（票 15 / 决策 118：按**变量名**判定；`PATH=`、纯数字、已含 `***` 的值与 `FOO=secret` 这类非敏感名保留，取舍见 `crates/core/src/agent/sanitize.rs` 模块文档）
- 输出脱敏：正则过滤疑似密钥（`sk-*`、`ghp_*`、长 base64）——在结果**回填 agent messages 之前**执行（决策 118）：agent 看到的即脱敏后文本，落库与 context 同源，密钥既不进日志也不进会话；误伤面（合法长 base64 被打码）为已知代价
- **cwd 约束（如实描述，决策 104）：** `cwd` 默认取 worktree，这**只是卫生默认值，不构成安全边界**。v1 **不做系统级沙箱**，`run_command` 的 shell 不受限——agent 可以通过 `cd`、绝对路径、子 shell 等方式访问任务目录与 worktree 之外的任何路径。跨任务污染与读取本机明文密钥（§12.14）**系统不阻止**，只能靠本表的命令日志事后审计。文件工具（`read_file` / `write_file` / `edit_file` / `delete_file` / `list_dir`）**受** `FileToolPolicy` 强制约束（§10.6.2）。

**心跳刷新（决策 100）：** `run_recorded_command` 在命令**开始与结束**时都要刷新所属 run 的 `last_activity_at`，长命令执行期间按输出行周期刷新。否则 merge 的合入闸门（最长 `test_command_timeout_sec` = 600s）会被 `node_idle_timeout_sec`（300s）误判为空闲超时并杀掉进程组。

**与 §12.4.3 的分工：**

| 表 | 记录内容 | 回答的问题 |
|---|---|---|
| `kanban_node_conversations` | LLM 说了什么、调了什么工具 | agent 为什么这么判断 |
| `kanban_node_commands` | 实际执行了什么命令、结果如何 | 系统真的做了什么 |

两者通过 `run_id` 关联：agent 的一次 `run_command` 调用，在会话中是 `tool_call`，在命令表中是执行明细。

### 12.5 Review 人机协作

支持两种 review 模式，任务创建时指定：

| 模式 | 行为 |
|---|---|
| `agent`（默认） | review agent 自动评审，通过则继续，不通过则 pending 让用户决定 |
| `human` | review agent 只做**预审**（生成报告），然后 pending(human_review) 等待人工评审 |

**human 模式流程：**

```
review.execute（agent 预审）
  → 生成 review-report.md
  → pending(human_review)，通知用户
  → 用户在 UI 上查看：
      - 变更 diff（review-diff.diff，决策 124）
      - agent 预审报告
      - 单元测试结果（develop.validate_output 产出；集成测试在 review 之后，此时尚不存在）
  → 用户提交评审结果：
      POST /tasks/{id}/review {approved: true/false, comments: "..."}
  → approved → test；rejected → develop.execute（带用户评论）
```

> **阻塞语义（决策 2）：** human 模式下任务阻塞等待人工操作（pending），不设超时自动放行。

> **评审 diff 的生成（决策 124）：** `merge-proposal.diff` 在 merge 阶段才生成，review 时不存在。review_mode = human 时，review.execute 完成后由系统执行 `git diff {base_ref}..kanban/{task_id}`（走 `run_recorded_command`，`source = "system"`），写任务目录 `review-diff.diff` 并落 `kanban_stage_outputs`（`output_type = "review_diff"`），dossier 经 `GET /tasks/{id}/files/review-diff.diff` 读取。

### 12.6 任务依赖

**声明方式：** 创建任务时指定 `depends_on`：

```json
POST /tasks
{
  "title": "实现用户登出",
  "description": "...",
  "depends_on": ["{task-a-id}"]
}
```

**调度规则：**

```
任务创建
  │
  ├─ depends_on 为空 → status = queued（等并发准入，决策 98）
  │
  └─ depends_on 非空 → status = waiting
        │
        └─ 监听依赖任务状态：
             ├─ 所有依赖 done → status = queued（等并发准入，决策 98）
             ├─ 任一依赖 failed → pending(dependency_failed)（挂 main 游标）
             │    用户选择：继续执行 / 取消任务 / 等待依赖重试
             └─ 任一依赖 cancelled → pending(dependency_failed)
```

**依赖的实现：** `KanbanScheduler.tick()` 里检查 waiting 任务：

```rust
// crates/core/src/scheduler/tick.rs

async fn check_waiting_tasks(&self) -> Result<()> {
    let tasks = self.db.get_tasks_by_status("waiting").await?;
    for task in tasks {
        let deps = self.db.get_dependencies(&task.id).await?;
        if deps.iter().all(|d| d.status == TaskStatus::Done) {
            // 依赖满足 → 不是直接 running，而是转入 queued 等并发准入（决策 98）
            self.db.set_status(&task.id, TaskStatus::Queued).await?;
        } else if deps.iter().any(|d| d.status == TaskStatus::Failed || d.status == TaskStatus::Cancelled) {
            let failed: Vec<_> = deps.iter()
                .filter(|d| d.status == TaskStatus::Failed || d.status == TaskStatus::Cancelled)
                .collect();
            let names: Vec<_> = failed.iter().map(|d| d.title.as_str()).collect();
            // pending 挂在任务的 main 游标上（决策 90），不是任务级特例
            self.db.set_main_cursor_pending(&task.id, PendingReason {
                type_: PendingReasonType::DependencyFailed,
                stage: "init".to_string(),
                node: "execute".to_string(),
                message: format!("依赖任务失败：{}", names.join(", ")),
                suggested_actions: vec!["继续执行".to_string(), "取消任务".to_string(), "等待依赖重试".to_string()],
                ..Default::default()
            }).await?;
            self.db.sync_task_projection(&task.id).await?;
        }
    }
    Ok(())
}

/// 并发准入（决策 98 / 117）：queued → running 的唯一出口
async fn admit_pending_tasks(&self) -> Result<()> {
    // 名额占用 = running + pending（决策 117）：pending 任务仍持有 worktree，不释放名额
    let occupied = self.db.count_tasks_by_statuses(&["running", "pending"]).await?;
    let slots = self.settings.max_concurrent_tasks.saturating_sub(occupied);
    for task in self.db.get_tasks_by_status("queued").limit(slots).await? {
        self.start_task(&task).await?;   // 置 running 并 spawn run_executor
    }
    Ok(())
}

/// 依赖恢复（决策 57）：依赖任务从 failed 被重试转回 running 时，
/// 把因 dependency_failed 进入 pending 的任务清 pending、退回 waiting。
async fn recover_dependency_failed(&self) -> Result<()> {
    for task in self.db.get_tasks_with_pending_type(PendingReasonType::DependencyFailed).await? {
        let deps = self.db.get_dependencies(&task.id).await?;
        if deps.iter().any(|d| d.status == TaskStatus::Running) {
            self.db.clear_pending(&task.id).await?;
            self.db.set_status(&task.id, TaskStatus::Waiting).await?;
        }
    }
    Ok(())
}
```

**依赖与文件冲突的关系：** 有依赖关系的任务，后置任务在前置任务合入 `default_branch` 后才开始。所有任务的分支都基于创建时的 `default_branch` HEAD，不使用链式分支依赖（避免一个任务失败影响所有下游）。

**依赖失败后的恢复：** `dependency_failed` 的 pending 提供"等待依赖重试"选项。用户在前置任务上点"重试"后，前置任务转回 `running`，scheduler 的 `recover_dependency_failed()` 把后置任务清 pending 并退回 `waiting`，待前置任务再次 `done` 后正常启动（决策 57）。

**"继续执行"的语义（决策 116）：** "继续执行" = **忽略失败依赖**——任务置回 `queued` 正常走并发准入，任务上记一条 `dependency_overridden` 警告。**不得**清 pending 后退回 `waiting`：那会被 `check_waiting_tasks` 下一个 tick 重新 pending，形成死循环。动作集按失败依赖的终态裁剪：依赖 `failed` → 提供"等待依赖重试"；依赖 `cancelled` → 只提供"继续执行 / 取消任务"（cancelled 没有重试路径）。

**循环依赖检测：** 创建任务时（API 层）做拓扑排序检测，存在环则拒绝创建并返回 400（决策 27）。

### 12.7 通知机制

**在线通知（默认）：** SSE 事件流推送到前端：

| 事件 | 触发时机 | 前端行为 |
|---|---|---|
| `node_started` | 节点开始执行 | 更新进度条（按 `branch` 归位） |
| `node_finished` | 节点完成 | 更新进度 |
| `cursor_changed` | 游标状态变化（含 `waiting_join`） | 更新对应分支的节点状态 |
| `command_started` | 命令开始执行（agent 或系统） | 命令列表新增一行 |
| `command_output` | 命令产生输出 | 追加到命令输出区 |
| `command_finished` | 命令结束 | 更新退出码和耗时 |
| `conversation_delta` | agent 流式产出文本 / token 增量（决策 123） | 会话查看器追加消息文本、token 计数累加 |
| `tool_event` | agent 发起工具调用 / 工具返回（决策 123） | 会话查看器增 / 收工具调用卡 |
| `pending` | 进入 pending | 卡片转琥珀 + toast；详情页展开待办面板（2026-09-11 前端评审：持久面板，非弹窗） |
| `pending_updated` | pending 内容变化 | 更新卡片 |
| `stage_changed` | 阶段切换 | 更新看板列 |
| `task_done` / `task_failed` / `task_cancelled` | 终态 | 提示用户，更新看板 |
| `stalled` | 任务停滞超过阈值（pending 超过 `pending_timeout_hours`，置 `stalled = 1`） | 任务卡停滞高亮（见 §12.9 附近对 stalled 的描述） |

> **会话流式事件（决策 123，排掉前端差距①⑤）：** `conversation_delta` 事件体含 `run_id` / `agent_type` / `branch` / `role` / `text`，并带 `prompt_tokens` / `completion_tokens` 增量（流式 token 计数的唯一来源）；`tool_event` 事件体含 `run_id` / `branch` / `tool` / `phase(start|end|error)`（error 为实现侧扩展：工具被 FileToolPolicy 拒绝或执行失败）/ 参数摘要与结果行数。SSE 只是渲染通道，落库仍走 §12.4.3 的会话写入。

**离线通知（决策 268 落地 2026-09-24——原决策 65 的「v2 预留」随本条进 v1）：**
`[notify].webhook_url` 配置在场即出站：attention 落库且 `wakes()` 时 POST 一条通用 JSON
（`{title, body, kind, task_id, occurred_at, source}`，`body` 只带归因白名单字段、
不带正文/日志原文），URL 缺席 = 整段关死、URL 含 token 只进 config.toml 不进日志。`[notify].format`（决策 270，缺省 `generic`）可切 `feishu`——同一份事件按目标选序列化、报文变成飞书文本消息（`msg_type=text`，text = `title\nbody`），政策语义与格式无关；飞书端安全设置用**自定义关键词 `AgentPipeline`**（`title` 固定前缀命中），不做签名校验。
礼貌语义（每类 cooldown、免打扰 `[22, 8)` 跨零点按本地整点、`failed` 恒发、`pending`
免打扰豁免）在 Rust（`crates/core/src/notify.rs`）与前端
（`frontend/src/lib/notificationPolicy.ts`，只管浏览器 toast）各有一份，由
`tests/fixtures/notification_policy.json` 双端同表钉住（决策 246 先例）；前端还有一层
`notifyOn` 用户偏好开关（`cancelled` 缺省关），后端没有偏好面——差异记在决策 268 与
fixture `$comment`。飞书机器人已可 `format = "feishu"` 直连（决策 270），iMessage 自决策 272 起也有直连通道（见下一段）；其余 IM / 邮件仍可由通用 webhook 转发，独立 SMTP / 专用卡片有证据再议。
**职责划分（决策 130，不动）：SSE 全量推送、不做 cooldown 合并**——它是状态同步通道，
吞事件会丢状态；前端 toast 的 cooldown / quiet_hours 只作用于 toast 层
（frontend-design §9.1 对齐）。

**离线通知·iMessage 通道（决策 272 落地 2026-09-25）：** 经本机 **BlueBubbles** 服务发
iMessage——`format = "bluebubbles"` 时投递目标换成 `POST {端点}/api/v1/message/text`，
报文 `{chatGuid, tempGuid, message, method}`（正文字段名是 **`message`**，官方文档页写
`text` 是错的，以服务端源码为准；`chatGuid` = `iMessage;-;<收件地址>`；`method` 恒
`apple-script`，不暴露配置）。**部署前置**：一台登录着 iMessage 的 Mac 常开 BlueBubbles
服务端（记下端点与 password），收件地址填 Apple ID 或手机号；**消息没到先查
系统设置 → 隐私与安全性 → 自动化**里有没有被拒的授权。这一档多出两条通知线（272②③）：
值班长**回话完成**与**失败收口**——回话线是新类 `foreman_reply`（自有 cooldown 槽、
受免打扰不豁免；`say` 轮要过「这一轮至少 3 次工具调用」的门，快问快答不进手机，
值守播报恒通知、静默轮恒不通知），失败线走 `failed`（恒发，与台账同批同拍）。回话正文
**出网**（截断 200 字 + 会话名在标题里）——这是对 268④「不发正文」的一次显式修订：
收件人是本人的 Apple ID，出机器不出账户；豁免只覆盖回话正文，失败通知仍只带类别、
不带 `raw` 原文。**设置入口**在 `#/settings/notify`（272⑥⑦⑧）：一颗总开关 + 通道四件
（类型 + 端点 + password + 收件人）**整体覆盖** `config.toml`（两级，不允许混；
`origin` 说清谁生效），password 照 provider 的 `***` 掩码范式，BlueBubbles 在开启与
保存时先探活（`GET /api/v1/ping`）、够不着不当成功，切换**活生效**不必重启。
**节流与免打扰自决策 284 起也在这一页**（「礼貌」小节，落实对 272⑥ 的显式修订：
272⑥ 说这两件只住 `config.toml`，理由是「前端已有同语义的一份表，再开一个口就是两处
能改同一个语义」——284 把那份表的适用范围写窄：它只管**浏览器 toast**，出口这条线的
礼貌改由设置页说了算）。形状与通道同构而**各自成立**：界面保存的礼貌单元整体覆盖
`config.toml` 的 `[notify].cooldown_sec` / `[notify].quiet_hours`（组内不许混，两组可以
一个来自界面一个来自配置，各报各的 origin、各交各的）、0–86400 整数秒与 0–23 整点
（越界 400 点名，报错不静默）、保存即按新值重建出口（`WebhookNotifier` 直接持有
`NotifyPoliteness`）。**`0` 与「起止相同」是有含义的合法值**：前者 = 不节流，
后者 = 全天不静默——想让夜里的推送照来，就把起止填成同一个数。

**pending 超时提醒：** 任务进入 pending 超过 `pending_reminder_hours`（默认 24h）未处理，重复提醒一次；超过 `pending_timeout_hours`（默认 72h）自动标记 `stalled = 1`，看板高亮显示。提醒与高亮均通过 SSE 推送，不依赖外部渠道。**票 05 补上了那个「均通过 SSE 推送」的洞**：提醒此前只活在调度器内存的 `HashSet` 里、重启即失、从不外发——现在是 `stalled` 事件真的发出去，同时落一行 `task_stale` 待办（决策 209③）。

**pending 待办展示（2026-09-11 前端评审定稿：持久面板，非弹窗）：** 进入 pending 时（无论人工触发还是调度器自动检测），前端在**任务详情右侧的待办 dossier 面板**常驻呈现下列内容（任务不再 pending 时收起）；看板侧以 toast + 顶栏待办计数提醒。内容包含：

| 元素 | 说明 |
|---|---|
| 阻塞原因 | `pending_reason.message` |
| 建议操作 | `pending_reason.suggested_actions` 渲染为按钮 |
| 上下文 | 触发 pending 的节点会话（§12.4.3，可展开查看 agent 判断依据） |
| Diff 文件 | 若为 `merge_approval`，展示 diff 查看入口 |
| 产出文件 | 相关产出文件的查看入口 |

### 12.8 独立 Graph 与 Agent 运行时

**kanban agent 的核心特征：**

- **独立图：** kanban 使用独立的 DAG（静态落点表，决策 248），各阶段无长期记忆
- **节点级独立对话：** 每次 agent 调用使用独立的对话上下文，不跨节点累积
- **无记忆绑定：** 不绑定 memory_retrieve，各阶段无记忆写回
- **pending 机制：** 通过 pending_reason 实现人工介入（§11.3）

**原因：** kanban 每个节点是**一次性任务**（读输入 → 产出 → 退出），不需要记忆检索、不需要多轮对话累积。

**Tool 重试分层（G13）：**

```
agent loop
  │
  ├─ LLM 返回 tool_calls
  │   └─ 执行工具
  │       ├─ 成功 → 结果回填 messages，继续 loop
  │       └─ 失败 → 错误信息回填 messages，让 agent 自己决定重试
  │               计数 +1，超过 tool_retry_max → loop 失败
  │
  ├─ LLM 返回最终回复（无 tool_calls）
  │   └─ 校验产出 → 通过则退出 loop
  │
  └─ loop 整体失败（超时/超次数/崩溃）
      └─ 节点级重试（agent_retry_max），从干净对话重来
```

**关键：** 单次工具调用失败（如 `read_file` 路径不存在、命令返回非零）不应直接触发节点级重试。agent 应该在 loop 内看到错误并自行调整（如修正路径）。只有 loop 整体无法完成时才走节点级重试。

**子代理支持（只读，需阶段显式声明）：** 每个节点的 agent 可派生一个**只读子代理**处理可分解的检索子任务，把「读 20 个文件」的原文挡在父上下文之外，只回摘要（决策 172③，票 08）。`spawn_sub_agent` 是**扩展工具**，不在 `BUILTIN_TOOLS` 里；不声明时父代理的工具集里根本没有它。

```rust
// agent 调用示例：
// tool: spawn_sub_agent
// args: { task: "找出所有调用 login() 的位置，给出文件:行 清单" }
```

**子代理约束：**
- **工具集固定只读**：只有 `read_file` / `list_dir`。无 `run_command`（本系统无 OS 级沙箱，决策 19 修订 / 104）、无写文件、无 `submit_metadata`
- **不继承阶段声明的工具**：阶段配置无法给子代理扩权
- 子代理继承父代理的工作目录（worktree + 任务目录），不额外隔离
- 子代理的 token 记在**自己那一行** run 上并计入任务 `total_tokens`；父 run **不重复累加**
- 子代理不允许再派生子代理（最多一层，决策 9）——它的工具集里没有 `spawn_sub_agent`
- 子代理**各自占一行** `kanban_node_runs`（`agent_type = "subagent"`、`parent_run_id` 指向父 run），对应自己那一行 `kanban_node_conversations`，不与父会话混进同一个 `messages_json`（决策 77）
- 超时沿用节点级 `node_max_duration_sec`；子代理运行期间心跳刷**父 run**（决策 88 同源做法），父节点不会被空闲超时误杀

> **与 L4 兜底无关（决策 154 的边界不变）**：子代理是**技能可调用的能力**，不是上下文超限兜底手段。§12.13.3 的「分批 / 拆子代理」仍未实现，压缩后仍超硬限一律挂 `pending(context_overflow)`。

### 12.9 远程仓库关联

kanban 项目以**本地仓库路径**为唯一事实来源（决策 29）。不引入远程 URL 概念，去除 GitHub 依赖；本地仓库是否配置了 remote 不影响项目模型。

```typescript
interface KanbanProject {
  id: string;
  name: string;
  local_path: string;           // 本地仓库路径（唯一事实来源）
  default_branch: string;       // 默认 "main"，用于 worktree 基准与 merge 目标
  language: string;             // 检测到的编程语言（rust/python/typescript 等）
  test_framework: string;       // 检测到的测试框架（cargo_test/pytest/npm_test 等）
  lint_command?: string;        // 可选静态检查命令（决策 139），探测候选、用户确认时预填
  agents_md_path?: string;      // AGENTS.md 路径（如果存在）
  created_at: string;
}
```

**项目创建流程（决策 24 / 48）：**

```
用户输入本地仓库路径
  → 立即校验是否为 git 仓库（否 → 拒绝创建，决策 61）
  → **确定性探测（代码，不调 LLM）**：
      (1) 检测语言（Cargo.toml → Rust，package.json → Node，pyproject.toml → Python）
      (2) 检测测试框架（cargo test / pytest / npm test）
      (3) 检测 lint 工具（clippy.toml / ruff.toml / eslint 配置等 → 预填 lint_command，决策 139）
      (4) 检测 AGENTS.md 是否存在
      (5) 检测默认分支名（git symbolic-ref refs/remotes/origin/HEAD，无 remote 时取当前分支）
      (6) 检测目录结构（src/、lib/、tests/）
      (7) 检测 .gitignore
  → POST /projects/analyze 触发 project_analysis 伪阶段，agent 基于上述事实清单生成
    人读的分析摘要并标注可疑项（如"检测到多套测试框架，请确认"）
  → 展示分析结果到前端
  → 用户确认保存或修改
  → 存入 kanban_projects 表
```

> **职责划分（决策 78，探测清单经决策 139 扩为七项）：** 上述探测全部是确定性判断，按 G7 用代码实现——更省钱、可单测、结果稳定。`project_analysis` 伪阶段（决策 48）保留，但 agent 的职责收窄为"基于事实清单写摘要 + 标注可疑项"；prompt 可省略（省略时只展示事实清单）。

**任务创建流程：**

```
用户创建任务（指定 project_id）
  → POST /tasks 在 API 层创建 Task 记录**与单条 main 游标**（决策 42 / 90），
     判定 status = queued（无依赖）/ waiting（有依赖）
  → KanbanScheduler.admit_pending_tasks 按 max_concurrent_tasks 准入 → status = running（决策 98）
  → init.execute: git worktree add ~/.agentpipeline/worktrees/{task_id} \
        -b kanban/{task_id} {project.local_path}#{project.default_branch}
  （worktree 从项目本地仓库的 default_branch 创建；有 remote 时基准为 origin/{default_branch}）
```

> **准入先于 worktree（决策 98）：** 建 worktree 在 `init.execute` 内，而 `init.execute` 只在任务被 scheduler 准入后才跑。因此 `max_concurrent_tasks` 同时约束了"同时执行的任务数"与"同时存在的 worktree 数"。

### 12.10 SQLite 并发与锁

kanban 的 checkpoint、任务状态、会话内容都写同一个 SQLite（`~/.agentpipeline/data/agentpipeline.db`），并发写入容易触发 `database is locked`。

**措施：**

| 措施 | 说明 |
|---|---|
| WAL 模式 | `PRAGMA journal_mode=WAL`，读写不互相阻塞 |
| busy_timeout | `PRAGMA busy_timeout=5000`，锁等待 5s 而非立即失败 |
| 写操作串行化 | 进程内用 `tokio::sync::Mutex` 包住所有写操作，避免连接级竞争 |
| 短事务 | 事务内不做 LLM 调用/IO 等待，只做纯 DB 操作 |
| 单连接复用 | 复用 sqlx 连接池管理的共享连接，不每处新建 |
| 批量写入 | 会话 messages 等大字段攒批写入，减少写次数 |
| 定期清理 | 过期会话数据定期删除，控制库体积 |

```rust
// crates/core/src/storage/mod.rs

use tokio::sync::Mutex;
use std::sync::Arc;

// 写操作统一入口
pub struct DbWriter {
    lock: Arc<Mutex<()>>,
}

impl DbWriter {
    pub async fn write<F, R>(&self, f: F) -> Result<R>
    where
        F: std::future::Future<Output = Result<R>>,
    {
        let _guard = self.lock.lock().await;
        f.await
    }
}
```

### 12.11 前端交互设计

**布局：** 看板视图与对话视图**框架完全独立**，不复用界面结构（导航、布局、面板）。kanban 是任务视图（卡片 + 流水线），对话是单会话视图，两者信息密度和交互模式差异大。

> **v1 只发布看板视图（决策 79）。** 对话式 agent 已延后到 v2（决策 50，附录 B.2），因此 v1 不存在"主对话窗口"。下面这张表的左右两列描述的是**组件复用关系**：把 Markdown / diff / tool 调用渲染抽成公共前端组件，供 v1 的看板会话查看器（§12.4.3）使用，v2 的对话窗口将复用同一套组件。
>
> **前端规格延伸：** 交互与视觉的落地规格见 [design/frontend-design.md](../design/frontend-design.md)（夜间调度台：token 体系 / PipelineRail 三变奏 / SSE 归约表）；其 §10 与主文档的契约差距由决策 123 / 124 排掉。

但**渲染组件可复用**：kanban 的节点会话查看器（§12.4.3）复用公共消息渲染组件——

| 复用 | 不复用 |
|---|---|
| Markdown 渲染 | 布局框架 |
| 消息气泡样式 | 导航/侧边栏 |
| tool 调用展示组件 | 会话列表逻辑（kanban 按 stage/node 组织，对话按时间组织） |
| 代码高亮 / diff 渲染 | 输入框（看板无自由输入；唯一例外是 `info_insufficient` 卡片上的补充说明框，走 `ResumeRequest.input`） |

即：**"外壳独立，渲染件复用"**。

**流式输出：** kanban 各节点的 agent 执行过程实时流式输出到界面（统一走 `GET /tasks/{id}/stream`，决策 76）。权衡实时性与性能：

| 内容 | 是否流式 | 说明 |
|---|---|---|
| agent 文本输出 | ✅ 实时流式 | 用户可看到 agent "在想什么" |
| tool 调用与参数 | ✅ 实时 | 显示工具名 + 参数摘要 |
| 命令执行输出 | ⚠️ 分级流式 | 错误行实时全推，正常输出采样推送；完整内容落库（§12.4.4） |
| 系统命令（git / 测试） | ⚠️ 分级流式 | 无 agent 阶段同样展示，与 agent 命令统一到同一日志（§12.4.4） |
| 产出文件写入 | ⚠️ 摘要 | 只推"已写入 xxx（N 行）"，不推全文 |
| token 计数 | ✅ 实时累加 | 成本可视 |

**长耗时按钮异步：** 涉及长耗时操作的按钮（启动任务、重试、人工评审提交）改为异步：点击后立即返回、按钮进入 loading 态并禁用（防重复点击），结果通过 SSE 推送。

**任务卡片：** 禁止拖动。任务流转由流水线驱动，不做手动排序，避免拖拽与状态机冲突。

### 12.12 结构化输出强制与兼容

**强制输出（provider 支持时）：** 调用时按官方文档设置 JSON 输出参数：

```rust
// crates/core/src/agent/provider.rs

let response = agent
    .model("deepseek-chat")
    .system_prompt(&system_prompt)
    .tools(tools)
    .response_format(ResponseFormat::JsonObject)  // 或 JsonSchema
    .call(&user_prompt)
    .await?;
```

具体参数各 provider 不同，需在 provider 适配层中处理。

**兼容解析（三级降级）：**

```
1. 优先：从 tool_calls 中提取 submit_metadata 的参数（最可靠）
   ↓ 失败
2. 次选：从 assistant 文本中正则提取 JSON 块（```json ... ``` 或 {...}）
   ↓ 失败
3. 兜底：把校验错误回填给 agent，要求重新输出（计入重试）
   ↓ 超过 agent_retry_max
4. pending(retry_exhausted)，用户决定继续重试还是下一步
```

**解析实现：**

```rust
// crates/core/src/agent/metadata.rs

pub fn extract_metadata(response: &AgentResponse) -> Result<(Option<serde_json::Value>, Option<String>)> {
    // 1. 从 tool_calls 中提取 submit_metadata 的参数（最可靠）
    for tc in &response.tool_calls {
        if tc.function.name == "submit_metadata" {
            match serde_json::from_str(&tc.function.arguments) {
                Ok(value) => return Ok((Some(value), None)),
                Err(e) => return Ok((None, Some(format!("工具参数 JSON 解析失败：{}", e)))),
            }
        }
    }

    // 2. 从 assistant 文本中正则提取 JSON 块（容错）
    if let Some(content) = &response.content {
        let re = regex::Regex::new(r"```(?:json)?\s*(\{.*?\})\s*```").unwrap();
        if let Some(caps) = re.captures(content) {
            if let Ok(value) = serde_json::from_str(&caps[1]) {
                return Ok((Some(value), None));
            }
        }

        // 3. 最后一个平衡的 {...}
        if let Some(value) = find_last_balanced_json(content) {
            return Ok((Some(value), None));
        }
    }

    Ok((None, Some("未找到结构化元数据".to_string())))
}
```

**原则：** 不因"agent 多说了几句话"就直接失败——先尝试兼容解析；只有真的提取不到才重试。重试 3 次仍失败才进 pending 由用户决定。

### 12.13 上下文管理与压缩

#### 12.13.1 超限发生在哪

跨节点不会超限（§4 只传文件路径，不传内容），**唯一风险是单节点 agent loop 内**：多轮 tool 调用不断累积 messages。

| 节点 | 主要 context 消耗 | 风险等级 |
|---|---|---|
| architect-design.execute | 读少量现有代码 + 写 design.md | 低 |
| develop-design.execute | 读 design.md | 低 |
| test-design.execute | 读 design.md | 低 |
| **develop.execute** | 读 dev-plan.md + 读多个现有代码文件 + 写多个文件 | **高** |
| **review.execute** | 读全部变更文件 + 单元测试文件 | **高** |
| **test.execute** | 读测试场景 + 读变更代码 + 执行测试（长输出） | **最高** |
| 各 validate | 读单个产出文件 | 低 |

#### 12.13.2 核心前提

**文件系统是 source of truth，context 不需要长期持有内容。** 这是文件化设计（§4）带来的关键优势：任何被裁剪/丢弃的文件内容都能以极低成本重新读取，压缩因此是安全的。

#### 12.13.3 四级分层策略

按代价从低到高，逐级启用：

**L0 — 容量预估（调用前）**

```rust
// crates/core/src/agent/context.rs

pub fn estimate_context_capacity(model_window: usize, system_prompt: &str, user_prompt: &str, settings: &Settings) -> ContextCapacity {
    // 窗口大小取自解析后的 provider 行（`providers.context_window`，决策 46 / 111 / 110：
    // provider 表即注册表，前端可改）。无可用 provider（测试 FakeAgent / 纯代码场景）
    // 时跳过分档——**不臆造窗口**；provider 存在但 context_window 未登记（0）则显式失败，
    // 不静默取默认（决策 110）。
    let window = model_window;
    let reserved_system = count_tokens(system_prompt);
    let reserved_user = count_tokens(user_prompt);
    ContextCapacity {
        total: window,
        reserved_system,
        reserved_user,
        soft_limit: (window as f64 * settings.context_soft_limit_ratio) as usize,   // 60%
        hard_limit: (window as f64 * settings.context_hard_limit_ratio) as usize,   // 90%
        available_for_tools: window - reserved_system - reserved_user - OUTPUT_RESERVE,
    }
}
```

每次 loop 迭代前估算当前 messages 的 token 数，超过 `soft_limit` 就触发压缩。

**L1 — 工具结果裁剪（常开，零成本）**

工具返回时立即裁剪，不等到超限：

| 工具 | 裁剪策略 |
|---|---|
| `read_file` | 默认返回头部 200 行；超长文件只返回**结构大纲**（函数/类定义行）+ 头尾，提示 agent 可按 offset 分段读 |
| `run_command` | 保留前 50 行 + 后 100 行，中间的重复行折叠（`... 省略 N 行相同输出 ...`）；**错误行始终保留**（`grep -i error\|fail\|traceback`） |
| `list_dir` | 最多列出 200 项，超出折叠为目录摘要 |
| 其他 | 超过 `offload_threshold_tokens`（默认 4000）一律走 L2 卸载，context 只留预览 + 路径 |

**L2 — 大结果卸载到文件（超过 `offload_threshold_tokens`，默认 4000）**

`offload_threshold_tokens` 是**唯一的工具结果阈值**（决策 110）——原先并列的 `tool_result_max_tokens` 已合并进它，语义为"超过就落盘、context 只留预览"，避免出现"被截断但从未卸载、内容丢失"的中间区间。

不把大内容放进 context，只放摘要 + 路径：

```rust
// crates/core/src/agent/context.rs

pub async fn offload_tool_result(task_id: &str, tool_name: &str, content: &str) -> Result<String> {
    // 大结果落盘，返回替代文本
    let filename = format!("{:08x}", rand::random::<u32>());
    let path = format!("~/.agentpipeline/tasks/{}/.context/{}.txt", task_id, filename);
    write_file(&path, content).await?;
    let preview = head_tail_excerpt(content, 30, 30);
    let tokens = count_tokens(content);
    Ok(format!(
        "[工具 {} 输出过大，已卸载]\n完整内容：{}（{} token）\n预览：\n{}\n如需完整内容，用 read_file(\"{}\") 或分段读取。",
        tool_name, path, tokens, preview, path
    ))
}
```

典型场景：跑测试产生 5 万行日志 → context 只留"失败用例摘要 + 日志路径"。

**L3 — 对话压缩（超过 `soft_limit_ratio`）**

保留关键信息，把中间轮次压成摘要：

```
压缩前 messages：
  [system]（保留）
  [user 原始任务]（保留）
  round 1: assistant + read_file(a.py) 结果 8k token
  round 2: assistant + read_file(b.py) 结果 6k token
  round 3: assistant + run_command 结果 12k token
  round 4: assistant + write_file(c.py) 结果 100 token
  round 5: assistant + read_file(a.py) 结果 8k token   ← 重复读
  round 6-10: ...（保留最近 5 轮）
  round 11: assistant（最新）

压缩后：
  [system]（不变）
  [user 原始任务]（不变）
  [摘要] 已完成的操作：
         - 读取 a.py（路径/行数，内容可重读）
         - 读取 b.py
         - 执行 {test_command}，结果：3 failed, 12 passed（完整日志：{offload_path}）
         - 写入 c.py（已落盘）
         当前进度：正在修复 test_login 失败
  round 6-11（保留最近 5 轮完整）
```

**压缩规则（优先规则化，必要时才调 LLM）：**

| 内容类型 | 压缩方式 |
|---|---|
| 已 `write_file` 的内容 | **直接丢弃**，替换为"已写入 {path}"。文件在磁盘，无需保留 |
| 已 `read_file` 的内容 | 替换为"已读取 {path}（{n} 行）"，若后续需要 agent 会重读 |
| `run_command` 输出 | 保留退出码 + 错误摘要 + 卸载路径；成功的长输出折叠为一行摘要 |
| assistant 推理文本 | 保留最近 `keep_recent_rounds` 轮；更早的提取**关键决策**（一句话/条） |
| 错误与修正过程 | 保留最后一轮错误；更早的错误折叠为"曾遇到 X 错误，已通过 Y 解决" |
| tool_calls 序列 | 保留为简表（工具名 + 参数摘要 + 结果状态） |

**规则化压缩不调 LLM**，靠 messages 结构即可完成（我们知道每轮是 read/write/command）。只有 assistant 推理文本的浓缩需要 LLM，且只对更早的轮次做一次批量摘要调用（用便宜模型）。

**L4 — 兜底（超过 `hard_limit_ratio`）**

L3 后仍超限时的降级路径。下面是**设计阶梯（v2 三级）**，v1 只落最后一级与已被 L3 覆盖的那次压缩：

```
1. 强制压缩：keep_recent_rounds 降为 2，丢弃所有非必要内容   ← v1 未实现
   ↓ 仍超限
2. 按节点类型处理：                                        ← v1 不做（决策 154）
   - test.execute：分批执行测试（按测试文件分组，每组独立 loop）
   - review.execute：分批评审变更文件（按文件分组）
   - develop.execute：拆分为子任务
   ↓ 仍超限
3. pending(context_overflow)，用户决定：拆分任务 / 换长上下文模型 / 终止
```

> **v1 实现：只有两级（决策 154）**——上面这个阶梯是**设计台阶**，v1 的 L4 收口为
> 「按轮压缩 → `pending(context_overflow)`」，压缩就是 §12.13.3 的 L3 那一次
> （`compact_messages_from`，`keep_recent_rounds` 走设置值），压缩后仍超硬限即挂 pending
> 交用户处置，用户动作集为「拆分任务 / 换模型 / 取消」（决策 105）。**第 1、2 级在 v1 都没有实现**：
> 第 2 级（分批 / 拆子代理）由决策 154 明确不做；第 1 级的 `force_keep_recent_rounds = 2`
> 从来没有消费者，已随决策 154① 的死代码清理（`L4Plan` / `L4Action` / `plan_l4` /
> `l4_pending_kind` 整组）一并删除——**故不要在实现里找它**。
> 重开条件可核对：生产库出现真实 `context_overflow` 落库**且**用户以现有三动作处理后任务仍无法推进到终态（决策 154）。
>
> **L4 只此一条出口**：压缩后仍超限时**一律**挂 `pending(context_overflow)`——即使阶段配置声明了
> `spawn_sub_agent` 也走这条（决策 154「L4 兜底只有两级」的原裁决，决策 172③ 未改它）。
>
> **票 08 落地的子代理不是这一级兜底**：它给的是「父代理主动派一个只读检索子代理、只收摘要」这项能力（§12.8），由模型在对话中调用，与 L4 的自动降级路径**互不相干**；它也不在 executor 的判定里出现（那里已不做任何「声明了子代理吗」的探测）。

**闸门复检注入的体积上界（票 09）：** 复检段读闸门命令的完整日志（`gate-output-{stage}.log`）；注入上限 120k 字符，超限按「首 ⅔ + 尾 ⅓」截断并在正文里写明省略字符数与完整日志路径（**不静默回退到首尾预览**）。

#### 12.13.4 各节点的上下文策略

| 节点 | 策略 |
|---|---|
| develop.execute | 分批写文件；写完一个文件立即从 context 丢弃内容（L2/L3 规则） |
| review.execute | 变更文件多时**分批评审**：每批 3-5 个文件，每批产出独立的评审片段，最后合并成 review-report.md |
| test.execute | 测试输出强制卸载（L2）；测试文件多时按文件分组分批执行，汇总结果 |
| 各 validate | 只读单个产出文件 + 摘要，无需特殊处理 |

#### 12.13.5 与 Prompt Cache 的关系

压缩会改变 messages 前缀，导致 prompt cache 失效。因此：

| 策略 | 说明 |
|---|---|
| 压缩时机 | 只在超过 `soft_limit_ratio` 时触发，不频繁压缩 |
| system prompt 不变 | 压缩只动中间轮次，system 保持稳定 → system 缓存仍命中 |
| 压缩后重置缓存标记 | 压缩后下一轮的 cache 从压缩后的前缀重建 |
| 压缩频率监控 | 若单节点压缩超过 3 次，说明任务粒度太粗，记录告警 |

#### 12.13.6 监控

| 指标 | 用途 |
|---|---|
| 单节点峰值 context token | 找出最容易超限的节点 |
| 压缩触发次数/节点 | 压缩频繁说明 prompt 或任务粒度有问题 |
| 卸载文件数量/节点 | 卸载过多说明工具输出过大，需优化裁剪 |
| `context_overflow` pending 次数 | 兜底触发频率，高频说明需调整分层阈值 |
| 压缩后任务完成率 | 验证压缩是否丢失了关键信息 |

**告警阈值（只告警不强制，决策 66）：**

```python
CONTEXT_ALERTS = {
    "compress_count_per_node": 3,        # 单节点压缩超 3 次告警
    "peak_context_ratio": 0.85,          # 峰值占窗口 85% 告警
    "overflow_pending_rate": 0.05,       # overflow pending 率超 5% 告警
}
```

**自适应超时估算（决策 66）：** 从 `kanban_node_runs` 统计每个 `(stage, node)` 的 P50/P90 耗时，**只取成功运行**（排除 failed/timeout，避免失败样本污染阈值）。采样口径（票 17 落地）：窗口取该节点**最近 20 次成功运行**，样本数 < 5 时（冷启动）不展示也不告警。用途限于进度展示与告警：超过该节点 **3×P90** 时记一条告警（`tick` 报告 `slow_run_alerts` + `tracing::warn`）。**强制超时阈值始终取配置值**（`effective_idle_timeout` / `effective_max_duration`），自适应分位数绝不作为超时判定输入——挂死节点会自我抬高阈值（附录 B.4 的反馈回路）。`adaptive_timeout_enabled = false`（缺省）时完全关闭该估算，零行为变化。

### 12.14 本地数据保护

本系统把 provider 密钥、任务产出、完整会话与命令日志都存在本机同一目录下。v1 **不做系统级沙箱**（决策 19 已修订），因此这里的目标是**限制静态数据的暴露面，并使残余风险显式可见**，而不是假装阻止本机进程。

**文件权限要求：**

| 路径 | 权限 | 说明 |
|---|---|---|
| `~/.agentpipeline/` | `0700` | 仅属主可进入 |
| `~/.agentpipeline/data/agentpipeline.db` | `0600` | 含 provider 明文密钥（决策 112） |
| `~/.agentpipeline/data/`（含 `-wal` / `-shm`） | `0700` | WAL 文件同样含密钥明文 |
| `~/.agentpipeline/tasks/{task_id}/` | `0700` | 任务产出与会话卸载文件 |
| `~/.agentpipeline/logs/` | `0700` | 日志 |

**启动时校验：** 检查上述路径权限；**过宽则告警，不阻断启动**（避免把用户锁在门外）。告警写入日志并在前端顶部展示一条横幅，附一键修复入口。注意 macOS 上 `/tmp` → `/private/tmp`、`/etc` → `/private/etc` 是符号链接，权限与路径校验前都应 realpath 解析（决策 104）。

**为什么密钥是明文（决策 112，修订决策 10）：** 加密存储的前提是"密钥比密文更难拿到"。但在**同一台机器、shell 不受限**的前提下，agent 可以直接读取数据库文件，也可以调用 `security find-generic-password` 之类的命令取出任何 OS keychain 条目——加密只是增加了一次可被同样执行的解密步骤，挡不住它本应防住的对手。这一判断有实测依据：ZCode 自身即把 provider 密钥明文存在 `~/.zcode/v2/config.json` 中，不做加密。因此本系统选择**明文 + 严格的目录权限 + 如实告知**，而不是留在"看起来加密了"的中间状态。

**残余风险（明确接受，不视为遗漏）：**

| 风险 | 是否阻止 | 缓解 |
|---|---|---|
| agent 通过 `run_command` 读取 `agentpipeline.db` 拿走 provider 密钥 | **否** | 命令全量落 `kanban_node_commands` 可供事后审计；§12.4.4 的脱敏在回填 messages 之前执行（决策 118），密钥不进入日志、会话与 LLM 请求 |
| **同一条在对讲台同样成立**（决策 206 / 207，票 04；`logs/` 那一半由决策 226 修订）：值班长的域是 `home.root()`，文件工具按路径前缀拒 `data/`（`logs/` 的拒绝已撤——体量改由 `read_file` 的字节上限管，见决策 226），但**域检查对命令几乎无价值**——命令自己 `cd` 就出去了 | **否** | 分档：`ask` 档（值班长的缺省）下每个文件 / 命令动作都要值班经理按确认钮，`auto` 档直达。**`auto` 档这一条无补偿**——根本解是本节末尾那件事（OS 级沙箱，决策 19 修订 / 104 / 179 已登记，尚未落地）。审计侧照旧：命令落 `kanban_node_commands` 并按会话归属（决策 204④） |
| agent 跨任务读写其他任务的 worktree / 任务目录（违反 G11） | **否** | 同上，靠命令日志审计；文件工具层面受 `FileToolPolicy` 约束（`[pipeline] file_access_unrestricted = true` 时这半补偿也关掉，决策 283——那时只剩拒绝名单与命令审计） |
| 本机其他用户读取数据 | **是**（靠权限） | `0700` / `0600` + 启动校验告警 |
| 目录被整体复制 / 备份外泄 | **否** | 无。这是明文的固有代价，已在决策 112 显式接受 |

> **与 G11 / §9 的关系：** G11 已标注为"策略而非系统保证"（决策 104），本节的残余风险表与 §9 异常处理表中那一行是同一件事的两个视角——§9 从"执行期怎么发现"讲，本节从"静态数据怎么保护"讲。

**本机 API 的跨源防护（决策 128）：** server 只绑 `127.0.0.1`，但任意网页都能向 `http://127.0.0.1:{port}` 发起跨站 POST（HTML form / no-cors fetch 不受 CORS 响应检查约束），`merge/decision`（合入）、`/cancel`、`/resume` 等状态变更端点可被第三方页面驱动。防护（axum 中间件）：所有**写请求**（非 GET/HEAD）必须满足以下之一，否则 403——① 携带自定义头 `X-AgentPipeline: 1`（跨站 form 无法携带自定义头）；② `Origin` / `Referer` 缺失（非浏览器客户端）；③ `Origin` / `Referer` 等于 `http://127.0.0.1:{port}`。SSE 为纯 GET，不受影响。这是与决策 104 / 112 同一威胁模型（"本机、浏览器在场"）的配套防线。**本机之外的第二道防线是配对令牌**（决策 182⑦）——跨源防护管「谁在说话」，配对令牌管「这台设备有没有凭据」，两者正交，见 §12.16。

### 12.15 工具层出口控制与它的残余风险

技能市场（决策 172⑤；来源侧由**决策 194** 换成 GitHub 仓）把「**下载来的技能** + agent 有无限 shell」这一组合带进了威胁模型（仓名单可在「设置 · 技能市场」页上改并**当场生效**，决策 187；那份盖过 `config.toml` 的 `[market] github_repos`，清掉即回落——两处共用同一个校验函数，放行一个 `owner/repo` 等于允许从它下载引导 agent 的正文）：本地导入的技能来自用户自己的机器，不构成同一类风险；从 GitHub 仓装下来的技能是**第三方的正文**，而它可以在运行时引导 agent 去调 `run_command`。四家主流 agent（Claude Code / Codex / Cursor / Copilot）都用**网络出口控制**兜这一层，本仓此前**零出口控制**。票 12（决策 179）补的就是这一层。

> **这一层管的是 agent 经 `run_command` 发起的出口；技能来源仓自己的出网是另一个出口**（服务进程走 libgit2 git 通道，见下面「技能来源仓的出网」），两者不共用配置：前者的放行面是 `egress_allow_hosts`，后者的放行面是**仓名单**。

**技能来源仓的出网（决策 194）：** 下载引导 agent 正文的是**服务进程自己**（libgit2 git 通道），不经 `run_command`，故不受 `egress_allow_hosts` 约束——它由来源侧的固定口径承担，与决策 177②③ 的旧承担者是同两条裁决：

- **出网目标只有 `https://github.com`。** URL 由程序构造，形态唯一：`{base}/{owner}/{repo}.git`，`base` 默认 `https://github.com`。**不存在「用户填 origin」这回事**——用户填的是 `owner/repo`，主机由我们定。这不是洁癖：libgit2 的传输注册表里 `git://` / `http://` / `https://` / `file://` / `ssh://` 全在，**裸文件系统路径也会被 local transport 吃掉**，所以那个字符串不能直接当 URL。决策 177③ 的「非回环必须 https」由此被「URL 只能由我们拼」直接满足；
- **不跟随跨站重定向**（决策 177② 的承担者换成 git 通道）：libgit2 的 `RemoteRedirect::None` **必须显式设**——`FetchOptions::new()` 的默认是 `Initial`（跟初始请求的跨站重定向），靠默认值会当场破掉这条口径。**它的真实语义是「不跟跨站重定向」**：libgit2 对**同站 http→https 升级**仍然放行（`src/util/net.c` 里只在目标 scheme 不是 https 时才拒跨 scheme 跳转，host 检查被 `allow_offsite` 关掉）。我们只走 https，故这条残余**不可达**——写清是为了不让后人以为 `None` 密不透风；
- **明文 http 只对回环放行**（与决策 177③ 同一条规则），用于本机 fixture：环境变量 `AGENTPIPELINE_MARKET_GIT_BASE` 是**测试接缝**（与 `AGENTPIPELINE_HOME` 同族，决策 143 姿态），取值受同一条规则约束——回环 http 或任意 https，**不得带路径 / 查询 / 片段**；不设或非法则回落默认。这不是放宽：**放行的仍是 `owner/repo`，主机仍由我们定**，只是那个主机在测试里可以指回本机回环，而回环本来就在 177③ 的放行之列；
- **下载体积上限 64 MiB 由流式回调在传输中守**：`Progress::received_bytes()` 累加，超限 `return false` 中断（实测回调返回 `false` 会中止并报 `indexer progress callback returned -1`）。**粒度是读块（最小约 64 KB）——它是「边收边判」而不是下载前的门**（下载前的门在这里不存在：`content-length` 不可靠、`HEAD` 也不返回）。上限与本地导入端点的 `DefaultBodyLimit` 同值，使两条路对内存的消耗同量级；超限报文用**我们自己记的那份已收字节数**（中止错误串本身可能什么都没有：`class=None code=User msg=no error`）；
- **私有仓不做**：无凭据入口，界面上也不放 token 输入框；报错要说清「也可能是无权访问」，不让用户把无权限误读成仓名拼错。日后要做是**纯增量**：`FetchOptions::custom_headers` 能逐字转发 `Authorization`（实测在 `info/refs` 与 `git-upload-pack` 两跳都到了服务端），且凭据可只从环境变量读而不落盘——**不触决策 112 那条「provider 密钥目前明文存储」**。这是「日后」不是「已支持」。

**形态：命令级特征识别，不是网络层拦截。** 按 shell 分隔符切段后只看每段的首个命令词（跳过 `sudo` / `VAR=x` 包装，以及选项**及其取值**——不跳取值时 `git -C /tmp/repo push origin` 的首个非选项 token 是 `/tmp`，会把一条出口判成本地命令），识别五类出口形态：`git` 的网络子命令、包管理器的安装 / 发布子命令、解释器 + URL 字面量、取 URL 的二进制（`curl` / `wget`）、目标写作 `[user@]host[:path]` 的二进制（`ssh` / `scp` / `rsync` / `nc` …）。判定在 `spawn_in_own_process_group` **之前**——被拒的命令根本不执行。

**配置与默认姿态：**

| 配置（`[pipeline]`） | 默认 | 语义 |
|---|---|---|
| `egress_allow_hosts` | `[]` | 放行的目标主机：精确主机 / `*.example.com`（子域通配，落在点边界上）/ `*` |
| `egress_allow_all` | `false` | 显式放行全部出口 |

**默认只放行回环**（`localhost` / 127.0.0.0/8 的 IP 字面量 / `::1`——按决策 246 的规范形态判定，前缀伪装如 `127.evil.test` 不算；与上面「技能来源仓的出网」里那条「明文 http 只对回环放行」同源：流量不出本机，中间人不在威胁模型里）。未配置**不会**静默变成「全部放行」：忘配的代价是某条命令被拒并报出怎么放行（用户立刻发现），配宽的代价是静默放行陌生目标——两个方向的代价不对称，故取保守侧。判不出目标主机的形态（如 `git push origin`）按**拒绝**处理，错的方向是多拦。

**拒绝是可归因 + 可审计的：** 报错说清三件事（拒了什么、怎么放行、这层不是安全边界），且被拒的调用落 `kanban_node_commands`（与放行的命令**同表**，`exit_code = 1`、`stderr_preview` 写拒绝原因）——审计面必须看得见「有过一次被拒的出口尝试」，否则策略只是一次静默失败。

**残余风险（明确接受，不视为遗漏）：**

| 风险 | 是否阻止 | 说明 |
|---|---|---|
| 被启动的子进程**后续自行联网** | **否** | `python -c "import socket; …"` 手搓 TCP、`make` 目标里藏的 `curl`、静态链接的二进制自己发请求——全在本层的视野之外。**这是本层最根本的局限。** |
| 变量拼接 / `base64 -d \| sh` / 脚本文件里的命令 / shell 函数与别名 | **否** | 正则追不上变形，且追的过程会误伤合法命令（与票 11「不做正文正则安全扫描」同一条裁决）。**不追**。 |
| 已放行主机被当作跳板 | **否** | 白名单是主机粒度，不区分路径与用途 |
| 直白的 exfiltrate 指令（`curl -d @.env https://…`、`git push`、`npm publish`） | **是** | 这正是本层的真实价值所在 |

> **票 10 起这条的份量变了（决策 210③ / 212）**：值班长的**修复轮**把「经 `run_command` 碰本仓」
> 从一条边角路径变成了**常规路径**——它要在一个修复 worktree 里改代码、跑闸门。文件策略对它
> 写不了项目工作区（那是 worktree 存在的理由），但命令不受文件策略管，这一点在修复轮里
> 与在别处**一样**是「不可控的补偿」。区别只在**可见性**：修复轮的每一步（改动 → 闸门 →
> commit）都落账（`kanban_node_commands` + 带 `[repair]` 标记的 commit），而即兴的一次
> heredoc 不落。**「有账」不等于「有边界」**，两句话都得说。

**修复分支会积压，这是有意接受的（决策 212③ / 票 12）：** 修复提议**不设 TTL**（人有意留到
第二天早上看），被拒 / 过期时**保留分支、只删 worktree**——分支是唯一的证据。故
`git branch --list 'repair/*'` 会随时间变长。清理口径与对讲台其余各表同一处：
`conversation_retention_days`（默认 30 天）的年龄清理扫提议表，**但扫提议不等于删分支**
（分支在 git 里，不在库里）——目前**没有**自动删分支的路，要清就手动 `git branch -D`。
这条如实记着：分支积压是可接受的代价，而「谁在什么时候清」暂时是人。

**`GIT_OP_TIMEOUT_SEC` / `IS_DIRTY_TIMEOUT_SEC` 的语义（决策 209 附注 61 / 票 10）：**
`blocking_within` 超时**不会终止那个阻塞线程**——它丢弃 `JoinHandle`、任务 detach 后继续跑，
拿的是「关键路径能继续」而不是「那个调用停了」。更要紧的一半：`init_worktree` 的闭包在
`with_worktree_lock` 里跑，**超时后那个线程仍持有该仓库的分桶锁**，此后同一仓库的每一次
建 worktree 都会阻塞（直到那个线程真的醒来）。这是「真取消尚未落地」的残余风险——
修复轮（票 10）也走同一条路，故它的份量同样被本批放大。

> **结论（票 12 的硬要求）：** 本项**不是安全边界**，只是一道「让直白动作可见、可拦」的闸。与决策 19（修订）/ 104 的关系：**不引入 OS 级沙箱**，只加工具层策略。文档与报错都必须守住这句话——「不是安全边界」本身是设计的一部分，不是免责声明。

**OS 级沙箱（Seatbelt / bubblewrap）登记为后续决策的正式候选。** 真正的默认拒绝要求接管所有子进程的出站连接与文件系统访问（macOS 的 `sandbox-exec` / Linux 的 bubblewrap + 网络代理），四家主流 agent 都是那么做的；本仓**结构上没有这一层**（决策 19 修订 / 104 有意接受）。要做的话它是一个**新的决策**，不是本票的补充——它同时会改变 `run_command` 的语义、worktree 的可见范围与本地开发的便利性，代价与收益都需要单独权衡。它也是本层所有残余风险的唯一根本解。

> **这也是值班长那一条（§12.14 残余风险表第二行）的出口**（决策 206 / 207）：`auto` 档下经命令读走密钥库**无补偿**，因为文件策略管不了命令、而 `cwd` 对命令几乎无约束。分档（缺省 `ask` = 每次按键）只是让这件事**可拦、可见**，不是边界——与本节「不是安全边界」是同一句话。

> 措辞纪律：四家产品的公开文档都在强调同一件事——Claude Code 写「no system is completely immune」、Cursor 写「Auto-review is not a security boundary」。本节的措辞与它们一致，避免制造虚假安全感。

### 12.16 配对令牌与局域网态势

**威胁模型。** 决策 167 把服务开到局域网（`--host 0.0.0.0`，桌面壳为 `AGENTPIPELINE_LAN=1`，或**在「手机访问」页按下「绑定全网卡」**——决策 186）之后，同网段的任何设备都能调本服务的接口。看板时代的代价是「别人点了你的 resume」，而**对讲台时代的代价是没有上界的花费**——写请求能触发真实 LLM 调用，且对讲台本身就是一条「随便问、按 token 计费」的通道。决策 182⑦ 因此补上配对令牌，这正是决策 167 明确推迟的那件事。

**口径一句话（**由决策 336 修订**）：未配对的设备**什么页面都拿不到**——包括入口页与静态资产。** 原口径是「看的随便看，动手和花钱要凭据」，它是写给**可信局域网**的；服务直接挂公网（裸 IP + 应用自己终止 TLS，见 §12.17）之后，「读」本身就是最要紧的那件事，而自签证书只拦浏览器、拦不住扫描器。

| 请求 | 局域网 / 公网形态（绑非回环地址） | 回环形态（默认） |
|---|---|---|
| **导航**（`Accept: text/html` 或 `Sec-Fetch-Mode: navigate`）未带凭据 | **401 + 一张自带样式的配对页**（`crates/app/src/pairing_page.html`） | 照常 |
| 入口页 `/`、`/assets/*`、`/sw.js`、`/manifest.webmanifest`、字体与图标**（未带凭据）** | **403**（与接口同一形状）——外壳也在闸门后面 | 不要求 |
| 只读 GET（看板 / 任务 / 会话 / 命令 / 指标 / 分享页）**（未带凭据）** | **403** + `kind: pairing_required` | 不要求 |
| 写请求（`POST` / `PUT` / `PATCH` / `DELETE`）与全部 `/foreman/*`（**含 GET**） | 同上（同一个令牌，不再按方法分档） | 不要求 |
| 来自**本机**（回环来源）的任何请求 | **一律豁免** | — |

**凭据的三种载法**（同一个令牌，任一带对即放行）：

| 载法 | 谁在用 | 备注 |
|---|---|---|
| 地址里的 `?pair={token}` | 二维码 / 主屏图标 / 手输粘贴 | **唯一能把令牌带进一次导航**的通道。对上即放行（**不跳转**，见决策 191），并在响应上种 cookie |
| 请求头 `X-AgentPipeline-Token` | 脚本、桌面壳、契约测试、应用内的 fetch | 决策 182㉙ 的老路，一个字没动 |
| cookie `agentpipeline_pairing` | 浏览器自己 | **决策 336 新增**：导航请求带不了自定义头，而 `?pair=` 只在扫码那一次有，判「这台设备配过没有」得靠一样浏览器自己会带的东西。`HttpOnly; SameSite=Lax; Path=/; Max-Age=1y`——**没有 `Secure`**：这个进程只有一种传输形态，带上它反而会让明文部署**整页空白**（外壳的子资源只能靠这个 cookie 过闸门，而 `Secure` cookie 在 `http://` 上被浏览器拒收）。实测账见决策 336 |

**只在非回环绑定时生效**（`AppState::lan_mode()`）：默认本机形态零摩擦是硬约束，日常本机使用完全不受影响；只有绑到非回环地址才启用配对。判定写在 `stream.rs::pairing_guard` 里——① 非局域网形态直接放行；② 回环来源直接放行；③ 地址里的 `?pair=` 对上了 → 放行 + 种 cookie；④ 头或 cookie 对上了 → 放行；⑤ 都没对上 → 导航给配对页（**401**，不带 `WWW-Authenticate`，故浏览器不弹自己的密码框），其余给 **403**（报文「这台设备还没配对：请在跑服务的电脑本机打开手机访问页扫码」，措辞见决策 189）。**回环豁免是有意的**：局域网形态下本机浏览器与 CLI 仍在用这台服务，要求它们配对等于把本机也变成需配对的设备；它同时是「令牌泄露后还能从本机复位」的前提。读取令牌失败时**不放行**（fail closed）——拿不到用来比对的令牌就无从证明请求有权。

**令牌本身。**

| 项 | 行为 |
|---|---|
| 生成与存储 | 服务端生成，持久化在 `kanban_pairing_token` 单行表（`id INTEGER PRIMARY KEY CHECK (id = 1)`——「有且只有一枚」由 schema 强制，不靠代码约定）。两枚 ULID 拼接（约 160 bit 随机位），52 个 Crockford base32 字符，URL 安全字符，可直接落在查询串里无需转义 |
| 生命周期 | **一次生成、长期有效**——不随启动重生成（每开一次服务就要在手机上重扫一次，摩擦会高到使用者干脆关掉它）；只由重置决定 |
| 传递 | 三种载法（见上表：地址 `?pair=` / 请求头 / cookie），同一个令牌 |
| `GET /pairing/token` | **仅回环可读**（局域网来源 403）。判定按**来源地址**而非绑定形态——绑 `0.0.0.0` 时从本机发出的请求仍是回环地址，这正是「用本机把令牌递给手机」的组合。若局域网客户端也能读，令牌就不再是「已配对设备的凭据」而成了任何人可取的公开值，守卫也就没有意义了 |
| `POST /pairing/reset` | 重生成并返回新值，**旧令牌立即失效**。它在局域网形态下是写请求（已被守卫拦，需持有令牌或来自本机），**回环来源豁免**——令牌丢失或怀疑泄露时必须还能从这台机器上复位，否则唯一出路是删数据库文件 |
| 配对 URL | `{base}/?pair={token}`（`server_info.rs::pairing_url()` 是唯一事实源——参数名与分隔符各写各的就会漂移） |

**怎么开启局域网访问（决策 186）。** 三条路，优先级从高到低：

| 方式 | 生效时机 | 谁说了算 |
|---|---|---|
| `--host 0.0.0.0` / 桌面壳 `AGENTPIPELINE_LAN=1` | 启动时 | 启动参数（界面改得动**这一次**，重启后仍以它为准） |
| **「手机访问」页上的「绑定全网卡」钮**（`POST /server/lan`） | **当场**（不必重启） | 界面（住 `kanban_server_bind` 单行表，重启仍生效） |
| `config.toml` 的 `[server] host` | 启动时 | 配置文件（界面上保存过之后不再生效，页面上有「改回 config.toml」的入口） |

`GET /server-info` 的 `bind_source`（`startup` / `settings` / `config`）就是上表第三列的机器可读形式，界面据此告诉用户「这颗钮按了重启还算不算数」。**改绑只允许回环来源发起**（局域网来源 403）——这是全站唯一能把服务暴露到局域网的入口，若局域网设备也能调，配对令牌就白设了（先把它打开，再从自己的机器上来）。改绑由 `serve` 里的**监听器主管**执行：停旧（最多 500ms 优雅窗口，超过就强制断开——SSE 流在客户端断开前永不结束，只看优雅停机按钮会永远转圈）→ 绑新（**端口不变**，否则刚扫的码失效）→ 起新；绑不上就回滚到旧地址。**触发改绑的那次请求本身走在被切断的连接上**，故它的应答不保证到达——前端的判定以「重读 `/server-info`」为准，`lib/lanToggle.ts` 把「传输失败」与「真的失败」分开。


**二维码端点的校验随之放宽**：从「URL 与地址白名单精确匹配」改为「**origin 落在白名单内，允许追加 query / path**」，否则带 `?pair=` 的配对 URL 会被 400 挡掉。**这不构成泄露**：该端点既不生成也不返回令牌，调用方得先持有令牌才构造得出带令牌的 URL，而它仍然拒绝把二维码渲染成任意外站地址（异 origin 与前缀伪装照旧 400）。

**现状：链路已完整打通（2026-09-15，票 07 收尾补记；**2026-09-29 由决策 336 扩面**）。** **服务端**：缺令牌 403 / 带令牌通过 / 回环豁免 / **未配对时导航只得到配对页（原先的「只读 GET 不护」已废）** / 读取口仅回环 / 重置使旧令牌失效 / 缺省回环绑定不要求令牌；配对链接（`?pair=`）与 cookie 两条通道的契约用例见 `api_contract.rs::pairing_lan_*`。**前端**（票 07 的前端半边，与决策 182⑦ 同步落地）：分享页调 `GET /pairing/token` 并用 `pairing_url()` 同款约定把令牌拼进二维码地址（**取不到令牌时不画码**，见下一段），另有一键重置入口；`main.ts` 在任何请求之前调 `capturePairingFromLocation()` —— 读 `?pair=` → 存 localStorage（**参数留在地址栏里不抹掉**，见下方决策 191 那一段——手机的主屏图标与书签要靠它每次启动重新递进来）；此后 `api/client.ts` 与 `realtime/connection.ts` 都给请求带上 `X-AgentPipeline-Token`（**含只读 GET**）。403 会在对讲台上渲染成「这台设备还没配对」并给出通往手机访问页的入口（读会话与发话两条失败路径都有）——决策 336 之后这条应用内路径只剩「设备已配对但凭据被重置」这种情形（未配对设备根本加载不到应用，见到的是配对页）。**回环形态不受任何影响**：未配对时不带这个头，本机使用零摩擦。

**「取不到令牌就不画码」（决策 189，2026-09-16）。** 配对令牌只允许**回环来源**读取（决策 182㉗），于是从手机打开「手机访问」页、或在电脑上用**局域网地址**打开这一页，**必然**读不到它。此前的降级是「照画一张裸地址的码」，而那张码与正常的那张**在视觉上毫无区别**：扫了它，看板照常打开（当时只护写请求），一动写操作或进对讲台就撞 403「这台设备还没配对」，报错页的指引（原文「在已配对的设备上重扫一次二维码」）又把人送回这一页——使用者会把力气花在重复扫码上而不是去找那台电脑，而且这条路径**没有出口**：手机上打开这一页永远读不到令牌。（决策 336 之后那张裸码更早失效——扫它落在一张配对页上——但「一张扫不出结果的码比没有码更坏」这条判定不变。）现在的行为：① **有地址但没有令牌 → 不画码**，改给「去那台跑服务的电脑上打开 `http://127.0.0.1:{port}/#/share`」的指引块；判定在 `frontend/src/lib/sharePairing.ts::sharePanel`（纯函数，四种形态：只绑回环 / 枚举不出地址 / 有地址无令牌 / 带令牌的码），模板只按它分派。② **对讲台两处配对指引改口径**：从「在已配对的设备上重扫一次」改为「配对码只在那台跑服务的电脑本机生成」（桌面应用窗口，或浏览器里的 `127.0.0.1`）。

**判据是来源地址，不是设备**——这一条最容易绕错，故写明：用局域网地址打开本页的**那台电脑自己**同样是非回环来源，在守卫眼里它也就是一台未配对设备（任何请求都带不出令牌；唯一放行它的是三种载法之一）。这一页对它给的是同一条指引；出路是改回 `127.0.0.1` 上的那个地址，或桌面应用的窗口。

**取不到令牌的三种来源分开说**（决策 189）：还在读 → 一句「正在读取配对令牌」（旧行为在这一瞬间画的正是那张没令牌的码）；403 → 不是从本机打开的（报文本身不动声色是对的，这是护栏在工作，不是故障）；其余故障 → 照实报出报文，并说「先刷新重试」。

**顶栏入口只在跑服务的这台机器本机上出现（决策 190）。** 手机上（以及用局域网地址打开的电脑上）顶栏**不给**「手机访问」这个入口：那一页对它们只剩指引（上一段），摆出来就是送人去白跑一趟。判据取「来源是否回环」而不是「视口宽度」——手机、平板、用局域网地址打开的电脑在守卫眼里是同一件事，而一个被拖窄到 380px 的桌面窗口仍是本机、那里的「手机访问」完全可用。判定在 `frontend/src/lib/localPage.ts`（**不发请求**，只看页面主机名；桌面壳注入的 `http://127.0.0.1:{port}` 优先于页面自身地址），与 `lib/lanToggle.ts` 共用同一份「什么算回环」（要求四段点分数字，`127.evil.com` 这类前缀伪装不算）。**`/share` 这条路由本身仍然可达**：直接输入地址或从旧收藏进来，它会照前面两段给出指引——隐藏入口是省得人白跑，不是禁止访问。

**令牌留在地址栏里（决策 191，2026-09-16；修订 182㉙ 的「从地址栏抹掉」）。** 起因是实测反馈「**添加到主屏幕后无法二次访问**」：手机「添加到主屏幕」保存的就是**当时地址栏里那条 URL**，而 **iOS 的主屏 web app 与 Safari 各有独立存储**（localStorage / cookie 都不互通，Apple 文档明说）。于是「装载后把 `?pair=` 抹掉」这件事，等于让主屏图标在「URL 里没有令牌、自己的容器里也没有存储」的空状态下启动：当时看板还读得到（只护写请求），一进对讲台或动写操作就是 403「这台设备还没配对」，而且**再也回不来**——扫码只会打开 Safari，救不了那个图标。故 `capturePairingFromLocation()` 现在只做「读出来 → 存 localStorage」，**不动地址栏**；令牌的取用有三条途径并存：URL（主屏 / 书签）→ localStorage（同一浏览器跨标签页）→ 进程内缓存。

**决策 336 与这一条的关系（写清楚，免得下次绕回来）：** 191 当年明确「不把令牌塞进 cookie」，理由是**主屏 web app 与 Safari 的 cookie 同样隔离**——那条理由针对的是「把令牌从 Safari 递给主屏图标」，它今天仍然成立，故 `?pair=` 留在地址栏这件事一个字没改。336 新增的 cookie 干的是另一件事：**在同一个容器里**护住第二次访问（用户直接输地址、点书签、或地址栏里没有 `?pair=` 的刷新）——导航请求带不了自定义头，而 localStorage 里的那份服务端读不到。两者不冲突：`?pair=` 负责跨容器递令牌，cookie 负责同容器免挂参数。

**当初的三条理由怎么处置的：** `Referer` 泄给第三方那一条，改由**响应头**关掉（`assets.rs` 给所有静态响应加 `Referrer-Policy: no-referrer`）——本应用不引外部资源（字体自托管），这个头是给「以后顺手加外链 / 外图」留的保险，一个头换掉一条理由，划算；**截图与浏览历史里有凭据那两条如实留下**，出路是既有的「重置配对」（旧令牌立即失效）。**残余风险两条，如实记**：① 重置后各设备必须重扫，**已添加到主屏幕的还要重新添加一次**（图标里记的是旧地址），对讲台的失败指引里已写明这句；② 「URL 为准」意味着陈旧的书签会覆盖本地较新的令牌——URL 是使用者的显式动作（点图标 / 点书签 / 扫新码），故取它为准，代价是一次 403 加一次重扫。

### 12.17 106 的 HTTPS 入口（mkcert 长效 IP 证书 + Caddy，决策 327）——**已被决策 335 撤除**

> **这一节记的是「Caddy 反代 + Basic」那一代的做法与它踩过的坑（证书怎么签、裸 IP 为什么不能按域名索引、443/80 在安全组里没放行）。结局是撤掉了 Caddy**：反代把「请求是不是来自本机」这件事抹平了（转发源地址恒为 `127.0.0.1`），于是后端的两条豁免同时命中，应用自己的配对令牌在 106 上**整体失效**，只能靠 Caddy 那层 Basic 顶替——也就是外面那一层成了唯一的门，而应用里那套「按设备可重置」的凭据用不上。**现状与做法见 §12.18**（应用自己终止 TLS，决策 335；全站配对闸门，决策 336）。下面保留的内容里，证书签发、信任链边界、设备侧装 CA 三段**仍然有效**，其余（Caddy 配置、Basic、反代相关验证）只作历史。

**为什么必须上 HTTPS。** 浏览器推送（service worker + Push API）只在**安全上下文**里可用：`https://…` 或本机 `localhost`。106 是裸 IP（`106.12.12.6`）且只有明文 `http://…:3333`，手机上打开它既注册不了 service worker、也订不了推送——「锁屏收推送」这件事必须先把安全上下文建起来。本机开发不受影响（`localhost` 本身就是安全上下文，不需要 CA 也不需要 Caddy）。

**信任链的边界（一条硬约束）。** 根 CA 生成在**开发机**上，**根 CA 私钥永不上 106**：服务器被拿下时偷走的只有一张已签发的叶子证书与它的私钥（换一张重签即可，重签不碰任何设备上的信任），而不是整条信任链的根（那意味着攻击者可以给任意域名签一张被你的设备信任的证书）。**上机的只有两项**：叶子证书 `106.12.12.6.pem` 与它的私钥 `106.12.12.6-key.pem`。

> **⚠️ 切换前先读这一条：拆明文会让配对令牌在 106 上失去牙齿。**
>
> 配对守卫（§12.16、决策 167 / 182⑦）的豁免判据是**来源地址是否回环**，而守卫整体只在**绑非回环地址**时才启用。Caddy 与后端同机，它转发过来的请求源地址就是 `127.0.0.1`；后端一旦按本节的方案改绑 `127.0.0.1`，`lan_mode()` 也变假——**两条豁免同时命中，等于 `https://106.12.12.6` 上的写请求与 `/foreman/*` 全都不再要求令牌**（不装 CA 的浏览器点一次「继续访问」就能全程使用，包括花 token 的对讲台）。这不是「比以前安全一点还是差一点」的取舍，而是**今天那层保护会消失**，故**切换被显式挂起**（决策 323 如实记）。
>
> 两条出路，选一条再动服务器（都不需要改后端代码、都能与 CA 那一套并存）：
>
> | 出路 | 做法 | 代价 |
> |---|---|---|
> | **① Caddy 加一道 HTTP Basic（推荐先走这条）** | `caddy hash-password` 生成一串哈希写进 `basic_auth`，全站一层 | 每台设备第一次进站要输一次用户名口令（浏览器会记住）；与 App 自己的配对令牌**并存**——经代理进来的请求在守卫眼里是回环，靠 Basic 挡外面 |
> | ② 后端学会认可信代理的转发地址 | 让后端在「来源回环 **且**带可信代理标头」时**不再豁免**（读 `X-Forwarded-For` 取真实来源） | 要动 `stream.rs` 的判据 + 一条新决策 + 一组测试；转发头本身可伪造，必须与「只信本机代理」的约束一起落地 |
>
> 换句话说：**HTTPS 与「谁动手要凭据」是两件事，别让前者把后者顺手关掉。**

**已选出路 ①（2026-09-29，决策 332），下面是实际落地的版本。** Caddy 走 106 自带的 EPEL 包（2.6.4，指令名是 `basicauth`——`basic_auth` 是 2.8 之后的改名）、443 用**兜底站点块**、80 回 404；口令明文只在开发机（`~/ca-106/basic-auth.txt`，600），服务器上只有 bcrypt 哈希。**切换尚未走完**：443/80 被**云安全组**挡着（服务器侧没挡），放行之前**不改绑**——先绑回环再发现 443 不通，等于把唯一的入口关掉。

**一次性签发（在开发机上）。** mkcert 是本地 CA 工具，只在开发机装：

```bash
brew install mkcert nss           # nss 用于让 Firefox 也认（可选）
mkcert -install                   # 生成并信任根 CA（进 macOS 钥匙串）
mkcert -CAROOT                    # 根 CA 在哪：rootCA.pem + rootCA-key.pem
```

签一张含 **IP SAN** 的长效证书（浏览器对「裸 IP 的 HTTPS」要求 SAN 里真的有这个 IP；主机名与 IP 的 SAN 类型不同，写错等于没签）：

```bash
cd ~/ca-106                              # 建议单独放一个目录，别混进仓库
mkcert -cert-file 106.12.12.6.pem -key-file 106.12.12.6-key.pem 106.12.12.6
openssl x509 -in 106.12.12.6.pem -noout -text | grep -A2 'Subject Alternative Name'
#   → DNS:…（若有）IP Address:106.12.12.6
openssl x509 -in 106.12.12.6.pem -noout -enddate      # 有效期（mkcert 缺省约 27 个月）
```

**注意两条**：① **不要**把 `-cert-file` 指到 `.pem` 之外的格式上（Caddy 直接读 PEM）；② `~/ca-106/` 与 `$(mkcert -CAROOT)` 都**不要**提交进仓库（`.gitignore` 已挡住 `*.pem`，但根 CA 目录在仓库外更稳妥）。

**上机（在开发机上执行）。** 证书与私钥放到 Caddy 的固定目录，权限只给 Caddy 那个用户：

```bash
scp 106.12.12.6.pem 106.12.12.6-key.pem root@106.12.12.6:/tmp/
ssh root@106.12.12.6 'install -d -m 755 /etc/caddy/certs && install -m 644 /tmp/106.12.12.6.pem /etc/caddy/certs/106.12.12.6.pem && install -m 600 /tmp/106.12.12.6-key.pem /etc/caddy/certs/106.12.12.6-key.pem && rm -f /tmp/106.12.12.6*.pem'
# 服务以 `caddy` 用户跑（EPEL 包固定），私钥必须让那个组读得到——`0600 root:root`
# 会让它起不来并报 `open …-key.pem: permission denied`（2026-09-29 实测踩到）：
ssh root@106.12.12.6 'chown root:caddy /etc/caddy/certs/106.12.12.6-key.pem && chmod 640 /etc/caddy/certs/106.12.12.6-key.pem && chown root:caddy /etc/caddy/certs && chmod 750 /etc/caddy/certs'
```

**106 上的 Caddy（装一次，之后只管 reload）。**

```bash
# 装 Caddy：106 是 BaiduLinux（RHEL 9 系），直接用 EPEL 里的包——实测可装 2.6.4。
# **不要**照抄 Debian 那套 apt / cloudsmith 源：这台机器上没有 apt，且它到 GitHub 与
# cloudsmith 都是跨境慢线（实测 GitHub 直接超时，64KB 要 15s 以上）。
dnf install -y caddy

cat >/etc/caddy/Caddyfile <<'CADDY'
{
	# 静态证书 + 裸 IP：关掉自动 HTTPS 与 ACME（裸 IP 申请不了公共证书）
	auto_https off
}

# **站点块写成 `:443, :3389` 兜底，不要写 `https://106.12.12.6`**。
# 两个端口都听是有意的：443 与 80 在云安全组里**没放行**，而 **3389 恰好放行**（探测
# 得 `Connection refused` 而非超时即可判定放行）——现在入口是 https://106.12.12.6:3389/，
# 将来放行了 443 不用改配置，只把 URL 里的端口去掉。——裸 IP 访问时客户端按
# RFC 6066 **不发 SNI**（curl 与浏览器都一样），按域名索引的站点块拿不到证书，握手当场
# `tlsv1 alert internal error`（2026-09-29 实测踩到）。`:443` 让这张静态证书成为无 SNI
# 连接的默认证书；这台机器 443 上只服务这一个应用，兜底不扩大暴露面。
:443, :3389 {
	tls /etc/caddy/certs/106.12.12.6.pem /etc/caddy/certs/106.12.12.6-key.pem

	# 出路 ①（决策 332）：全站一层 HTTP Basic。Caddy 与后端同机，转发源地址必然是
	# 127.0.0.1，后端改绑之后 lan_mode() 也变假——守卫的两条豁免同时命中，外面这一层
	# 就是 106 上唯一的门。口令明文不落任何地方，这里只有 bcrypt 哈希：
	#   HASH=$(caddy hash-password --plaintext '<口令>')    # 2.6.x 的指令名是 basicauth
	#   （`basic_auth` 是 Caddy 2.8 之后的改名，106 上这版不认那个名字）
	basicauth {
		me <60 字符的 bcrypt 哈希>
	}

	# SSE（/tasks/{id}/stream、/foreman/stream）必须关掉缓冲，否则事件被攒住不下发
	reverse_proxy 127.0.0.1:3333 {
		flush_interval -1
	}
}

# 443 之外不留明文旁路：用 80 直接回 404（不重定向，避免误把明文流量送到应用上）
http://106.12.12.6 {
	respond 404
}
CADDY

systemctl enable --now caddy     # 开机自启
caddy validate --config /etc/caddy/Caddyfile
systemctl reload caddy           # 改完配置只 reload

# 自测（**不带 SNI**，即浏览器访问裸 IP 的真实形态）：
curl -sS -o /dev/null -w '%{http_code}\n' -k https://127.0.0.1/                                   # → 401
curl -sS -o /dev/null -w '%{http_code} %{http_version}\n' -k -u 'me:<口令>' https://127.0.0.1/     # → 200 2
curl -sS -N -k --max-time 26 -u 'me:<口令>' https://127.0.0.1/foreman/stream | head -c 1           # → ':'（心跳帧，证明没有被缓冲）
```

**后端改绑回环（2026-09-29 已执行）。** 顺序是「先让外面真进得来，再关明文」——中间任何一步失败都还能退回去：

```bash
# ① 外网确认 https 入口可用（这一步过了才动手；明文此刻还在，随时可退）
curl -sS -o /dev/null -w '%{http_code}\n' --cacert ~/ca-106/rootCA.pem https://106.12.12.6:3389/   # → 401 = 通

# ② 清理「界面那一级」的绑定覆盖（决策 186 的 DB 覆盖压过 config）：回环发一次 clear 即可。
#    106 上实测 bind_source 一直是 `startup`，本来就没人按过「绑定全网卡」，这条是空操作。
ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 'curl -sX DELETE http://127.0.0.1:3333/server/lan'

# ③ 改 unit 的启动参数（绑定优先级最高的一级）并重启。**那台机器上没有 config.toml**——
#    绑定就是 unit 定的，所以不需要写 config（写了也只是声明式默认，flag 压过它）。
ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 '
  cp /etc/systemd/system/agent-pipeline.service /root/agent-pipeline.service.bak-$(date +%Y%m%d-%H%M)
  sed -i "s|--host 0.0.0.0 --port 3333|--host 127.0.0.1 --port 3333|" /etc/systemd/system/agent-pipeline.service
  systemctl daemon-reload && systemctl restart agent-pipeline'

# ④ 验证（三条都要）
ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 'ss -ltnp | grep 3333'   # → 只剩 127.0.0.1:3333
curl -m 8 -sS -o /dev/null -w '%{http_code}\n' http://106.12.12.6:3333/ || echo '明文入口已关（connection refused）'
curl -sS -o /dev/null -w '%{http_code}\n' --cacert ~/ca-106/rootCA.pem -u 'me:<口令>' https://106.12.12.6:3389/   # → 200
```

**手机与设备侧。** 每台要用推送的设备装一次根 CA 描述文件并显式信任：iPhone 用 **Safari**（不是微信/QQ 内置浏览器）打开 `$(mkcert -CAROOT)/rootCA.pem`（先把 `rootCA.pem` 拷到一台能访问的机器上，或用 AirDrop / 邮件发过去）→ 设置 → 已下载描述文件 → 安装 → 通用 → 关于本机 → 证书信任设置 → **打开**「mkcert …」那一项（少这一步 Safari 仍报不受信任）。**只有根证书上机，根 CA 私钥不上机**——这也是为什么描述文件可以从开发机分发而不是让 106 自己签。

**到期重签（三五年后想起来一次）。** 根 CA 不变、设备不用重装描述文件：

```bash
cd ~/ca-106
mkcert -cert-file 106.12.12.6.pem -key-file 106.12.12.6-key.pem 106.12.12.6   # 同一条命令重签
scp 106.12.12.6.pem 106.12.12.6-key.pem root@106.12.12.6:/etc/caddy/certs/
ssh root@106.12.12.6 'systemctl reload caddy'
```

**Basic 口令的轮换（出路 ① 的要付的那一次维护）。** 口令明文**只在开发机** `~/ca-106/basic-auth.txt`（600，2026-09-29 生成）；服务器上只有 Caddyfile 里那串 bcrypt 哈希，反推不出口令——忘了就直接换一串（各设备下次访问重新弹一次）：

```bash
PW=$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 24)
# 注意：106 上是 Caddy 2.6.4，`caddy hash-password` **只认 --plaintext**（走 stdin 会报
# `Error: EOF`，实测）；这一步口令会短暂出现在服务器进程表里，换完即散。
HASH=$(ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 "caddy hash-password --plaintext '$PW'")
# 改 Caddyfile 的 basicauth 块（把 me 那一行换成新哈希），然后：
ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 'caddy validate --config /etc/caddy/Caddyfile && systemctl reload caddy'
printf '用户名 me\n口令 %s\n' "$PW" > ~/ca-106/basic-auth.txt && chmod 600 ~/ca-106/basic-auth.txt
```


**已知变数，如实记。** ① Apple 对「用户自装 CA」在 Safari 里的信任姿态是政策面的事，未来若收紧，退路是补一个真域名走标准证书（CA 那一套换掉，其余不动）；② 证书到期是**手动**动作，没有自动续期——`openssl x509 -enddate` 是唯一的提醒，记在运维日历里；③ 106 的 `443` 对外开放这件事本身**不增加暴露面**（`3333` 今天就在公网上），增加暴露面的是「拆掉明文入口之后令牌失效」那一条（见上面的警告框）。

**现状（2026-09-29 晚，切换已完成；**这一代的做法已被决策 335 撤除，见 §12.18**）。** 106 上的入口现在**只有一条**：**`https://106.12.12.6:3389/`**（Caddy 全站 Basic；443 与 80 也都在听，但被云安全组挡着，放行之后把 URL 里的端口去掉即可）。落地清单：Caddy 2.6.4（EPEL 直装）+ mkcert 叶子证书（IP SAN，2028-12-29 到期，根 CA 私钥只在开发机）+ 全站 `basicauth` + 后端改绑回环（unit 的 `--host 127.0.0.1`）+ 明文入口关闭。**外网实测**：无凭据 401 / 带凭据 200（HTTP/2，返回应用本体）/ 经代理 `/server-info`、`/tasks`、`/notify/settings` 均 200 / `/foreman/stream` 26 秒内见心跳帧 / `http://106.12.12.6:3333/` 已是 `connection refused`。**为什么是 3389**：443 与 80 在云安全组里没放行（服务器侧没有防火墙挡着；外网探测 443/80/8443/8080/8888/8000 一律超时），3389 恰好放行（探测得 `Connection refused` 而非超时，即包能到、只是当时没服务在听）——Caddyfile 写成 `:443, :3389`，将来放行 443 无需改配置。

**手机访问指向公网入口（决策 334，切换带出来的那个缺口已收口）。** 后端绑回环之后，「手机访问」页原先会走进两条错路：① 判据只看绑定形态，于是永远显示「手机现在连不上这台机器」并递上一颗**按得动**的「绑定全网卡」钮（经反代进来的请求源地址是 `127.0.0.1`，回环豁免命中）——按下去等于把刚关掉的明文入口装回来；② 配对二维码按后端自己的绑定地址拼 URL（网卡候选 + 回环），那台机器的 eth0 是私网、3333 又只在回环上听，指向的是不可达地址。修法是给应用一个**公网入口**（`[server] public_base_url`，形状与 `allowed_origins` 同一种：`scheme://host[:port]`、不带路径）：

```bash
# ① 106：把入口写进 unit 的启动参数（那台机器上没有 config.toml，绑定本来就由 unit 定）
#    老二进制不认识这个参数会同一条规矩**静默忽略**（`parse_serve_args` 的宽容姿态），
#    故这一步可以先做、也可以在部署之后做。
ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 '
  cp /etc/systemd/system/agent-pipeline.service /root/agent-pipeline.service.bak-$(date +%Y%m%d-%H%M)
  sed -i "s|--host 127.0.0.1 --port 3333|--host 127.0.0.1 --port 3333 --public-base-url https://106.12.12.6:3389|" \
    /etc/systemd/system/agent-pipeline.service
  systemctl daemon-reload && systemctl restart agent-pipeline && systemctl is-active agent-pipeline'

# ② 外网验证：/server-info 要同时给出「只绑回环」与那个入口，地址表里只有它一项
curl -sS --cacert ~/ca-106/rootCA.pem -u 'me:<口令>' https://106.12.12.6:3389/server-info \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["loopback_only"], d["public_base_url"], d["addresses"])'
#   → True https://106.12.12.6:3389 [{'interface': '公网入口', 'url': 'https://106.12.12.6:3389', 'preferred': True}]

# ③ 二维码端点认这个入口（带令牌的配对 URL 与原样渲染）
curl -sS -o /dev/null -w '%{http_code}\n' --cacert ~/ca-106/rootCA.pem -u 'me:<口令>' \
  'https://106.12.12.6:3389/server-info/qr.svg?url=https%3A%2F%2F106.12.12.6%3A3389%2F%3Fpair%3Dtest'   # → 200
```

**已落地（2026-09-29，本段命令逐条执行）**：unit 的 `ExecStart` 带上了 `--public-base-url https://106.12.12.6:3389` 并重启；外网实测 `/server-info` 经代理返回 `loopback_only=true` + `public_base_url="https://106.12.12.6:3389"` + 地址表只剩 `[{interface:"公网入口", url:"https://106.12.12.6:3389", preferred:true}]`；二维码端点带令牌的配对 URL **200**、缺省目标 **200**、白名单外 origin **400**；本机 chromium 打开 `https://106.12.12.6:3389/#/share`（Basic 凭据 + 忽略证书）时页面画出的正是指向该入口的码（`.picked` = `https://106.12.12.6:3389/?pair=…`），`.gate` 与「绑定全网卡 / 改回只绑本机」两颗钮**都为 0**，service worker 在信任根 CA 的浏览器里照常注册（scope `https://106.12.12.6:3389/`）。

改完这一页应当：画出**指向 `https://106.12.12.6:3389/?pair=…` 的二维码**、不再出现「手机现在连不上这台机器」与两颗改绑钮、底部改为说清入口来自哪里。**注意这一页仍要在能读到令牌的入口打开**（106 上经反代进来的请求算本机，故从任何设备进站都读得到；见下面的警告框）。另外：`~/.zcode/skills/agentpipeline-deploy-106/SKILL.md` 已同步（入口、验证命令、3389 这个事实、这一条参数）。

### 12.18 106 的入口：**应用自己终止 TLS**，没有反代、没有 Basic（决策 335 / 336）

**为什么撤掉 Caddy。** §12.17 那套（Caddy 反代 + 全站 Basic）能跑，但它把两件事一起丢了：① **应用自己的配对令牌在 106 上失去牙齿**——反代与后端同机，转发源地址恒为 `127.0.0.1`，「回环来源豁免」与「绑回环则不启用守卫」两条同时命中，于是应用里那套「按设备可重置」的凭据用不上，外面那层 Basic 成了唯一的门（那层口令全站共用一个，且没有复位口）；② **多一个进程、多一处配置**，而它做的两件事（TLS 终止、门禁）现在应用自己都能做：TLS 走 `--tls-cert/--tls-key`（决策 335），门禁走**全站配对闸门**（决策 336，未配对的设备连入口页都拿不到，只得到一张配对页）。

**新形态一句话：一个进程，直接听 `0.0.0.0:3389`，自己终止 TLS，整站在配对令牌后面。** 于是 `lan_mode()` 为真（绑的是非回环地址）、来源地址是真的（没有代理抹平它）、`peer_is_loopback` 只对真正从这台机器发出的请求为真。

**证书落位（root 可读即可，应用以 root 跑）。** 证书与私钥从 Caddy 的目录搬到应用自己的目录——**私钥只给 root**（原来是 `root:caddy` 640，因为 Caddy 以 `caddy` 用户跑）：

```bash
ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 '
  install -d -m 700 /etc/agentpipeline/tls
  install -m 644 /etc/caddy/certs/106.12.12.6.pem     /etc/agentpipeline/tls/106.12.12.6.pem
  install -m 600 /etc/caddy/certs/106.12.12.6-key.pem /etc/agentpipeline/tls/106.12.12.6-key.pem
  ls -l /etc/agentpipeline/tls'
```

**改 unit 并重启（顺序要紧：先腾出 3389，再让应用去绑它）。**

```bash
ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 '
  cp /etc/systemd/system/agent-pipeline.service /root/agent-pipeline.service.bak-$(date +%Y%m%d-%H%M)
  systemctl disable --now caddy                      # ① 先让 3389/443/80 空出来
  sed -i "s|^ExecStart=.*|ExecStart=/opt/AgentPipeline/target/release/agent-pipeline serve --host 0.0.0.0 --port 3389 --tls-cert /etc/agentpipeline/tls/106.12.12.6.pem --tls-key /etc/agentpipeline/tls/106.12.12.6-key.pem --public-base-url https://106.12.12.6:3389|" \
    /etc/systemd/system/agent-pipeline.service
  systemctl daemon-reload && systemctl restart agent-pipeline
  sleep 2; ss -ltnp | grep -E "3389|3333"; systemctl is-active agent-pipeline'
```

**验证（四条，缺一不可）。** 都在开发机上跑，全走**外网**（`--cacert` 用 mkcert 的根 CA，不带任何凭据）：

```bash
# ① 入口在、TLS 是应用自己谈的、未配对只得到配对页（401 + text/html）
curl -sS -o /tmp/p.html -w '%{http_code} %{http_version} %{content_type}\n' \
  --cacert ~/ca-106/rootCA.pem https://106.12.12.6:3389/          # → 401 2 text/html; charset=utf-8
grep -c '还没配对' /tmp/p.html                                     # → 1

# ② 接口未带凭据一律 403 + kind（不再是 Basic 那种 401 challenge）
curl -sS --cacert ~/ca-106/rootCA.pem https://106.12.12.6:3389/tasks | head -c 120

# ③ 配对链接进得去，并且种下 cookie（这就是「手机扫一次」的全部）
TOK=$(ssh -i ~/.ssh/106.key -o IdentitiesOnly=yes root@106.12.12.6 \
      'curl -s http://127.0.0.1:3389/pairing/token' | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')
curl -sS -D- -o /dev/null --cacert ~/ca-106/rootCA.pem "https://106.12.12.6:3389/?pair=$TOK" | grep -i '^set-cookie'

# ④ 明文入口彻底没了（3333 与 80/443 都不该有人听）
curl -m 5 -sS -o /dev/null -w '%{http_code}\n' http://106.12.12.6:3333/ || echo '明文已关（connection refused）'
```

**配对（每台设备一次，之后不再问）。** 手机扫「手机访问」页上那张码（`https://106.12.12.6:3389/?pair={token}`）——这一次导航就是配对：应用放行并种下 cookie，此后直接输地址、点书签、刷新都进得去。**换设备或怀疑泄露**：在那台跑服务的机器上（`http://127.0.0.1:3389/#/share`，本机来源豁免）点「重置配对」，旧令牌与旧 cookie **立刻失效**，各设备重扫一次。**手机为什么要装根 CA**：自签证书对浏览器是不受信任的，装上并显式信任之后（§12.17 那一节的做法不变）推送与 service worker 才在安全上下文里；不装也能用（点一次「继续访问」），只是推送用不了。

**回滚（退回 Caddy 那一代）。** 证书还在 `/etc/caddy/certs/`，Caddyfile 也没删：`systemctl disable --now agent-pipeline` → 把 unit 的 `ExecStart` 换回 `--host 127.0.0.1 --port 3333 --public-base-url https://106.12.12.6:3389` → `systemctl enable --now caddy` → `systemctl start agent-pipeline`。**注意回滚会把配对闸门一起关掉**（源地址变回环），那是决策 332 那代的老账（§12.17 的警告框）。

**已落地（2026-09-30，本节命令逐条执行）。** 106 上现在的形态：unit 的 `ExecStart` 走 `--host 0.0.0.0 --port 3389 --tls-cert /etc/agentpipeline/tls/106.12.12.6.pem --tls-key …-key.pem --public-base-url https://106.12.12.6:3389`；**Caddy 已 `disable --now`**（证书仍留在 `/etc/caddy/certs/`，回滚要用）；旧 unit 备份在 `/root/agent-pipeline.service.bak-20260930-*`。**外网实测（开发机 → 106，真跨境路径）**：未配对导航 **401 + 配对页**（带 CA、不带任何凭据）；`/tasks` **403 + `kind: pairing_required`**；`/?pair=<token>` **200 + `Set-Cookie`**（无 `Secure`）；`/sw.js`、`/manifest.webmanifest`、`/icons/*` 一律 **403**；`http://106.12.12.6:3333/` 与 `http://…:3389/` **都不通**（明文旁路全关）。**真浏览器（Playwright chromium，忽略证书错误）**：① 未配对 → 配对页（`#url` 由脚本填上，证明 JS 在 401 响应体里照跑）；② 扫码（`?pair=`）→ 应用完整起来（对讲台/看板渲染、真实读数）；③ **同容器新开一页打开裸地址 → 直接进站**（导航那条路只有 cookie 能授权，故这条即 cookie 的验收）；④ **升级前配过的设备**（只有 localStorage、没有 cookie）打开裸地址 → 配对页把它自动迁移到 `/?pair=<本机那份>` → 进站（**老设备不必重扫**，前提是令牌没重置过）。

**切换时实测抓到的两个坑（都已修进代码，写在这里免得下次又踩）**：① 配对 cookie 一开始带 `Secure`，而**明文形态下浏览器直接拒收**，于是「配对成功却整页空白」——外壳的子资源（`/assets/*.js`、`/sw.js`）既带不了自定义头、地址里又没有 `?pair=`，只能靠这个 cookie 过闸门（决策 336 的账，`stream.rs::enrollment_cookie` 有注释与反向单测）；② TLS 的 ALPN 一开始宣告了 `h2`，而**本工作区的 axum 没开 `http2` 特性**——客户端谈成 h2 便按 h2 发 preface、服务端按 h1 解析，连接当场重置：症状是 TLS 形态下**什么都打不开**而 `curl --http1.1` 完全正常，最容易误判成证书坏了（`serve.rs::alpn_protocols` 有注释与单测）。

**在 106 本机上打开这一页**（要读配对令牌时）走 `https://127.0.0.1:3389/`：证书 SAN 里只有 `106.12.12.6`，浏览器会报名字不匹配——点一次「继续」即可（回环来源豁免，令牌读得到）；命令行用 `curl -sk https://127.0.0.1:3389/pairing/token`。**明文发往 3389 只会拿到空响应**（那不是故障，是往 TLS 端口发明文）。

**闸门转绿（2026-09-30，决策 337 / 338 / 339 / 342 / 343 / 345）。** 上面那条「闸门是红的」的实情已收口。前后红过**九处**，没有一处是环境玄学：① 真红的是 `crates/core/src/rtk.rs::probe_attributes_each_failure`——Linux 上 exec 一个**还开着写 fd** 的文件回 `ETXTBSY`（`Text file busy`），而 `cargo test` 是几百条用例**并排**跑的，别的用例 fork 出来的子进程会把那时开着的写 fd 一起继承走；修法是把假 shim 的**写**交给子进程（`sh -c 'cat > "$1" && chmod 755 "$1"'`），写 fd 就只活在它自己肚子里，窗从形状上关掉，而不是重试等它过去。② `frontend/e2e/ux2-geometry.spec.ts:137` 那条**不是让位算错**——**上一版这里记错了**（把它记成「档案盒压根没进入吸顶状态」）：探针实测吸顶是好的（滚 22–140px 恒为 `dossierTop=94 / tagTop=80`，顶栏下沿 78），红的是**取样点**——这份夹具的页面只能滚 156px，用例写的 `scrollTo(0, 500)` 被夹到 156，采到的是 sticky 的**行程末端**（行程受包含块即网格行的下沿所限，`dossierTop` 掉到 86、铭牌跟着上移 8px → 相交 6px），现在是「钉住之后、离场之前」取样并**先断言确实吸顶**。③ talk 的「发送失败摆成两轮」是一条**真实的共存窗**（重取回包落地那一刻，台账那一行与本地那条同时在场；20 次里红 2 次），判据从「收尾时判一次」搬到**渲染**上，窗从形状上不存在。④ `agent::bounded_read::tests::crossing_the_threshold_raises_exactly_one_claimable_note` 在**数 yield 等另一个线程**（「卡着的」那个减法在**阻塞池线程**里做），换成**有上限的真实时间窗**（2ms 一跳等 5s）。⑤ **第五处红在生产代码上**，是前四处修完、闸门第一次走到集成那一杆（`crates/core/tests/integration/command_funnel.rs`）才露出来的：`RealProcessKiller::kill_process_group` 把「负 pid = 进程组」这条语义交给**外部的 `kill` 命令**去解析，而各家实现并不一致——**procps-ng 的 `kill`（Ubuntu 24.04 = runner，`kill --version` 实测即此）只按 `-PGID` 的第一个数字字符算**（`kill -TERM -7511` 打的是**进程组 7**；`-1234` 会变成 `kill -1`），**退出码 0、stderr 一个字没有**，该杀的进程组一个都没动；macOS 的 BSD `kill` 与 106 的 util-linux `kill` 都按负 pid 办，故这个洞**在开发机与生产机上都看不见**。修法是改走 `libc::kill(-pgid, SIGTERM)`（`libc` 早就在依赖树里，不新增依赖）、非 0 再 `SIGKILL`。**顺带解掉的一个怪事**：那几趟红里 runner 总在用例失败后 12–15 秒报 `The runner has received a shutdown signal`（紧跟 `make ... Terminated`），修完第五处之后同一套跑法**不再出现**——与「那一发打在了别的进程组上」相符。⑥ **第六处红在用例侧，而且只在闸门走到 app 集成那一杆时才露头**（`crates/app/tests/integration/api_contract.rs::proposals_endpoint_lists_only_the_pending_ones_of_that_session`）：它断言「未决提议**按创建顺序**返回」，而那个读端口的顺序来自 `ORDER BY id ASC`（ULID 序）——**同毫秒内两条的先后由 ULID 的随机尾段定**，创建先后并不等价于 id 先后。这条是**高频**偶发：本地十次红三次，CI 上同一个 commit（8fb33ff）一绿一红。修法是用例**按 id 比、按 id 找行**断言（顺序从来不是这条用例的主题，「只给这一班未决的」才是）；班次列表那条用过的 `advance_secs` 在这里**不管用**——它拨的是排序键 `last_active_at`，而这条的排序键是 id（ULID 由系统时钟铸，测试里拨不动）。修后本地连跑 12 次零红、app 集成 224 条连过两遍。⑦ **第七处在装置上**（`crates/core/tests/integration/web_fetch.rs` 的 TinyHttp，notify / foreman 两族测试共用）：满载整套并跑时，它把「连上了但报文还没到」的连接也计一次命中，500ms 读超时又被当成「对端停手」，于是 `wait_hits` 一放行、调用方读到的 `first_request_line` / `request_head` 就是空串——满载四趟红两趟（两条 notify 用例各中一回；单跑复现不了，bluebubbles 那条单跑 12/12 绿）。修法在装置侧：读超时只当「再等一拍」（3s 总时限兜底），计数收窄为「这条请求已收全、三样读数都已写好」之后才 +1（决策 342）。⑧ **第八处在用例与装置的交界上**（`executor.rs` 的 `wait_for_a_held_running_run`）：它抓任务的**第一条** active run，而 `init.execute`（纯代码节点）的 run 完成得快但**不是零耗时**——本地 5ms 轮询窗内 init 早已跑完、抓到的直接是会挂起的 agent run；runner 满载时 init 跑得慢，测试抓到的是 **init 的 run**，随后才拨时钟 400s，executor 起的 agent run `started_at` 落在**拨后**，看门狗看它是新鲜的——`timed_out_runs` 空了，断言当场红（3225215 那趟 check，`a_terminal_run_frees_its_ownership_even_when_the_future_never_returns`）。这颗雷是同文件既有注释记载过的（ladder 那条钉了节点，这两条没钉）；修法：t-stuck / t-chain 改用 `wait_for_running_validate_input` 把等待钉在会挂起的 agent 节点上——agent 节点的心跳只在响应之后刷，run Running 之后不会再有触碰，拨时钟后它必然陈旧（决策 343）。⑨ **第九处在另一会话的 PWA 用例对构建产物的隐含假设上**（决策 340 的 `pairing_lan_unpaired_can_still_fetch_install_assets`）：test 杆**不装 Node**（check.yml 注释写明的设计，`build.rs` 对缺失的 `frontend/dist` 生成**空资产表**，这一态由既有两条资产契约用例明确接受），而它无条件断言四条安装资产 **200**；runner 上 dist 缺失 → 图标 404 → 红（5746d72 那趟，run 36667897838）。本地有 dist 所以两侧都绿、只有 runner 红。修法沿既有双态先例：空表时断 **404**——闸门拦的是 403，对照组三条 403 证明闸门在岗，404 即「放行了、但表里没有」的证据，两层不冒充（决策 345）；check.yml 的注释同步补上第三条用例。**明确没做**：不放宽那几条断言、不给 e2e 加重试、不动那 6px 的布局（让位规则本身是对的）、不给 `probe` / `rewrite` 加 ETXTBSY 重试（生产的形态够不着那扇窗：装 shim 与 exec shim 之间没有并发的 fork）。本地四杆（`make check` 四档）逐杆复验通过；**第五处在 runner 上单跑 5 次全绿、整条集成二进制 463 passed / 0 failed**（临时 debug 分支实测，该分支与它的工作流已删）；main 的复跑结论：8fb33ff（第五处修复）那趟 check（run 36655849488）四杆全绿，5e42661 那趟（run 36657804133）同样全绿，`deploy-106` 由 `workflow_run` 自动接力并成功（run 36658496820，`/opt/deploy.sh` 的探针修正之后）；其间另一个会话手动的 `workflow_dispatch`（run 36657083180）红出的正是第六处——同一份代码一绿一红，恰是「绿靠运气」的实证。第六 / 七处的修复已随 3225215 push 上去，它的 `check`（run 36665212472）lint / frontend 全绿，test 杆 653 条单元全过、core 集成 462 过 / 1 红——红的是**第八处**（⑧ 的老雷，与 339 / 342 无关：proposals 与 notify 两条判据在该趟全过）；⑥⑦ 已随 3225215 验收（run 36665212472 的红是 ⑧）；⑧ 已随 5746d72 验收（run 36667897838 的 core 集成 **463 全过**，红的是 ⑨）；⑨ 的修复随这一段同一次 push，紧接的那趟 `check` 是整段的完整复验。此后 `deploy.yml` 照 `workflow_run` 自动接力，手动 `workflow_dispatch` 那条口子照旧保留（「check 自己挂了要重推一遍 106」）。

**部署链路上的一处陈年探针（2026-09-30，与闸门转绿同一趟发现）。** `deploy.yml` 只发一条 SSH 指令，真正的动作全在 106 的 `/opt/deploy.sh`（自拉、自建、自重启），而**成败由脚本里那串 `&&` 的最后一步——健康探针——判定**。那个探针一直探的是 `http://127.0.0.1:3333/`，即决策 335 **之前**的明文形态；TLS 搬到 3389 之后它必然失败，于是**「部署其实成功了却被判 failed」**：8fb33ff 那一趟就是这样——106 已经 `git log -1` 到 8fb33ff、`systemctl is-active` 是 active、`ss` 上 3389 在听、外网四条检查全绿，而 GitHub 上的 `deploy-106` run 是红的（失败串只写着 `state=failed`，日志尾部的最后一行恰好是 `active`）。现已改成探 TLS 那一处，**401/403 也算通**（未配对时应用回的是配对页 / `kind: pairing_required`，那是「在服务」而不是故障，与 `deploy.yml` 外网检查同一口径）：

```sh
curl -sk -o /dev/null --max-time 10 -w "%{http_code}" https://127.0.0.1:3389/ | grep -Eq "^(200|401|403)$"
```

**这条改动只在 106 上**（`/opt/deploy.sh` 不进仓库，全仓 grep 里没有它的副本）——原脚本备份在 `/opt/deploy.sh.bak-20260930-095220`；改后原地 `sudo -n /opt/deploy.sh start` → 轮询 `status` 得 `state=success`。**同类陈年物再见到就照这条查**：判据里写死旧端口/旧形态的地方，做完形态切换之后不会自己报错，只会在某次「看起来无关」的部署里判出一个假失败。

**已知变数，如实记。** ① 证书到期仍是**手动**重签（`mkcert` 同一条命令 + 重新 `install` 到 `/etc/agentpipeline/tls/` + `systemctl restart agent-pipeline`），没有自动续期；② 配对 cookie **不带 `Secure`**（理由见 §12.16 那张表）：带上它在 106 这种 TLS 形态下同样有效，但明文形态（局域网直连）会**整页空白**——同一个二进制要服务两种形态，故取「明文也不空白」那一档；③ **将来若再放一个反代在前面，闸门会整体失效**（源地址变成回环）——这是 §12.17 那个警告框的同一条算术，不是新问题，但别再踩一次：要代理，就得让后端认「可信代理的转发地址」（决策 332 的出路 ②），或维持「应用直接对外」这条形态。


