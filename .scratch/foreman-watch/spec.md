# 值班长的值守与修复（foreman-watch）— 落地依据

**状态：已裁（2026-09-17，决策 209–212）。** 本文是四张票的落地依据；`§6 待裁项` 为空——
本轮从诉求到收口的每一处都逐条对过（round 1–5，共 22 问），不留默契假设。

用户诉求原文：「该项目处于初期，功能不完善，较难跑完整轮任务，希望对讲台能具备和 zcode 一样的
能力，当我创建一个流水线任务时，值班长能帮我盯着任务状态、后台日志和任务各阶段的会话，当任务
卡住或者没有按要求完成时，值班长能分析出代码问题、prompt问题、环境问题等，并在告知为之后自动
修复」。

## 1. 现状：这批为什么不是从零开始

先钉住三件已经在仓里的事，免得把已有的东西重做一遍（也免得把已有的边界当成待建的空地）：

1. **值班长已经有 17 个工具**（决策 188 / 206 / 207 落地，commit `2aec356`）：
   A 层 8 个只读台账 + B 层 3 个只读环境 + C 层 2 个写文件 + E 层 1 个命令 + D 层 3 个本服务接口。
   清单是唯一事实源：`crates/core/src/pipeline/foreman.rs:94` 的 `FOREMAN_TOOL_SPECS`。
2. **环境层的自动执行今天就在**：`gate_decision`（`crates/core/src/agent/tools.rs:148-165`）在
   `env_mode = auto` 下对 `write_file` / `edit_file` / `run_command` 返回 `Execute`。
   所以「告知后自动修复」里的**环境层那一半不需要新机制，只需要拨档位**。
3. **D 层恒提议**：`is_service_write_tool` 那一支**不读档位**（`tools.rs:150-152`，决策 206 原话
   「D 层不读档位」）。要让 `resume(continue)` 免按键，必须改这个函数——**这是本轮唯一的破坏性
   改动**，见 §4.2。

同时钉住两个**已经存在但被丢掉**的信号：

- 调度器每 10 秒的 tick 里已经有发现器（`check_timeouts`、`remind_pending_tasks`、
  `maybe_alert_slow_run`，`crates/core/src/scheduler/mod.rs:148-228`），但**只写日志**；
  `TickReport.reminded` 是内存里的 `HashSet`（`:486-488`），**重启即失、从不推 SSE**，
  前端永远不知道。所以「盯着」不是从零造观察机制，是把已有的发现接出去。
- `pending_timeout_hours`（72h 才判 stalled）与 `pending_reminder_hours`（24h）是给
  「人三天没管」设计的，在「夜里值守」的场景里等于没有。

### 1.1 本批开工前已落地的一件事（非票）

**`Git` 的阻塞调用加上了兜底上限**（`crates/core/src/git.rs` 的 `GIT_OP_TIMEOUT_SEC = 180` /
`IS_DIRTY_TIMEOUT_SEC = 10` / `blocking_within`），`do_init` 的脏工作区检查改成 best-effort。
来历是 2026-09-17 的实测：`支持rtk` 这个任务「一创建就永久卡在 init」，根因是 `do_init` 的
**第一句** `Git.is_dirty()` → `git2::Repository::open` 读 `.git/config` 时被 macOS 拦在
`open()` 里（未签名的 app 没有 `~/Documents` 的访问授权），进程 0% CPU、state=S、永不返回。
完整叙事见决策 209 正文。这条不属于本批任何一张票，但它**是** §2.2 那张事件清单的实证来源。

## 2. 值守的触发与播报（决策 209）

### 2.1 一条纪律：只在「需要有人管」的事件上唤醒

与 `ForemanBriefing` 的既有纪律同源（`crates/core/src/pipeline/foreman.rs` 的注释原话：
「**只装「需要有人管的」**」）。节点成功、每次心跳、每次 SSE、每次工具调用**都不唤醒**。
唤醒是花钱的，而且是在没人在场的时候花。

### 2.2 事件清单

