# 领域术语表

> 拆分自 agent-pipeline.md（原 附录 A）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

## 附录 A：领域术语表

### 核心概念

| 术语 | 定义 |
|------|------|
| **Task** | 一个待执行的开发任务，有自己的 ID、状态、worktree 和任务目录。对应 `kanban_tasks` 表的一行 |
| **Cursor（游标）** | 任务在某条分支上的执行位置与状态（`branch` / `stage` / `node` / `status` / `validate_attempts` / `pending_reason` / `skipped_to_join`）。执行状态的唯一事实来源，落库 `kanban_node_cursors`（决策 80）。串行阶段恒为一条（`main`），并行阶段两条（此时**没有** main）；生命周期见决策 90 |
| **Stage** | 流水线的一个阶段。共 10 个：init, architect-design, develop-design, test-design, sync-check, develop, review, test, merge, done |
| **Node** | 每个 stage 内部的执行节点。标准模式为 3 个：validate_input, execute, validate_output。init/done/sync-check 有特殊节点集 |
| **Pipeline** | 整个看板流水线的 DAG 图定义 + 执行引擎。基于 petgraph 构建 |
| **ExecutionState** | 流水线执行状态，即全部活跃游标的集合。持久化到 `kanban_node_cursors`，进程重启后据各游标行恢复；`kanban_tasks` 的 `current_stage` / `current_node` / `validate_attempts` 只是焦点游标的投影，供展示与筛选（决策 80） |

### Agent 相关

| 术语 | 定义 |
|------|------|
| **Agent** | 一个 LLM 调用单元，有独立的 system prompt、tool 集合和对话上下文。每个 node 执行时创建一个新的 agent 调用 |
| **Provider** | LLM 服务提供者（DeepSeek、OpenAI、Anthropic 等）。按阶段可配置不同 provider |
| **Tool** | agent 可调用的函数。内置 8 个：`write_file`（写文件）、`edit_file`（局部编辑）、`read_file`（读文件）、`delete_file`（删文件）、`list_dir`（列目录）、`run_command`（执行 shell 命令）、`submit_metadata`（提交结构化元数据）、`Skill`（按名加载技能正文，决策 172③）。扩展工具：`spawn_sub_agent`（**只读**子代理，决策 172③ / 票 08，**需阶段显式声明**，不在 `BUILTIN_TOOLS` 里）。`Skill` 与 `spawn_sub_agent` 的共同点是**不进基线强制集**——前者由阶段声明启用、或存在名字态 / 目录态技能时自动放行；后者只有阶段声明才可用 |
| **Context** | agent 的对话上下文（messages 列表）。有四级压缩机制控制长度 |
| **ContextManager** | 管理 agent 对话上下文的组件，负责 token 计数、摘要压缩、硬限制截断 |
| **validator_cross_check** | 异族复判伪阶段（决策 134）：`cross_family_judge = true` 时，agent 型 validate_output 首判不合格即由它用不同 vendor 的强档模型复判一次；复判合格 → 分歧上交（决策 135），复判不合格 → 维持原路径打回。独立 run/会话行（决策 100 模式） |

### 状态相关

| 术语 | 定义 |
|------|------|
| **TaskStatus** | 任务级状态：queued, pending, waiting, running, done, failed, cancelled。`queued` = 排队等并发准入，与 `waiting`（等依赖）正交（决策 98）；`failed` 仅由用户选择"终止任务"进入（决策 70） |
| **NodeStatus** | 节点级状态，与 `kanban_node_runs.status` 同一套词表：`running` / `success` / `failed` / `timeout`（未开始不落库，由 checkpoint 推断） |
| **Pending** | 任务因阻塞进入的中断状态。不是一个 stage，而是任何 node 都可进入的状态。挂在**游标**上（`kanban_node_cursors.pending_reason_json`），任务级 `pending_reason` 是它的投影（决策 82） |
| **PendingReason** | 阻塞原因结构体，包含 type（info_insufficient/conflict_wait/retry_exhausted/merge_approval 等）、stage、node、message、suggested_actions |
| **Resume** | 从 pending 恢复执行。用户操作后按**恢复动作**（continue / skip / goto）清除 pending_reason，流水线从 checkpoint 继续；取消 / 拆分等**旁路动作**走各自专用 API（决策 69） |
| **Stalled** | pending 超过 `pending_timeout_hours` 的标志位，看板高亮。不是独立 TaskStatus |
| **Archived** | 终态任务的软删除，通过 `archived_at` 时间戳表示。不是独立 TaskStatus |
| **judge_disagreement** | validate_output 首判"不合格"而异族复判"合格"时的 pending context.kind（决策 135）。用户终审：continue 特判直接放行（不重跑校验），或 goto execute 打回（attempts +1） |

