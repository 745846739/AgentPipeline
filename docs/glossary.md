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
| **Pipeline** | 整个看板流水线的 DAG 图定义 + 执行引擎。拓扑的事实源是 `landing.rs` 落点表与 `routes.rs` 条件边（决策 248） |
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

### 对讲台（与值班长对话，决策 176 / 182 / 204）

| 术语 | 定义 |
|------|------|
| **班次 / 会话（`foreman session`）** | 对讲台里一段对话的容器（决策 204）：时间线顶部那排 chip 就是班次。四件事——**新建 / 切换 / 重命名 / 归档**；不做物理删除（归档 = 从列表里收起来，消息照旧按保留期清理），**不做分叉**。隔离的是**对话上下文**（这一屏读什么、页头合计多少），**不是权限、也不是态势快照**——「换会话 ≠ 换看板」。一句话：**一条长台账拆成一排可管理的班次** |
| **值班长（`foreman`）** | 任务无关的对话 agent：一个真的会说话的角色，向**值班经理**汇报夜班态势并给出建议。身份是 `stage_configs.stage` 的第 4 个伪键 `"foreman"`（**不进 `Stage` 枚举**），事件身份串 `agent_type = "foreman"`（决策 182①）。每轮拿到一份**夜班态势快照**（待拍板的原因原文、在跑、失败），需要深挖时自己调**只读**工具（台账 `read_task` / `read_conversation` + 环境读数 `read_board` / `read_metrics` / `read_projects` / `read_stage_configs` / `read_skills` / `read_providers`，决策 207 的 A 层，清单见 `pipeline/foreman.rs::FOREMAN_TOOL_SPECS`）。**它能读、能提议，但动手的是值班经理**（决策 188 / 206 / 207）：文件与命令这两层由**权限档位**管（缺省 `ask` → 每次动手生成一条**提议**、等按键；`auto` → 直接执行；`deny` → 连工具都看不到），本服务自己的写接口（建任务 / 拍板 / 合入 / 改配置 / 装技能）**恒为提议、不看档位**。它的域是家目录根（`home.root()`，`data/` 按路径前缀拒——库里明文存着 provider 密钥；`logs/` 原按同一条规则拒，**决策 226 撤掉了它**：体量改由 `read_file` 的字节上限管，而那一堵墙会把 877 字节的日志也一起挡在外面）。它的**回复里永远没有按钮**；时间线上唯一的按钮是操作台内联在提议那一轮里的**确认钮**（决策 176④ / 207③，边界见视觉规格 §3.3） |
| **环境层 / 本服务写接口** | 值班长的工具面按**改的是什么**分成的两组（决策 206，票 04 / 05 / 06）。**环境层** = 文件、命令、技能拉取（`ENV_TOOLS` / `ENV_WRITE_TOOLS`）——它受**权限档位**管，配成 `auto` 可以直达。**本服务写接口** = 改流水线事实的那些（`SERVICE_WRITE_TOOLS`，一族一个工具：`task` / `config` / `skills`）——**恒为提议 + 确认钮，不读档位**；把它们配成 `auto` 只放开环境层。两段清单与实现**同源**（白名单从工具清单生成、执行点按名字分派），故「给值班长加个工具」在 diff 里显式可见。三个**故意缺席**的动作（决策 207⑤：重置配对令牌、局域网开关、仓名单增删）连工具名都没有——判据是「改的是**谁能访问这台机器**」 |
| **权限档位（环境层档位，`env_mode`）** | 三层配置决定的**一个**值（决策 206）：`config.toml` 的 `[pipeline] env_mode`（全局缺省 `auto`）→ `stage_configs.env_mode`（阶段行覆盖）→ 该阶段的**内置缺省**（真实阶段 `auto` = 与档位出现之前逐字相同，值班长 `ask`）。三档语义：`auto` 直接执行；`ask` 把**会改动东西**的动作转成提议（只读的 `read_file` / `list_dir` / `Skill` 照常直通——「读一个文件也要人按键」是把确认钮变成噪声）；`deny` 把整个环境层从**广告集与执行点白名单**里摘掉。解析只有一处（`types::effective_env_mode`），严格解析、不静默降级 |
| **只读取证（`run_readonly`）** | 值班长的**第 21 个工具**（决策 237，**修订决策 232 的「不做第 21 个工具」那半句**）：在只读层里放一份**命令白名单**（`date` / `ps` / `pgrep` / `lsof` / `wc` / `tail` / `sample`），让**值守轮**也能自己取证——此前 `run_command` 被值守轮整个挡掉，7 次自主唤醒全部止步于「我定不死 / 等你按键」。三条护栏写死在实现里：**不经 shell**（argv 直出、不做 `sh -c`，故分号 / 管道 / `$(...)` 没有落点）、**按命令名**判定、参数两条校验（不得越出文件域 `home.root()` 且 `data/` 照旧拒；`sample` 的 pid **必须在「本服务的 pid + 其子进程」集合内**）。它**不受 `env_mode` 档位管**——档位管「能不能改动东西」，而它**改不了任何东西**——也**不在** `FOREMAN_WATCH_TOOL_DENY` 的拦截面里 |
| **值班经理** | 使用本系统的人类开发者（原稿里的「工头（玩家）」）。与值班长说话、看钉在第一屏的状态区、按后端下发的恢复动作——**拍板与写操作只由它做**（决策 176①）。名分原为**值班员**，由决策 193 改为**值班经理**：`员` 与 `长` 是同一序列里相邻的两级，而本系统的权力是反着配的（人格里明写「你没有动手的权力——改状态的动作一律由…按下」），读起来像下属给上级下命令 |