| 事件 | 判据 | 今天有信号吗 |
|---|---|---|
| 任务转 pending | `kanban_tasks.pending_reason_json` 由空变非空 | 有（前端 toast 已有） |
| 重试耗尽 | `PendingKind::RetryExhausted` | 有（pending 的子集） |
| 上下文溢出 | `PendingKind::ContextOverflow` | 有（pending 的子集） |
| 闸门失败 | `MergeResult.gate_failure_kind` / develop 闸门 | **无** |
| 同一任务 30 分钟内转 pending 超过一次 | 需新计数（＝自动修复没治好，与 §4.9 的次数上限是同一件事的两面） | **无** |
| **调度器处置未生效** | run 已是终态（`timeout` / `failed`）而它对应的游标仍 `active`、任务仍 `running` | **无，这一类今天完全不可见** |
| **owner 持有超时** | 任务 `running` 且 `executor_owner` 非空，且超过 N 分钟无 run 心跳 | **无** |
| 任务转 done | `SseEvent::TaskDone` | 有（toast 已有） |
| 慢跑（3×P90） | `metrics::should_alert_slow` | 有（log-only）——**只播报不唤醒**，它是自适应告警 |

「调度器处置未生效」这一类是 2026-09-17 实测倒出来的，不是推演：任务的 run 已被标 `timeout`、
超时处理也写了「干净对话重试」的 transition，但**没有 attempt-2 的 run 行**，任务从此停在
`running`。`check_timeouts` 只看 `active_runs()`，run 已是终态就不再被扫；任务从未转 pending，
`remind_pending_tasks` 的 stalled 判据（`has_pending_cursor && !has_runnable_cursor`，
`scheduler/mod.rs:455-456`）也不成立。**既有调度器与既有台账之间的这条缝，今天没有任何信号。**

### 2.3 载体：发现 → 待办表 → 去抖 → 唤醒一次

```
调度器 tick（现成的发现器）
  → 写「值班长待办」表（新表，一事件一行）
  → 去抖窗口（默认 60s）攒批
  → 窗口到期且有待办 → 唤醒一次诊断轮
  → 播报落进班次（§2.5），待办标记已消费
```

**为什么落表而不是内存**：既有 `TickReport.reminded` 是内存 `HashSet`，重启即失——本次事件里
是实证（不是推测）。落表还顺带给了「这条我处理过没有」一个可查的答案。

### 2.4 静默的判据

- 去抖窗口内没有新事件 → **不唤醒、不说话**（空闲时零 token）。
- 唤醒后诊断结论是「无需处理」→ 静默入库、**不播报**（否则一次自愈的风吹草动都会变成一条消息，
  而播报本身会挤占 §2.5 的历史窗口预算）。
- 播报只写「需要有人管的」那几条，与 §2.1 同一条纪律。

### 2.5 播报落点

落进**当前最新未归档班次**，沿用 `ForemanRunner::resolve_session(None)` 的既有语义
（`crates/core/src/pipeline/foreman.rs:984-1005`）——决策 204 的语义本来就是「一次值班 = 一个会话」，
夜里值守长出来的那一班，第二天早上打开就是它。

手机端复用既有的 toast 通道（决策 65 / 130③ 的通知策略），**不新开视觉档**——决策 203 定过
全站唯一告警仍是急停。

**代价如实记**：历史窗口是 24k 字符预算（`FOREMAN_HISTORY_BUDGET_CHARS`），一夜几十条播报会挤掉
对话上下文。所以 §2.2 的事件清单与 §2.6 的节流**不是可选项，是这个选择的前提**。

**明确不做**独立的值班播报区 / 收件箱：要新建表、新 UI、新未读状态，而「播报要不要也进时间线」
会立刻变成一个没有正确答案的问题。

### 2.6 节流与分级诊断

- 去抖 60 秒 / 同任务 30 分钟冷却 / 全局每小时唤醒上限（默认 12，可配）。
- **分级诊断**：自动那一轮只读台账与诊断包**摘要**；读会话原文（12k 字符）、跑命令这些贵的动作，
  只在被追问或已确认要修时做。
- 主动播报是「固定成本 × 时间」，而你不在场时没有收益能摊平它；分级把贵的那部分留到有人接话时。

## 3. 诊断的证据面（决策 211）