### Git 相关

| 术语 | 定义 |
|------|------|
| **Worktree** | git worktree，每个任务的隔离工作区。路径：`~/.agentpipeline/worktrees/{task_id}/` |
| **Branch** | 任务隔离分支，命名：`kanban/{task_id}`，基于创建时的 `project.default_branch` HEAD |
| **Task Directory** | 任务产出目录（设计文档等），路径：`~/.agentpipeline/tasks/{task_id}/`。与 worktree 分离，不污染 git 历史 |
| **Rebase** | merge 阶段将任务分支 rebase 到最新基准分支（有 remote 为 `origin/{default_branch}`，见决策 41）。冲突时打回 develop 阶段 |
| **Merge Proposal** | 本地合并提案。merge 阶段生成 diff 文件 + 统计信息，存储到任务目录，等待用户在 GUI 审核并点击合入（可选"合入"或"返回修改"）。审批状态记在 `merge_result.approval`（决策 72） |
| **Diff** | 基准分支与任务分支的差异（unified diff 格式）。存储在 `~/.agentpipeline/tasks/{task_id}/merge-proposal.diff` |
| **DiffStats** | diff 统计信息：文件数、增删行数、每个文件的变更详情 |

### 阶段流转

| 术语 | 定义 |
|------|------|
| **Validate Input** | 检查输入是否充分。不充分 → pending(info_insufficient) |
| **Execute** | 执行阶段核心逻辑（agent 调用或代码逻辑） |
| **Validate Output** | 验证产出质量。不合格 → 重试 execute（prompt 追加反馈）。超过 max retries → pending(retry_exhausted) |
| **Sync-Check** | develop-design 和 test-design 并行分支的汇聚判断。双方都通过 → proceed；任一方有 blocker → backtrack 回退 architect-design。**不占游标行**，是游标无关的屏障（决策 107） |
| **Backtrack** | 回退到 architect-design 重新设计。由 sync-check 触发 |
| **打回** | review 不通过或 merge 冲突时，回退到 develop 阶段修复 |
| **验收标准（Acceptance Criteria）** | architect 在 design.md「验收标准」节产出的编号完成判据（AC-1、AC-2…），随 submit_metadata 以 `acceptance_criteria` 提交（决策 136）。test-design 经 `design_refs` 引用、sync-check 机械校验引用完整性、review 逐条对照——把"自报 readiness"换成可核对证据 |
| **design_refs** | TestScenario 引用验收标准 id 的字段（决策 136）。high 场景缺失/悬空 → sync-check 判 blocker → backtrack；medium/low → 仅 warning |
| **Retry Feedback** | develop / test 的 retry_exhausted 选"带失败摘要回架构设计"时，系统写入任务目录 `retry-feedback.md` 的重试历史摘要（决策 138），architect 重入时注入 prompt（与决策 126 的 backtrack-feedback.md 同构） |

### 存储相关

| 术语 | 定义 |
|------|------|
| **Checkpoint** | 流水线执行状态的持久化快照，即 `kanban_node_cursors` 中该任务的全部游标行（决策 80）。进程重启后从 checkpoint 恢复 |
| **kanban_tasks** | 任务主表（`current_stage` / `current_node` / `validate_attempts` 为焦点游标投影，决策 80） |
| **kanban_node_cursors** | 活跃游标表，执行状态的唯一事实来源（决策 80）。并行区间不存在**活跃的** main 行（决策 90）；历史行归档保留、永不物理删除（决策 113） |
| **kanban_task_deps** | 任务依赖表 |
| **kanban_stage_outputs** | 阶段产出记录（文件路径 + `metadata_json` 路由元数据）。merge 行的 metadata 额外承载 `gate` / `gate_failures`（决策 108） |
| **kanban_node_runs** | 节点执行记录（可观测性），含 `cursor_id` 归属。子代理各占一行，用 `agent_type` / `parent_run_id` 关联（决策 77 / 80） |
| **kanban_node_conversations** | agent 对话日志（每个节点尝试一行；子代理独立成行，与 run 1:1，决策 77） |
| **kanban_node_commands** | 命令执行记录（agent 和系统驱动的命令及其输出） |
| **kanban_transitions** | 流转记录（节点切换时间线，见 §12.4.2） |

### 调度相关