**「工头」一个词曾指两个人（已消歧）。** 规格 §1 原把**工头**定为**玩家本人**（= 开发者，dossier 那枚头像即此人），而对讲台按用户诉求「和工头对话」把工头摆成**对话的另一方**；决策 174 把这一冲突挂标待裁，于是没人敢引用它。**决策 176 裁决：工头 = 值班长**，人类开发者改称**值班员**（该名分由**决策 193** 再改为**值班经理**）——「工头 = 玩家本人」的读法作废，dossier 头像的归属随之明确为「值班长在向你汇报」。界面叫法对照见下面的视觉语汇表。

### 状态相关

| 术语 | 定义 |
|------|------|
| **TaskStatus** | 任务级状态：queued, pending, waiting, running, done, failed, cancelled。`queued` = 排队等并发准入，与 `waiting`（等依赖）正交（决策 98）；`failed` 仅由用户选择"终止任务"进入（决策 70） |
| **NodeStatus** | 节点级状态，与 `kanban_node_runs.status` 同一套词表：`running` / `success` / `failed` / `timeout`（未开始不落库，由 checkpoint 推断） |
| **Pending** | 任务因阻塞进入的中断状态。不是一个 stage，而是任何 node 都可进入的状态。挂在**游标**上（`kanban_node_cursors.pending_reason_json`），任务级 `pending_reason` 是它的投影（决策 82） |
| **PendingReason** | 阻塞原因结构体，包含 type（info_insufficient/conflict_wait/retry_exhausted/merge_approval 等）、stage、node、message、suggested_actions |
| **Resume** | 从 pending 恢复执行。用户操作后按**恢复动作**（continue / skip / goto）清除 pending_reason，流水线从 checkpoint 继续；取消 / 拆分等**旁路动作**走各自专用 API（决策 69） |
| **Stalled** | pending 超过 `pending_timeout_hours` 的标志位，看板高亮。不是独立 TaskStatus。**与「调度器处置未生效」不是一回事**：它要求**所有**活游标都 pending（`has_pending_cursor && !has_runnable_cursor`），而后者恰恰是「游标仍 active」——那个判据**永远不满足**，这正是它此前零信号的原因（决策 209② / 票 05 / 票 13） |
| **调度器处置未生效** | 一类卡死（决策 209② / 票 05 发现、票 13 收口）：run 已是终态（`timeout` / `failed`）而它对应的游标仍 `active`、任务仍 `running`。`check_timeouts` 只看 `active_runs()`（终态 run 不再被扫），`remind_pending_tasks` 的 stalled 判据又不成立——**既有调度器与既有台账之间的这条缝**。判据落在 `pipeline::unstick::stuck_evidence`（报出来与解开的用同一份） |
| **best-effort 检查** | 一个只值一条警告的检查（如 `do_init` 的脏工作区判断）：失败或超时都**不阻塞**关键路径（决策 209 附注 61）。来历是 2026-09-17 的实测——它把 `支持rtk` 的 init 挂死了四小时（栈停在 `git2` 的 `open()`，被 macOS 拦住） |
| **运行故障（run failure）** | 一次看板任务在生命周期里**没有按预期推进的一次具体失败或停摆**（一次 `failed` / `timeout` 的 run、一次僵死、一次「调度器处置未生效」）。它是**一次有待定因的具体事件**，不是「bug」这个笼统说法——**「运行 bug」不作词条**，因为 bug 是还没定因时的说法，而定因之后必须落到**归因类别**四类之一，「我不知道是什么问题」不是结论（决策 227） |
| **归因类别（attribution class）** | 值班长播报一条**运行故障**时**必填**的一个字段，四类：**宿主环境**（操作系统授权、磁盘与 I/O、进程被拦）/ **流水线运行**（调度、台账、超时与心跳语义）/ **目标项目代码**（`repair` 那条 worktree 链吃这一类）/ **prompt 与配置**（上下文预算、阶段配置、provider 参数）。它对上一条故障只说**一个**类别，说不清时也只说一个（证据指向哪一类就说哪一类）——一处不合规（四类之外 / 缺字段 / 坏 JSON / 多处互相矛盾）即判**未定位**，而「未定位」本身是要看见的结论，不是可以含糊过去的写法（决策 227 / 235 / 238）。它的载体是回话里一个**机器可读的结构块**（不是人格里的一句纪律）——写在人格里的必填没有校验点，故**四类之外不许收口**（决策 235）。**结构块里同时带 `run_id`**（正整数，取台账里那一行；指不出单条 run 时可省，不写是诚实的）——它是决策 230 判据①的校验面：只有类别装不下「这次说的是哪条 run」，而**报错 run 与没有 run 同判未定位**（那一课来自 2026-09-19 的一次实测回话：证据与类别全合规，只是把 run 27 的活栈记在了 run 26 名下） |
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