### 3.1 三处断链（都是 2026-09-17 的实证，不是推演）

| # | 断链 | 实证 |
|---|---|---|
| 1 | **失败那一轮的会话根本不落库** | `insert_conversation` 只在成功路径（`executor.rs:1463-1479`）与上下文溢出路径（`:1287-1302`）被调用。`支持rtk` 任务在 `architect-design` 失败于「校验错误：未找到结构化元数据」，`kanban_node_conversations` 里该任务 **0 行** |
| 2 | **它走不动诊断树** | `read_task` 不回 run 列表（`tools.rs:915-934`）；没有任何工具能读命令台账 / 闸门输出 / 阶段产出；`read_metrics` 只有全局聚合（`tools.rs:1006-1036`），拿不到单任务的每 run 耗时与 token |
| 3 | **组装后的 prompt 不留原文** | 只存 `prompt_template_hash`（SHA-256 前 16 位十六进制，`executor.rs:1237-1240` + `prompts.rs:226-231`），且**只对系统段**；用户段连 hash 都没有。所以「这是 prompt 问题」这句话今天没有可核对的证据 |

### 3.2 失败 run 也落库会话

改 `executor.rs` 的失败路径（`agent_node` 的重试耗尽分支，`:958-971`），与决策 99「会话与 run
1:1 落库」同族。**先改断言**：要确认既有没有「失败不落库」的隐含约定被哪个用例钉着，以及
`conversation_max_chars`（默认 20 万字符）的账怎么算。

### 3.3 组装后的 prompt 原文落库

三件必须先答的事，不能顺手做掉：

1. **它跟 `prompt_template_hash` 谁是权威**——建议：hash 降级为「快速比对」的索引，
   原文是权威；两者同时写、同时读得到。
2. **留存期**——跟 `conversation_retention_days`（默认 30 天）同口径。
3. **与 20 万字符账的关系**——落在会话行旁边（同一行的另一列），不另起一张表、不另设保留期。

### 3.4 诊断包工具

**一个新工具，一族一个**（按决策 207④ 的粒度纪律）。一次调用给出：

- 该任务的全部 run：id / stage / node / attempt / agent_type / status / 耗时 / token / error /
  `prompt_template_hash` / `process_group_id` 是否为空
- 命令台账（`kanban_node_commands`）与闸门输出路径（`gate-output-{stage}.log`）
- 阶段产出与验收标准（`kanban_stage_outputs.metadata_json`，含 architect 的 `acceptance_criteria`）
- pending 原因的 `message` 与 `diagnostic` 原文
- 组装后的 prompt 原文（§3.3 落地后）

**为什么不扩 `read_task`**：`read_task` 是高频、便宜的看状态；诊断包低频、一击就撞 12k 上限。
混在一起会让「看一眼任务状态」这件事开始烧 12k 字符，而它在每一轮值守里都会被调用。

**先改断言**：`crates/core/tests/foreman.rs:598-632` 的「工具集恰为这 17 个」按顺序钉着，
加工具**必须**先改它（`foreman.rs:56-59` 的原话：「这是安全边界本身，不是配置项」）。

### 3.5 系统节点留痕

`init.execute` 这类系统节点（非 LLM、非进程组）失败后台账只有一句「超时」，看不到
「卡在哪个系统调用、持着什么锁」。这就是 §3.1 第三处断链之外的第 4 处：**形状有、因没有**。

要求：系统节点在关键步骤边界留痕（至少「进行到哪一步」），且**留痕本身不能再挂住关键路径**
（§1.1 的教训）。

### 3.6 对讲台失败回合留痕

实测：会话 `监测01M2QH0DHKGSGNVHC0WT2Q4CG0…` 里只有两条 `user` 行，`prompt_tokens` 与
`completion_tokens` 全 0，`briefing_json` 与 `traces_json` 全空——**值班长两次没回话，库里没有任何
错误痕迹**，连「为什么没回话」都查不到（`say()` 先把用户消息落库再调 LLM，`foreman.rs:792-795`，
所以失败回合留下的正是这个形状）。

要求：失败时落一条 `role = system` 的账（复用决策 207 已经造好的「操作台记账」那条路），
写一句人话原因。**否则「盯着」的第一条就会以静默失败告终。**