| 术语 | 定义 |
|------|------|
| **KanbanScheduler** | 独立调度器，处理图外的定时/轮询逻辑。每 `tick_interval_sec`（默认 10s）tick 一次，另有时钟级 maintenance 任务 |
| **Tick** | 调度器的一次轻量检查周期。检查超时、冲突恢复、依赖启动、并发准入、pending 提醒、stalled 标记 |
| **Merge Approval** | merge 阶段等待用户在 GUI 审核 diff 并决定是否合入的状态。用户点击"合入"（`POST /tasks/{id}/merge/decision`，决策 119）后由 `merge.execute` 阶段 B 执行实际合并 |
| **Conflict Wait** | architect 阶段检出文件/符号与活跃任务重叠后进入的 pending，冲突任务终态后自动恢复，属自动串行化 |
| **Duplicate Risk** | 语义层重复风险（不同文件实现同类功能），由 `conflict_check` 伪阶段判定，高风险转 pending(user_decision) |
| **逃逸率（Escaped Rate）** | 各闸门的漏检度量（决策 137）：下游质量事件数（review 打回、merge 闸门失败）÷ 上游闸门放行数，按阶段聚合。v1 只提供查询口径，不做自动归因（`escaped_from` 推断列留 v2） |

### 测试相关

| 术语 | 定义 |
|---|---|
| **测试设计（testing.md）** | AgentPipeline **系统自身**的测试设计（决策 140，落 `docs/testing.md`）。与流水线阶段 `test-design`（为任务设计业务测试场景，决策 136）是两回事：前者测本系统，后者是本系统的一个阶段 |
| **testkit** | 测试基建 crate（`crates/testkit`，决策 146）：用系统 git CLI 搭建场景仓库（unborn HEAD / 脏工作区 / 冲突 / 多语言项目等 8 类），供集成与 E2E 测试复用 |
| **FakeAgent** | 脚本化 LLM 替身（决策 142 / 148）：对 LLM 调用口抽 trait，按 `(stage, node)` 播放 tool_calls 脚本并可注入失败形态；**只替换 LLM 响应流，工具层真实执行**（FileToolPolicy / 脱敏 / 卸载 / 命令记录都真走）。伪阶段同样脚本化；真 LLM 仅 `#[ignore]` 手动冒烟。prompt 只测组装（golden + `prompt_template_hash`），不测效果（B.6 v2） |

### 配置相关

| 术语 | 定义 |
|------|------|
| **Config** | 全局配置（TOML），包含 retry 参数、timeout、日志、prompt 目录等。位于 `~/.agentpipeline/config.toml`，**不含** provider / model / API key 等界面可改的配置（决策 22 / 56） |
| **StageConfig** | 每个 stage 的独立配置，包含 provider（`provider_id` 引用）、tools、skills、超时覆盖（决策 111：不单列 model）。存 DB，界面可改 |
| **技能（skill）** | 阶段或节点声明、被注入上下文的知识/流程指引（决策 170，修订决策 47；决策 172 修订）。名字是唯一身份，两类来源：**用户 markdown**（`~/.agentpipeline/skills/{name}/SKILL.md`，技能根可由 `[skills] dir` 覆盖）、**PATH 外部工具**（决策 47 原语义，如 `rtk`，只有名字无正文）。**内嵌技能已退场**（决策 172①）：二进制不含任何技能正文，技能一律由用户安装到本地。渲染分**三态**（决策 172④）：全文态（正文进 system prompt）、名字态（只列名字，正文由 `Skill` 工具按需拉取）、目录态（未被声明的可用技能只给名字 + 描述，渐进披露）。正文「存在且非空」、frontmatter `name` 与目录名一致、兄弟文件存在，三者都在启动与 `PUT /stage-configs` 时 fail fast（与 `persona_path` 同口径）。**安装入口**（决策 172⑤，票 09）：`POST /skills/import`（上传 zip 原始字节）/ `POST /skills/import-dir`（本地目录，可批量）/ `GET /skills/scan`（扫描已有技能根，如 `~/.zcode/skills`）；同名默认拒绝、覆盖需显式确认，`DELETE /skills/{name}` 卸载**不检查引用**（引用完整性由启动校验与 `PUT /stage-configs` 兜住）。与 MCP 的分工：skill 是知识，MCP 是可调用能力（backlog §B.1） |
| **节点级技能** | 写在 `StageConfig.node_overrides_json[node].skills` 的技能声明（决策 170）。解决「同一阶段不同节点需要不同知识」——如 architect-design 的 validate_input 要拷问、execute 要综合成规格。字段形态为 `string \| {name, mode, trusted}` 混合数组（决策 172④）。有效集 = `mandatory ∪ 阶段级 skills_json ∪ 节点级`（只增不减；同名技能的 `mode` / `trusted` 由**更具体的一层**决定，即节点级 > 阶段级，保留首次出现的位置以维持声明顺序） |
| **AGENTS.md** | 项目上下文文件，每个 agent 启动时加载，拼入 system prompt 的固定段落，提供项目约定和规范 |
| **伪阶段（Pseudo-stage）** | 不进入 kanban 图、无 StageIO/checkpoint/pending 的单次 agent 调用（`project_analysis`、`conflict_check`、`validator_cross_check`），仅复用阶段配置与白名单校验 |
| **cross_family_judge** | 全局开关（默认 false，决策 134）：开启后 agent 型 validate_output 首判不合格时调用 `validator_cross_check` 异族复判；开启但伪阶段未配置 provider → 配置加载 fail fast |
| **lint_command** | 项目级可选静态检查命令（决策 139）。develop.validate_output 先 lint 后测试（都过才放行），merge 闸门同跑；lint 失败是确定性错误，直接打回 develop，不走 test.execute 根因分析 |