### 推进（advance，决策 245）

| 术语 | 定义 |
|------|------|
| **推进（advance）** | 把一条游标从当前落点搬到下一个落点的那一次写入，**唯一实现**住 `crates/core/src/pipeline/advance.rs`（决策 245）。接口吃**落点 + 修饰**（`Landing::{Entry, EntryWithAttempt, Retry, Stay, JoinBoundary{skipped}, Pause}`）而**不吃原因**——`EdgeKind` / `ResumeAction` / 超时耗尽 / 用户裁决这四种原因在门外各自翻译成落点（judge-continue 不接受 `Terminal`、goto 必须落在 `entry_node`、skip 在 merge 上非法，都是**校验**，留在门外），门只回答「给定落点和修饰，一次事务写完」。它**只返回 `Advanced{cursors}`**，不发 SSE、不做任务投影——那两件由调用方完成（草案里的 `replaced` 字段最终不设：`Split` / 「归档旧行 + 插新行」没进 `Landing`，恒为假的判据就是死代码）。**包事务的是四条路径**：`Retry` / `Entry` / `JoinBoundary` / `Pause`；`Split` 与「归档旧行 + 插新行」仍是既有的原子 `Store` 方法（`split_cursors` / `replace_cursors_with_main`），门只补那笔 transition。落地前它是四份实现（`executor.rs` 的 `apply_edge`、`storage/decisions.rs` 的 `apply_resume` 与 `advance_after_judge_continue`、`scheduler` 的 `handle_timeout`、`unstick`），共同症状是调用方必须自己记住 3–5 笔存储调用的顺序。**「唯一」的边界**：指的是**推进**（把游标搬到新落点）这一件事。另有两处仍直接写 pending 且**有意不进这扇门**——merge 审批 / 人工评审的旁路动作（决策 119 / 124，`storage/decisions.rs`，那是「先写字段再推进」的旁路事务而非推进）与调度器的 `conflict_wait` 复检改写（决策 102，游标本来就在 pending 上，只换 context、不挪落点）——因为 `Store::begin_write()` 是 `pub(crate)`，pipeline 侧本来没有组合事务的能力 |
| **不进推进的东西** | 一条判据（决策 245）：**进程内存 / 重派不进事务模块**。两处适用——`handle_timeout` 的**重试支**（游标不动、attempts 不动、带进程级重派，那不是推进）与 `unstick` 的**编排**（第一步 `force_release` 是不可回滚的内存操作；它签名里那个 `+ Sync` 闭包参数正是硬塞进纯 DB module 的后果）。两处的 DB 那一步仍复用 `advance`。故 `advance` 的入参只有 `&Store` 与纯数据，没有闭包 |

### 调度相关