## 4. 修复的授权与载体（决策 210）

### 4.1 任务级托管（默认仍恒提议）

**托管是例外，不是模式。** 默认：值班长的一切写动作照现状——环境层按 `env_mode`，D 层恒提议。
只有你显式对**某个任务**打开托管，它才能对这个任务免按键 `resume(continue)`。

**为什么不做全局档位**：一个全局开关一旦拨过去就再也回不来（你不会记得自己什么时候拨的、
也不知道今晚它是开着的）；任务级托管天然自限（任务一 done 就失效），且与 §4.9 的次数上限是
同一个东西的两面——**托管范围 = 一个任务 + N 次**。

落地：`kanban_tasks` 加一列（或复用 `pending_reason` 的一个字段）+ 端点 + 界面开关。

### 4.2 D 层的唯一例外

`gate_decision` 今天对 `task` / `config` / `skills` **不读档位、恒返回 `Propose`**
（`crates/core/src/agent/tools.rs:148-165`，决策 206）。托管要生效，必须在这一支上开一个**恰好
一个动作**的例外：`task` 工具的 `resume`，且**仅限 `continue`**（不是 `skip` / `goto`）。

- `retry` **永不自动**：它会 `git reset --hard {base_ref}` + `git clean -fdx`
  （`crates/app/src/routes/tasks.rs:321-370`），会洗掉工作区。
- `merge` / `review` **永不自动**：它们写回主干或替人拍板。
- `cancel` / `create` **永不自动**：一个丢掉工作、一个花钱。

**这是本批唯一的破坏性改动**，要改的不止函数：`docs/decisions.md` 的 206 / 207 要标注被修订，
`crates/core/tests/foreman.rs:598-632` 与前一轮改写的广告集断言要同步。故它排在证据面与播报**之后**。

### 4.3 修复 worktree 挂在哪份仓

**两份仓都可修，按诊断结论分派。** 本仓（AgentPipeline 自己）**已经注册成了 project**
（`kanban_projects`：`local_path = /Users/lazyking/Documents/AgentPipeline`，`default_branch = main`，
`test_framework = cargo`，`lint_command = cargo clippy`）——所以「让流水线修自己」在机制上端到端现成。

**本仓的补丁写在一个独立的 git worktree 里**，不是直接改工作区。三条理由都是硬的：

- **可达性**：值班长的文件域是 `workdir_bound = vec![home.root()]`，`FileToolPolicy::check` 的第③条
  对**读和写都强制**「解析后的路径必须落在允许根之内」（`crates/core/src/agent/file_policy.rs:119-136`）。
  本仓在 `~/Documents/AgentPipeline` → `write_file` / `edit_file` **写不了它任何一个文件**。
  唯一能碰到它的是 `run_command`（命令不受文件策略管，`file_policy.rs:70-75` 自己写着「这是补偿，
  不是边界」），而那条路是 shell heredoc，**没有账**。worktree 落在 `{home}/worktrees/` 下 → 可达。
- **隔离**：不污染你正在开发的工作区（你下次 `git status` 不会看见一堆不是你写的改动）。
- **diff 天然**：worktree 的产物天然是 `{base}..{branch}`。

**不新建 `kanban_tasks` 行**（用户裁决「修复要快」）。代价如实记：这条修复因此不在看板上、
不进 `kanban_node_cursors` 那套账——所以 §5.1 的提议表要承担它的账。

**实现代价**：`Git.init_worktree` 现在的签名是 `(project_path, task_id, worktree_path, default_branch)`
（`crates/core/src/git.rs:312-318`），且只被 init 阶段调用；修复路径要自己管一套 worktree 的
创建与清理生命周期。**这是本批最实的一块工程量。**

### 4.4 闸门必过

修复必须过闸门（lint + test）才算「改完」；没过就播报失败、不出 diff。闸门是现成的：
`test_command_for`（`executor.rs:3584-3591`）已把 cargo / pytest / npm 映射好。

**「快」在这里的定义是「不用等你来钉它」，不是「跳过验证」。** 跑一遍 `cargo test --quiet` 是分钟级；
没有它，你早上审的是一份赌注而不是一份补丁。