### API 与安全相关

| 术语 | 定义 |
|------|------|
| **REST API** | 前后端通信的请求/响应接口（axum 框架），负责 CRUD 操作和控制指令 |
| **SSE** | Server-Sent Events，服务端单向推送，用于 pipeline 状态实时通知前端 |
| **API Key** | LLM provider 的访问密钥，前端配置后**明文**存储在 `providers.api_key` 列（决策 112，修订决策 10）。安全性依赖 `~/.agentpipeline` 的目录权限（§12.14），**不防本机 shell** |
| **FileToolPolicy** | 文件工具的路径策略（决策 104）：约束 6 个文件工具的允许根与 `deny_paths`，判定前 realpath 解析、拒绝写符号链接。**不是系统级沙箱**——shell 不受限 |
| **spawn_blocking** | tokio 提供的函数，将阻塞操作（如 git2 调用）放到专用线程池执行，避免阻塞异步 runtime |

### 视觉语汇（主题六 · 像素机房）

> **仅为视觉语汇，不改变领域模型**（决策 169）。下表的词只在前端界面上出现，是同一批领域
> 实体在像素主题里的叫法；**票、代码、API 与本文档其余部分一律使用左列术语**。
> 读者不要把它们当成新的领域实体。

| 界面叫法 | 对应领域术语 |
|---|---|
| **货箱** | 任务（看板卡） |
| **工位** | 看板列（阶段） |
| **传送带 / 链节** | 轨道脊线（流水线拓扑） |
| **信号灯** | 任务状态色（绿=执行 / 琥珀=等人 / 红=失败 / 灰=归档） |
| **急停 / 操作台对话框** | pending 面板（dossier） |
| **回流带** | 折返线（review→develop / merge→test 打回） |
| **道具栏** | 顶栏状态过滤槽位 |
| **台账** | 设置类页面（项目 / 模型与密钥 / 指标 / 手机访问） |

**待裁：「工头」与「对讲台」**（决策 174）。规格 §1 把**工头**定为**玩家本人**
（= 开发者，dossier 头像即此人），故它**不是**领域实体、未进上表；而决策 174 的对讲台
按用户诉求「和工头对话」把工头摆成**对话的另一方**（值班长）。两种用法冲突，
**裁决前**：`工头` / `对讲台` **不进本表**（不当作领域实体），页面本身已实现于 `#/talk`，
但它渲染的是**真实状态的对话式视图**而非自由对话——**不属 v1**（决策 79），
落点（顶层路由 / 任务详情页签 / 升级只读会话页签）与工头身份一并待裁。

视觉规格与实现映射见 [design/theme-6-pixel.md](../design/theme-6-pixel.md)（对讲台见其 §3.3）。

### 文件路径约定

| 路径 | 用途 |
|------|------|
| `~/.agentpipeline/config.toml` | 全局配置文件 |
| `~/.agentpipeline/data/agentpipeline.db` | SQLite 数据库 |
| `~/.agentpipeline/tasks/{task_id}/` | 任务产出目录（设计文档、merge proposal 等） |
| `~/.agentpipeline/worktrees/{task_id}/` | 任务 worktree（代码变更） |
| `~/.agentpipeline/logs/` | 日志文件 |
| `~/.agentpipeline/bin/` | 二进制安装目录 |
| `~/.agentpipeline/prompts/` | System prompt 模板目录 |
| `{project.local_path}` | 用户项目的本地 git 仓库（唯一事实来源），worktree 的 `.git` 指向它 |