| 术语 | 定义 |
|------|------|
| **KanbanScheduler** | 独立调度器，处理图外的定时/轮询逻辑。每 `tick_interval_sec`（默认 10s）tick 一次，另有时钟级 maintenance 任务。**票 05 起它还把发现写进值班长待办表**——日志不是通道，表才是 |
| **值守（watch）** | 值班长从「答话的」变成「值守的」（决策 209）：调度器把发现写进**待办表**（`kanban_foreman_attention`，一事件一行）→ 去抖窗口攒批 → 窗口到期唤醒一次诊断轮 → 播报落进当前班次。**只在「需要有人管」的事件上唤醒**：节点成功、每轮心跳、每次工具调用都不唤醒（唤醒是花钱的，且是在没人在场的时候花） |
| **播报（broadcast）** | 值守轮**自己醒过来**说的那一轮：落进会话（`assistant` 行，开头带后端加的 `【值守播报】`），前端据此把名牌渲染成「值班长 · 值守」——与有人问才说的话分开。诊断结论是「无需处理」时**静默**：不落播报行（`【无需处理】` 哨兵，§2.4） |
| **待办（attention）** | 一条待记录的事件（`kanban_foreman_attention`）。去重键是 `(task_id, kind, occurred_at)`——`occurred_at` 是事件**发生**的时刻，不是写入时刻；`consumed_at IS NULL` 即未处理。十二类：`task_pending` / `retry_exhausted` / `context_overflow` / `gate_failure` / `repeated_pending` / `scheduler_no_effect` / `owner_stuck` / `task_stale` / `task_done` / `run_failed` / `task_cancelled` / `slow_run`。**`slow_run` 是唯一只播报不唤醒的**（决策 66 的自适应告警没有可操作的动作）；后加的 `run_failed` / `task_cancelled` **都唤醒**，但吃既有的三重节流（决策 234） |
| **托管（stewardship）** | **任务级**的一次授权（决策 210① / 票 08）：对一个指定任务，值班长可以**免按键** `task resume(continue)`——恰好一个动作。默认关；`kanban_tasks.stewardship_json` 一列装三件事（开着没有 / 自动动过几次 / 上次的指纹）。止损两条一起卡：**满 2 次**或**同一指纹**即停手、交回人按 |
| **unstick** | 「解除僵死占用」（决策 210⑧ / 票 09）：摘进程内去重 + 清 `executor_owner` + 僵死 run 标终态 + 游标转 `pending`。与 `resume` 是两回事——**去重摘不掉时 `resume` 是空操作，还白吃托管的次数配额**。只对「调度器处置未生效」与「owner 持有超时」两类生效，正常在跑的任务踢不动 |
| **等修复合入（awaiting repair merge）** | 修复那条路的收尾标记（决策 210⑨ / 票 11）：补丁出了、闸门过了、提议落成一条**等人按**的，而任务那一侧**不自己往前走**。落地形式是往**焦点游标 pending 原因的 `message` 里追加一句**（`Store::note_task_awaiting_repair_merge`），**不新增 `PendingKind`**——种类还参与 `ResumeCause::classify`，「为什么停」与「现在等什么」是两件事，原句一律保留。没有 pending 游标时什么都不写：一条没停下来的任务不因为顺手提了个修复就显示成「在等」 |
| **修复提案（repair）** | 一条形态为 `repair` 的提议（决策 212① / 票 12）：载荷里带**没设 TTL**的修复现场（worktree / 分支 / 闸门读数 / diff）。执行它不是「一次工具调用」，而是「**合入一个分支**」——按下之前先 rebase 检查，冲突就拒执并列出文件 |
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
| **技能（skill）** | 阶段或节点声明、被注入上下文的知识/流程指引（决策 170，修订决策 47；决策 172 修订）。名字是唯一身份，正文一律**以技能根下的 markdown 落盘**（`{skills_root}/{name}/SKILL.md`，技能根默认 `~/.agentpipeline/skills`、可由 `[skills] dir` 覆盖）；它进入技能根有两条路——**本地导入**（见下「安装入口」）与**从 GitHub 仓安装**（决策 194，见「技能市场」条）——**PATH 工具型技能已退场**（决策 185，修订决策 47 原语义：二进制由 `run_command` 与系统权限管，不是技能）。**内嵌技能已退场**（决策 172①）：二进制不含任何技能正文，技能一律由用户安装到本地。渲染分**三态**（决策 172④）：全文态（正文进 system prompt）、名字态（只列名字，正文由 `Skill` 工具按需拉取）、目录态（未被声明的可用技能只给名字 + 描述，渐进披露）。正文「存在且非空」、frontmatter `name` 与目录名一致、兄弟文件存在，三者都在启动与 `PUT /stage-configs` 时 fail fast（与 `persona_path` 同口径）。**安装入口**（决策 172⑤，票 09）：`POST /skills/import`（上传 zip 原始字节）/ `POST /skills/import-dir`（本地目录，可批量）/ `GET /skills/scan`（扫描已有技能根，如 `~/.zcode/skills`）；同名默认拒绝、覆盖需显式确认，`DELETE /skills/{name}` 卸载**不检查引用**（引用完整性由启动校验与 `PUT /stage-configs` 兜住）。与 MCP 的分工：skill 是知识，MCP 是可调用能力（backlog §B.1） |
| **技能市场（market）** | 技能的两个来源侧入口：**本地导入**（票 09，见「技能」条）与**从 GitHub 仓安装**（决策 194，票 01–04；**取代**决策 172⑤ 的自定 registry——那些旧口径的消歧见本条下方「来源白名单（已退场）」）。信任单元是 `owner/repo`（见「技能来源仓」），权威身份是 **commit SHA**（见「commit 锚」），来源是 **GitHub 仓 + git 通道**（libgit2），**没有索引层**。端点：`GET | PUT | DELETE /market/repos`（仓名单；两级结构——界面那份住 DB、**保存即生效**，清掉回落 `config.toml` 的 `[market] github_repos`，**显式清空 ≠ 未保存过**）、`GET /market/skills?repo=&q=&refresh=`（列出该仓的技能，钉住一个 commit；`q` 是对**已 fetch 那一份**的本地过滤）、`POST /market/install {owner, repo, commit, subpath, overwrite}`。**搜索退化为「我加过的仓」的本地过滤**——不引 GitHub search API，也就没有跨全 GitHub 的目录，界面上不假装它能搜全。**八类失败**（决策 194 裁决⑦）：`market_network`(502) / `repo_not_found`(404) / `commit_not_found`(404) / `skill_not_found`(404) / `repo_unreadable`(404——**此处与决策 194 裁决⑦ 原文的「401·404」不同、按实现口径记**：那两个码是**远端**可能回的形态（见 `repo.rs::unreadable` 的实测缺口注），而 `map_market_error` 恒映 404（票 23 契约用例按实现钉住），与 `repo_not_found` 同码、只有 `kind` 分得开) / `digest_mismatch`(400，语义是「git 对象哈希不符」) / `repo_not_allowed`(400) / `download_too_large`(400)——界面按响应体里机器可读的 `kind` 分支（`repo_not_found` 与 `commit_not_found` 都是 404，只有 `kind` 分得开），原始诊断进 `detail`。落盘**复用票 09 的同一入口**：读出来的技能目录重打成 `{name}/SKILL.md` 单根包交给 `from_zip`，故结构校验 / 同名冲突 / 路径穿越一处生效、两处受益，**远程包不比本地上传的包享有更宽的路**。**明确不做**（决策 194）：私有仓（无凭据入口、界面不放 token 输入框）、跨全 GitHub 的技能搜索、签名与人工审核队列、镜像。**身份校验 ≠ 安全**：`commit` 与 git 对象哈希只证明「没被改过」，证明不了「内容是善意的」——善意性由装前预览与信任标记（票 11）承担。测试经第五条可测试性接缝（仓访问的**远端地址替换点**——原 `SkillRepo` trait 已由决策 250 删除）指向离线 fixture，**默认质量门不打真网络** |
| **技能来源仓** | `owner/repo` 形态的技能来源（决策 194 裁决④）。它是**信任单元**：**放行一个仓 = 允许从它下载引导 agent 的正文**，故这条判定的**规范只有一处**（票 23 收口，决策 250 Q2）：配置与读取层共用 `RepoId::parse` 这一个构造器，界面有自己的一份归一/校验（**调不了 Rust**——页面判定可能发生在任何 API 调用之前，与决策 246 同一个理由），但**不另立规范**——其输出必须落在 `RepoId` 接受集的**子集**里，这条子集不变量由共享表 `tests/fixtures/repo_id.json` 双侧同断言方向机器钉住（vitest 钉前端产出、Rust 表测试钉后端认识它；一侧改了规范另一侧没跟就变红，照决策 246 先例）。**取代旧的「来源 origin 白名单」**——GitHub 模式下 origin 恒为 `github.com`，按 origin 放行等于放行**全世界任何作者的任何仓**。合法性判定拒绝：带 scheme、含 `@`、含 `..`、多余 `/` 或空段、非 ASCII、空 owner/repo（libgit2 的传输注册表认 `git://` / `http://` / `file://` / `ssh://`，**裸文件系统路径也会被 local transport 吃掉**，故用户填的字符串不能直接当 URL） |
| **commit 锚** | 技能来源的身份锚（决策 194 裁决③）：**权威值是 commit SHA**（**完整 40 位十六进制**），列出技能时就钉住一个 commit、安装一路透传——**「看到的 = 装到的」**，否则锚退化成「安装那一刻的 HEAD」。**取代旧的「下载字节 sha256 摘要锚」**：GitHub 给的是**内容身份**而非字节身份（同一 commit 换 URL 形态字节就变、ETag 相同），而对象哈希由 libgit2 在 fetch 时**本地校验**，比 sha256 多覆盖一层目录结构。7 位缩写 SHA 会让 fetch **返回 `Ok` 而什么都不取**（无 ref、无对象、无报错），故必须校验完整 40 位 |
| **技能目录** | 含 `SKILL.md` 的目录（决策 194 裁决⑥）：**技能名 = 该目录的 basename，与深度无关**（实测 7 个流行仓 **290/290** 命中，深度 2–5 段；主流 CLI 的 `getSkillFolderPath()` 同判据）。walk 时按 basename **精确等于** `SKILL.md` 匹配（`endsWith("skill.md")` 会把 `.changeset/xxx-skill.md` 误收）。这是**扫描层**的判据；**解包层**的 `skill_md_key()`（只接受根或单层 `*/SKILL.md`）不因此改动——来源侧选出目录后重打成 `{name}/SKILL.md` 单根包，两层不冲突 |
| **来源记录（`skill_sources`）** | 装下来的技能来自**哪个仓、哪个 commit、哪个子路径**（决策 194，票 02；迁移 0011）。**本批新增的唯一持久化**：同名冲突报文据此报出 `owner/repo@<短 SHA>:<子路径>`；**没有记录时回落「技能根下那份 `SKILL.md` 的路径」**（手工拷进来、本地导入、扫描进来的技能都没有记录）。**不写进技能目录**——目录里的任何文件都会进兄弟文件展开（`from_zip` 过滤 `__MACOSX` / `.DS_Store` 正是为此），自建元数据文件比 `.DS_Store` 更坏。卸载时一并删记录，否则下一次同名安装会报一个已经不存在的技能曾经从哪儿来 |
| **来源白名单（已退场）** | 旧术语：按 **origin**（`scheme://host[:port]`）放行技能来源的名单（决策 172⑤ / 187），配 `config.toml` 的 `[market] allowed_sources` 与界面那份两级结构。**由决策 194 取代**——信任单元改为 `owner/repo`（见「技能来源仓」条），配置键由 `[market] github_repos` 取代（旧键留着会让启动失败，报错文案点明取代关系）。同批退场的还有「自定 `/index.json` 索引格式」「下载字节 sha256 摘要锚」「五类市场失败」三处旧口径，一律见**决策 194**；旧文档里出现这些词时按本条消歧 |
| **输入法护栏（回车提交）** | 让「用回车确认候选词」的那一次回车不被当成「提交」的判据（决策 184，`frontend/src/lib/enterToSend.ts`）：`isComposing` + `keyCode === 229` + **组合结束后 50ms 的时间窗**三重。第三重是必需的——WebKit（桌面壳的 WKWebView）先发 `compositionend` 再发那次 `keydown`，那一刻 `isComposing` 已经是 `false`。接线三处：对讲台输入坞、阶段配置的技能名输入框、指标页的任务 ID 输入框 |
| **绑定来源（bind source）** | `/server-info` 的 `bind_source` 字段（决策 186）：`startup`（`--host` / `AGENTPIPELINE_LAN`）/ `settings`（「手机访问」页上的开关，住 DB）/ `config`（`config.toml` 的 `[server] host`）。三级优先级的可见化——界面必须能说出「这颗钮按了重启还算不算数」 |
| **改绑（rebind）** | 运行时换掉监听地址（决策 186）：`POST /server/lan` → 监听器主管停旧、绑新、起新，**端口不变**；失败回滚到旧地址。只有回环来源能发起（那是全站唯一能把服务暴露到局域网的入口） |
| **节点级技能** | 写在 `StageConfig.node_overrides_json[node].skills` 的技能声明（决策 170）。解决「同一阶段不同节点需要不同知识」——如 architect-design 的 validate_input 要拷问、execute 要综合成规格。字段形态为 `string \| {name, mode, trusted}` 混合数组（决策 172④）。有效集 = `mandatory ∪ 阶段级 skills_json ∪ 节点级`（只增不减；同名技能的 `mode` / `trusted` 由**更具体的一层**决定，即节点级 > 阶段级，保留首次出现的位置以维持声明顺序） |
| **AGENTS.md** | 项目上下文文件，每个 agent 启动时加载，拼入 system prompt 的固定段落，提供项目约定和规范 |
| **伪阶段（Pseudo-stage）** | 不进入 kanban 图、无 StageIO/checkpoint/pending 的单次 agent 调用（`project_analysis`、`conflict_check`、`validator_cross_check`），仅复用阶段配置与白名单校验。**值班长另算**：它也占一个配置伪键（`"foreman"`，与上三个并列），但**不是伪阶段**——那三个都是流水线节点内同步发起的调用，而值班长与流水线无关（决策 182①） |
| **cross_family_judge** | 全局开关（默认 false，决策 134）：开启后 agent 型 validate_output 首判不合格时调用 `validator_cross_check` 异族复判；开启但伪阶段未配置 provider → 配置加载 fail fast |
| **lint_command** | 项目级可选静态检查命令（决策 139）。develop.validate_output 先 lint 后测试（都过才放行），merge 闸门同跑；lint 失败是确定性错误，直接打回 develop，不走 test.execute 根因分析 |