### 4.5 单独成 commit

修复的改动**单独成一个 commit**，commit message 带标记。目标项目那一类的修复会当场生效
（改任务 worktree → 跑闸门 → resume → 任务带着它写的代码继续跑），而你第一次看到这些改动是在
任务走到 merge 时那份**混在一起的 diff** 里——它手写的代码和 agent 写的代码在 diff 里长得一模一样。

**不做标记的账单会在三个月后到期**：那时你会盯着一行代码想「这是哪个 agent 写的、谁让它这么写的」，
而答案不在库里。带标记的 commit 还顺带给了你一条天然的审计线。

### 4.6 合入永远人按

走既有那颗 merge decision 钮（`POST /tasks/{id}/merge/decision`，`routes/tasks.rs:629`），
或你自己到仓里 `git merge`。**不新增第三条路。**

### 4.7 修补动作面：`unstick` 与「重启服务」提议

**`unstick` 与 `resume` 是两回事。** `unstick` = 清 `executor_owner` + 把僵死的 run 标终态 +
游标转 `pending`（带原因）。它需要 `Runtime` 提供一个 `force_release(task_id)`（把 id 从进程内
`Mutex<HashSet<task_id>>` 去重里摘掉，`crates/app/src/runtime.rs:10`），否则**清了 DB 也没用**——
2026-09-17 那次超时后重试没发生，正是因为原始 `try_run` 没返回、去重仍持有该 task_id，
重试被逐次拒掉。

`unstick` **进自动动作集**（只影响一个任务、可逆、且是 `resume` 能生效的前提）。

**「重启服务」只提议、永不自动**：它是全局动作，会打断所有在跑的任务。它落在决策 206 定的
「全局改动单独提议、人按」那条规则里，而且它是**只重启、不改代码**——与 §4.8 关掉的那件事不是一回事。
这一句必须写进决策，否则规则有歧义。（重启本身是安全的：决策 127 的 `clear_executor_owners` +
`requeue_running_tasks` 会把中断的 running 任务归队，`crates/app/src/serve.rs:319-329`。）

### 4.8 AgentPipeline 自身：不许热修、不许自己重启

诊断结论若是「流水线自身的 bug」：**代码不变 → resume 也白 resume**（同一个 bug 会同样地卡住）。
所以三条一起定：

- **不热修**：不许未经审阅地改本仓运行代码。
- **不自己重启**：重启让修复生效这件事由人按（§4.7）。
- **不试无用的 resume**：出 diff + 播报 + **把任务标成「等修复合入」**，不吃 §4.9 的配额。
  多出来的成本是任务上要有个新标记（新增一个 `PendingKind`，或复用 `pending` + 说明）——
  它让「我在等什么」在**任务本身**上看得见，而不是只在你早上读播报时才知道。

## 5. 修复的生命周期与清理（决策 212）

### 5.1 修复提议不设 TTL，且指纹要换义

复用提议表 `kanban_foreman_proposals`（决策 207 造的：`session_id` / `summary` / `situation_json` /
`claimed_at` / `expires_at` / 四态 `status` / SSE 事件 / 前端确认钮 / 每小时过期清扫）——它的每个
字段都对得上修复这件事，新开一张表等于把 TTL、过期、占用、审计全部重写一遍。**要加的是载荷里
多一种「diff」形态**（现在只有 `tool` + `args_json`）。

但那张表有两个为「当下这一刻」设计的性质，与「等你第二天早上看」直接冲突，**必须改**：

1. **TTL 是 10 分钟**（`crates/core/src/storage/proposals.rs:22`）。修复类提议**不设 TTL**，
   只随年龄清理（与 `conversation_retention_days` 同口径 = 30 天）。不改的话，你早上看到的是
   一排**灰按钮**，还得自己去合。
2. **`situation_fingerprint` 执行时拒执**是为「改任务状态」设计的（任务状态或 `allowed_actions`
   变了就别执行）。修复提议执行的是「合入一个分支」，分支不会因为别的事变迁而失效——**指纹的含义
   要换成「修复分支相对 base 是否还要 rebase、会不会冲突」**：执行时先走 merge 阶段已有的
   `rebase_onto_with_auto_resolve`（`executor.rs:577-591`），能干净 rebase 就合、冲突就拒执并
   告诉你冲突在哪。

