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
    task_id TEXT NOT NULL,
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
    FOREIGN KEY (run_id) REFERENCES kanban_node_runs(id)
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
    command TEXT NOT NULL,              -- 完整命令行（脱敏后）
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
- 命令**脱敏**：URL 中的 credentials、`--token` 参数、环境变量值替换为 `***` 后存储
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

**离线通知（v2 预留）：** Webhook / 邮件 / 飞书 / Slack 等外部渠道不在 v1 范围（决策 65），完整设计见附录 B。v1 只做 SSE 应用内通知，但保留 `NotificationPolicy` 结构。**职责划分（决策 130）：SSE 全量推送、不做 cooldown 合并**——它是状态同步通道，吞事件会丢状态；cooldown / quiet_hours 只作用于前端 toast 通知层（与 frontend-design §9.1 对齐）。

**通知策略：**

```rust
// crates/core/src/notification.rs

pub struct NotificationPolicy {
    pub notify_on: HashMap<String, bool>,
    pub cooldown_sec: u64,             // 同类通知 5 分钟内合并
    pub quiet_hours: (u8, u8),         // 免打扰时段，pending 除外
}

impl Default for NotificationPolicy {
    fn default() -> Self {
        let mut notify_on = HashMap::new();
        notify_on.insert("pending".to_string(), true);        // pending 立即通知
        notify_on.insert("done".to_string(), true);           // 任务完成通知
        notify_on.insert("failed".to_string(), true);         // 失败通知
        notify_on.insert("cancelled".to_string(), false);     // 取消不通知
        notify_on.insert("node_finished".to_string(), false); // 节点完成不通知
        Self { notify_on, cooldown_sec: 300, quiet_hours: (22, 8) }
    }
}
```

**pending 超时提醒：** 任务进入 pending 超过 `pending_reminder_hours`（默认 24h）未处理，重复提醒一次；超过 `pending_timeout_hours`（默认 72h）自动标记 `stalled = 1`，看板高亮显示。提醒与高亮均通过 SSE 推送，不依赖外部渠道。

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

- **独立图：** kanban 使用独立的 petgraph DAG，各阶段无长期记忆
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

**子代理支持（默认关闭）：** 每个节点的 agent 可派生**子代理**处理可分解的子任务，用于上下文超限兜底（§12.13 L4）。`spawn_sub_agent` 为扩展工具，默认不启用（决策 45）；阶段配置中开启后可用。

```rust
// 子代理通过 spawn_sub_agent tool 调用
// agent 在对话中调用 submit_metadata 时附带子代理任务

// 子代理类型：
// - code_searcher: 代码检索
// - test_runner: 测试执行
// - doc_writer: 文档撰写

// agent 调用示例：
// tool: spawn_sub_agent
// args: { agent_type: "code_searcher", task: "找出所有调用 login() 的位置" }
```

**子代理约束：**
- 子代理继承父代理的工作目录（worktree + 任务目录），不额外隔离
- 子代理的 token 消耗计入同一个任务的 `total_tokens`
- 子代理不允许再派生子代理（最多一层，决策 9）
- 子代理**各自占一行** `kanban_node_runs`（`agent_type` 非 `main`、`parent_run_id` 指向父 run），对应自己那一行 `kanban_node_conversations`，不与父会话混进同一个 `messages_json`（决策 77）
- **默认关闭时，§12.13 L4 兜底直接跳到 `pending(context_overflow)`**，不走子代理拆分

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

pub fn estimate_context_capacity(model: &str, system_prompt: &str, user_prompt: &str) -> ContextCapacity {
    // 窗口大小来自 provider/model 配置（存 DB，界面可改），内置注册表为默认值（决策 46）
    let window = model_context_window(model);
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

L3 后仍超限时的降级路径：

```
1. 强制压缩：keep_recent_rounds 降为 2，丢弃所有非必要内容
   ↓ 仍超限
2. 按节点类型处理：
   - test.execute：分批执行测试（按测试文件分组，每组独立 loop）
   - review.execute：分批评审变更文件（按文件分组）
   - develop.execute：拆分为子任务（仅在 spawn_sub_agent 开启时）
   ↓ 仍超限
3. pending(context_overflow)，用户决定：拆分任务 / 换长上下文模型 / 终止
```

**子代理拆分是首选兜底**（§12.8），但需在阶段配置中开启 `spawn_sub_agent`：把"读 20 个文件"拆成 5 个子代理各读 4 个并返回摘要，父代理只接收摘要，context 天然可控。未开启时直接进入 `pending(context_overflow)`。

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

**自适应超时估算（决策 66）：** 从 `kanban_node_runs` 统计每个 `(stage, node)` 的 P50/P90 耗时，**只取成功运行**（排除 failed/timeout，避免失败样本污染阈值）。用途限于进度展示与告警：前端进度条显示"已运行 4m12s（该节点 P90 为 3m40s）"，超过 3×P90 发一条 SSE 告警。强制超时阈值始终取配置值，`adaptive_timeout_enabled = false` 时完全关闭该估算。

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
| agent 跨任务读写其他任务的 worktree / 任务目录（违反 G11） | **否** | 同上，靠命令日志审计；文件工具层面受 `FileToolPolicy` 约束 |
| 本机其他用户读取数据 | **是**（靠权限） | `0700` / `0600` + 启动校验告警 |
| 目录被整体复制 / 备份外泄 | **否** | 无。这是明文的固有代价，已在决策 112 显式接受 |

> **与 G11 / §9 的关系：** G11 已标注为"策略而非系统保证"（决策 104），本节的残余风险表与 §9 异常处理表中那一行是同一件事的两个视角——§9 从"执行期怎么发现"讲，本节从"静态数据怎么保护"讲。

**本机 API 的跨源防护（决策 128）：** server 只绑 `127.0.0.1`，但任意网页都能向 `http://127.0.0.1:{port}` 发起跨站 POST（HTML form / no-cors fetch 不受 CORS 响应检查约束），`merge/decision`（合入）、`/cancel`、`/resume` 等状态变更端点可被第三方页面驱动。防护（axum 中间件）：所有**写请求**（非 GET/HEAD）必须满足以下之一，否则 403——① 携带自定义头 `X-AgentPipeline: 1`（跨站 form 无法携带自定义头）；② `Origin` / `Referer` 缺失（非浏览器客户端）；③ `Origin` / `Referer` 等于 `http://127.0.0.1:{port}`。SSE 为纯 GET，不受影响。这是与决策 104 / 112 同一威胁模型（"本机、浏览器在场"）的配套防线。