### API 与安全相关

| 术语 | 定义 |
|------|------|
| **REST API** | 前后端通信的请求/响应接口（axum 框架），负责 CRUD 操作和控制指令 |
| **SSE** | Server-Sent Events，服务端单向推送，用于 pipeline 状态实时通知前端 |
| **API Key** | LLM provider 的访问密钥，前端配置后**明文**存储在 `providers.api_key` 列（决策 112，修订决策 10）。安全性依赖 `~/.agentpipeline` 的目录权限（§12.14），**不防本机 shell** |
| **信任标记（trusted）** | 技能声明里的信任态（决策 172④，票 05 / 11）：未受信任的技能**不得以 `full` 模式保存**（写入与启动两侧都拒绝），只能以 `name` 模式使用——正文仍可由 `Skill` 工具按需拉取。信任态**不另存一份账**：它是 `SkillDecl` 的字段，界面上的「信任此技能」是就地改写引用该技能的那些声明（决策 181①），否则「这个技能可不可信」会有两个答案，而这是安全相关判定。裸字符串（老配置行）按 `{full, trusted:false}` 解释，**储存时原样写回**（物化成对象会撞上写入门） |
| **装前预览** | 安装前把三件事摆给用户看（决策 172⑤，票 11）：① 推荐去向（阶段 + 理由）② 注入模式与信任态 ③ 正文特征扫描（`run_command` / 网络调用 / 密钥路径字样，**逐行列出**）。③ **只用于告知、不参与准入**——正则拦不住变形又误伤合法技能，风险由预览 + 信任标记 + 工具层出口控制承担（决策 181③）。`GET /skills/{name}/preview` 看已装的，`POST /skills/preview` 看**包里的字节**（还没落盘） |
| **只读子代理（`spawn_sub_agent`）** | 扩展工具（决策 172③，票 08；重开决策 154）：派生一个**只读**子代理处理可分解的检索子任务，把「读 20 个文件」的原文挡在父上下文之外。工具集**固定为 `read_file` / `list_dir`**（无 `run_command` / 写文件 / `submit_metadata`）、**不继承阶段声明的工具**、**不再派子代理**（深度一层，决策 9）。边界落在**执行点**（`ToolExecutor::execute` 的工具白名单）而不是 tool 定义上——只限制广告出去的定义是纸糊的，模型可以无视定义直接发一次调用 |
| **工具层出口控制** | `run_command` 的网络出口策略（决策 179，票 12）：默认 allowlist 且**只放行回环**（什么算回环见「回环判定」条，决策 246），未放行的目标被拒并落 `kanban_node_commands`（与放行的命令同表）。只约束 agent **主动经 `run_command` 发起**的调用，**约束不了被启动子进程自行联网——不是安全边界**（残余风险与 OS 级沙箱候选见 operations §12.15） |
| **回环判定** | 「这个主机名是不是本机」的判据（决策 246）。**一条有领域分量的规则**：它同时决定**出口放不放行**（工具层出口控制——空 allowlist 只放行回环）、**配对令牌要不要**（绑定地址与配对来源走回环豁免）、**手机访问入口显不显示**（非本机打开的页面不给入口，决策 190）。**四处判回环、一处 module、一张共享表**：Rust 侧唯一实现住 `crates/core/src/host_policy.rs`（出口策略 `egress.rs`、技能来源仓明文 http 放行 `repo.rs`、服务绑定与配对 `app/peer.rs` 三处改调它；`PeerAddr::is_loopback` 判的是已解析的 `SocketAddr` IP、不在主机名判定域内）；前端 `localPage.ts` 调不了 Rust（hostname 判定可能发生在任何 API 调用之前），同源靠 `tests/fixtures/host_policy_loopback.json`——两侧读同一张输入→期望表、同一断言方向，一侧改规范另一侧没跟就变红。规范形态：归一（去空白/转小写/去一个尾点/脱方括号）→ `IpAddr::is_loopback()` → 否则 `localhost`；`127.evil.test` 这类前缀伪装**不算**回环（决策 179 的 `127.*` 字面写法由此收窄） |
| **会话续接** | pending → resume 重入时从 `kanban_node_conversations` 读回上一 attempt 的 messages 作为起点，信息补充型 pending 不必让 agent 从零重读仓库。只作用于 **pending → resume 边界**，`agent_retry_max` 的干净重试语义（决策 33）不变（**模型的自动失败重试不给续接，人的介入才给**）。**开不开由原因决定**（决策 205，取代决策 180 的阶段 / 节点开关）：被清掉的那个 pending 原因查代码里的判定表 `types.rs::resume_continues`，故**设置里没有这个开关**。三个连带的必要条件：`context_overflow` 退出路径补写会话行、token 双算防护（`continued_from_run_id`）、压缩锚点不认载入的历史 |
| **FileToolPolicy** | 文件工具的路径策略（决策 104）：约束 6 个文件工具的允许根与 `deny_paths`，判定前 realpath 解析、拒绝写符号链接。**不是系统级沙箱**——shell 不受限 |
| **spawn_blocking** | tokio 提供的函数，将阻塞操作（如 git2 调用）放到专用线程池执行，避免阻塞异步 runtime |