### 5.2 worktree 与分支的回收

- **合入成功** → 删分支、删 worktree。
- **被你拒绝或年龄清理** → **保留分支、删 worktree**。分支是唯一的证据，与决策 207「过期只让
  按钮变灰、那一轮留在时间线」同一理由。

### 5.3 项目级 run 的生命周期终止者

`check_timeouts` **有意跳过**项目级 run（`if run.task_id.is_none() || run.cursor_id.is_none() { continue; }`，
`scheduler/mod.rs:158-162`，注释说生命周期归 analyze 端点收尾）。实证后果比「没有超时看护」更重：
`pseudo:project_analysis` 的三条 run（id 10/11/12）`last_activity_at` **冻结在 11:11/11:15**，
**跨一次重启存活**，仍是 `running`——它们没有 `task_id`，`requeue_running_tasks` 不认。

所以这一条不是「让值守去盯」，是**修调度器的洞**：给项目级 run 自己的超时（与节点超时分开语义），
或给它一个明确的终止者。值守顺带播报即可。**动它之前先把 analyze 端点读一遍**，确认收尾语义。

## 6. 待裁项

**空。** round 1–5 的 22 问逐条裁定，无遗留。

## 7. 明确不做

- **不做语义判定驱动的自动修复**（Q3=乙）：主动只报「卡住」。「没按要求完成」是**正确性**问题，
  而今天**没有任何权威判据**——`kanban_tasks` 只有 `title` + 自由文本 `description`，
  全仓没有验收标准字段；唯一像验收标准的东西是 architect-design 阶段**模型自己生成**的
  `acceptance_criteria`（落在 `kanban_stage_outputs.metadata_json`）。**让值班长拿模型写的验收标准
  去审计模型自己的产出，是拿被告的证词当判决。** 语义判定只在被问时做。
- **不做**「值班长自己执行状态机写动作」除 `resume(continue)` 之外的任何一项（§4.2）。
- **不做**热修本仓源码 + 自己重启（§4.8）。
- **不做**独立的值班播报区 / 收件箱（§2.5）。
- **不做**多步事务式的提议组合——一次提议 = 一个动作（决策 207 不变）。
- **不给**值班长开绕过 `allowed_actions` 的旁路（决策 69 / 101 的接缝不动）。
- **不做**「值一次白班就全自动」——托管是任务级的、有次数上限的（§4.1 / §4.9）。

### 7.1 §4.9 —— 循环防护与止损

**补条款**（Q4=甲+乙）：默认 **N = 2**（同一个任务被值班长自动 `resume` 满 2 次就停手、转 pending
等你），且**同一诊断指纹不重复动手**（复用决策 207 的 `situation_fingerprint` / `situation_drift`
思路）。单靠次数挡不住「同一件事被反复触发」，单靠指纹挡不住「每次指纹都不同但都没用」——
两个一起才封住。每次动手必须在班次里留一条可追溯的账（§4.1 的硬要求）。

既有系统里 `agent_retry_max = 3` 管「同一次尝试内的干净重试」，`pending_resume_cooldown_sec = 5`
管「别连点」，但**没有任何东西管「值班长自己的修复循环」**——而它花钱，且会在你睡觉时花。

## 8. 影响面（要改哪里）