### 视觉语汇（主题六 · 像素机房）

> **仅为视觉语汇，不改变领域模型**（决策 169）。下表的词只在前端界面上出现，是同一批领域
> 实体在像素主题里的叫法；**票、代码、API 与本文档其余部分一律使用左列术语**。
> 读者不要把它们当成新的领域实体。
>
> **右列「首现翻译」是决策 200 的定稿文案**（与 `design/frontend-design.md` §12.2 同源，
> 两处必须一致）：隐喻保留，但**每个词在「每个页面」内首次出现处**给一次平实说法，
> **同一页面内不重复**；形态是**行内全宽括号、紧跟词后、同字号同档**（用 `--text-3` 这一
> 「次级必读」档，决策 195），**不新增图元 / 颜色 / 徽章 / 背景 / 动画位**，`title` 不承载译文
> （那是内部编号的位子，决策 199）。**按钮与标题里不翻译**：顶栏三项名、「新建任务」、
> 状态过滤槽的词、空态的下一步一律直接用平实词（归票 21 执行）。
>
> **状态过滤槽的七个词不进本表**（决策 201）：`全部 / 执行中 / 待处理 / 等依赖 / 排队 /
> 已完成 / 已结束` 不是车间隐喻，而是 `TaskStatus` 这些**领域状态词本身**的中文叫法，没有
> 第二个叫法可译；需要认的是那七枚**自造图元**（货箱 / 齿轮 / 急停三角 / 合并 / 旗 / 奖杯 / 锤），
> 它们的身份由主题契约的 sprite 表管。

| 界面叫法 | 对应领域术语 | 首现翻译（定稿文案） |
|---|---|---|
| **货箱** | 任务（看板卡） | `货箱（一张任务卡）` |
| **工位** | 看板列（阶段） | `工位（流水线的阶段）` |
| **传送带 / 链节** | 轨道脊线（流水线拓扑） | `传送带（这条流水线的顺序）` |
| **信号灯** | 任务状态色（绿=执行 / 琥珀=等人 / 红=失败 / 灰=归档） | `信号灯（任务的状态色）` |
| **急停 / 操作台对话框** | pending 面板（dossier） | `急停（等你拍板的阻塞）` |
| **回流带** | 折返线（review→develop / merge→test 打回） | `回流带（打回重做的那条线）` |
| **道具栏** | 顶栏状态过滤槽位 | `道具栏（顶栏那排状态过滤）` |
| **台账** | 设置类页面（项目 / 模型与密钥 / 阶段配置 / 技能市场 / 手机访问 / 指标） | `台账（设置这一类页面）` |
| **工头** | 值班长（对讲台对面那个会说话的角色，决策 176） | `工头（就是值班长，跟我对话的 AI）` |
| **值班长** | 与人对话的那个 agent（`stage_configs.stage = "foreman"`，决策 182①） | `值班长（跟我对话的 AI）` |
| **值班经理** | 使用本系统的人类开发者（决策 193 起；176 原定「值班员」） | `值班经理（你）` |
| **对讲台** | 与值班长对话的页面（`#/talk`，决策 176 / 182） | `对讲台（跟值班长说话的地方）` |

**「工头」与「对讲台」已消歧（决策 176）。** `工头` 是**值班长**在界面上的叫法，不再是
「人类开发者本人」——人类开发者称**值班经理**（决策 193 起；176 原定「值班员」，定义见上「对讲台」一节）；决策 174 挂起的
身份冲突由此了结。对讲台落点维持**顶层路由**（`#/talk`，与看板并列），并随决策 79 的修订
**纳入 v1**；它在**顶栏页面导航行的第 1 项**（决策 198 把导航行收到三项：对讲台 / 指标 / 设置）。

视觉规格与实现映射见 [design/theme-6-pixel.md](../design/theme-6-pixel.md)（对讲台见其 §3.3）；
交互规格与文案规范见 [design/frontend-design.md](../design/frontend-design.md)（§4 信息架构、§12 文案与可追溯性）。

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