| 区域 | 文件 | 改什么 |
|---|---|---|
| 调度器 | `crates/core/src/scheduler/mod.rs` | 发现器写待办表（§2.3）；项目级 run 的超时（§5.3） |
| 新表 | `crates/core/src/storage/migrations/0017_foreman_attention.sql` | 值班长待办 |
| 任务表 | 迁移 + `types.rs::Task` | 托管标记（§4.1）、「等修复合入」标记（§4.8） |
| 值守轮 | `crates/core/src/pipeline/foreman.rs` | 系统简报驱动的唤醒入口、去抖与节流、分级诊断 |
| 对讲台 HTTP | `crates/app/src/routes/foreman.rs` | 失败回合留痕（§3.6）、托管端点、修复提议端点 |
| 工具闸 | `crates/core/src/agent/tools.rs` | `gate_decision` 的 D 层例外（§4.2）——**破坏性** |
| 工具清单 | `crates/core/src/pipeline/foreman.rs` | 诊断包工具（§3.4） |
| 证据面 | `crates/core/src/pipeline/executor.rs` | 失败 run 落会话（§3.2）、prompt 原文落库（§3.3）、系统节点留痕（§3.5） |
| 修复载体 | `crates/core/src/git.rs` + 新模块 | 修复 worktree 生命周期（§4.3）、闸门（§4.4）、commit（§4.5） |
| 提议表 | `crates/core/src/storage/proposals.rs` | 修复类不设 TTL、指纹换义（§5.1） |
| 前端 | `frontend/src/routes/Talk.svelte`、`lib/proposals.ts` | 播报渲染、托管开关、修复提议的 diff 展示与那颗钮 |
| 断言（**先改**） | `crates/core/tests/foreman.rs` | 工具集冻结断言（§3.4）、D 层恒提议断言（§4.2） |
| 文档 | `docs/decisions.md` 206/207 标注被修订；`docs/glossary.md`；`docs/operations.md` 残余风险表（`run_command` 能碰本仓这条既有口子，因为本批会把它变成常规路径） | |

## 9. 切片与次序（**依赖不可换**）

```
证据面 01 → 02 → 03 ┐
留痕   04           ├→ 值守 05 → 06 → 07 ┐
                     │                     ├→ 托管 08 → 09 ┐
                     │                     │                ├→ 修复 10 → 11 → 12
                     └─────────────────────┴────────────────┘
项目级 run 13（与上面各线无依赖，可并行）
收口       14（最后）
```

| 票 | 内容 | 决策 | Blocked by |
|---|---|---|---|
| [01](issues/01-failed-run-conversation.md) | 失败 run 的会话落库 | 211 | — |
| [02](issues/02-prompt-snapshot.md) | 组装后的 prompt 原文落库 | 211 | — |
| [03](issues/03-diagnosis-tool.md) | 诊断包工具（含工具集冻结断言改写） | 211 | 01, 02 |
| [04](issues/04-traces.md) | 两处留痕：系统节点 + 对讲台失败回合 | 211 | — |
| [05](issues/05-attention-queue.md) | 值班长待办表 + 调度器接入（含「提醒只存内存」既有缺陷） | 209 | — |
| [06](issues/06-watch-turn.md) | 值守轮的运行机制 + 播报落进班次 | 209 | 05 |
| [07](issues/07-throttle.md) | 节流与预算 + 分级诊断 | 209 | 06 |
| [08](issues/08-stewardship.md) | 任务级托管开关 + D 层例外（含冻结断言改写） | 210 | 03 |
| [09](issues/09-unstick-restart.md) | 修补动作面：`unstick` + 重启提议 | 210 | 08 |
| [10](issues/10-repair-worktree.md) | 修复 worktree 的生命周期 | 212 | 09 |
| [11](issues/11-repair-gate-commit.md) | 修复的闸门 + diff 交付 + 单独成 commit | 210 | 10 |
| [12](issues/12-repair-proposal.md) | 修复提议（复用提议表 + TTL 与指纹换义 + 前端钮） | 212 | 11 |
| [13](issues/13-project-run-lifecycle.md) | 项目级 run 的生命周期终止者 | 212 | — |
| [14](issues/14-closeout.md) | 收口：人格 / glossary / operations / 决策行 | 209–212 | 01–13 |

**两条硬要求**：

1. **先改断言再加能力**：03 与 08 各自钉着一条被本批改写的边界（工具集冻结、D 层恒提议），
   两张票都必须**先**改断言。这两条断言正是「安全边界本身」（`foreman.rs:56-59` 的原话）。
2. **08 排在 03 之后、10 之前**：托管是 §4.2 那个破坏性例外的载体，而它要以诊断包（03）
   能给出可归因的结论为前提——否则托管是让一个盲诊的值班长动手。
