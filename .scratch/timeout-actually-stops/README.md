# 判超时要真的把 run 停下来（timeout-actually-stops）

**状态（2026-09-19）：已裁并落地（决策 226）。** 这批没有单独拷问——诉求与证据都在同一份材料里：
对讲台班次 `01M2VHMVC3Q7YS6VP5CBV9WF3P` 的四轮播报 + 一个 877 字节的日志 + 代码。

## 起因

用户：「分析最近一轮对讲台的对话」。分析过程中定位到任务 `01M2QH0DHKGSGNVHC0WT2Q4CG0`（支持rtk）
在 `architect-design.execute` 上僵死，且**值班长的根因推断与台账读数双双失真**。

### 一手证据

| 证据 | 内容 |
|---|---|
| `~/.agentpipeline/logs/agentpipeline.log`（877 B） | `00:38:06Z 已清理残留的 executor 持有者 cleared=1` / `已将中断的 running 任务归队待调度 count=1` / `00:43:29.510Z WARN resume 触发被在跑的 executor 持续挡下，放弃本次触发` |
| `kanban_transitions` | 18 号转移（`00:43:27.142`，「节点超时，干净对话重试」）之后**没有** `start`；而 16→17→run 23 之间是 19 毫秒 |
| `kanban_foreman_attention` 343 | 事件 `00:43:27.140`，`created_at 00:53:27.544`（10 分钟宽限，合规） |
| run 22 / 23 | 31886s / 311s，两次 `prompt_tokens: 0`、`has_process_group` 分别为 true / false |
| 四轮播报的账 | 工具调用 35 / 41 / 12 / 2，prompt 419,610 + 573,959 + 79,747 + 17,933 = **1,091,249** |

### 根因链（每一环都有落点）

1. 09-18 15:47Z 前后上一个实例被强杀（`clear_executor_owners` 的指纹），整夜无 tick
   （`task_stale` 从 15:36:52Z 直接跳到 00:38:16Z，中间 9 小时一条都没有）→ 这才是「8 小时 51 分」
   的来源，它是**停机时间**，不是执行时间。
2. 00:38:06Z 新实例起，恢复流程把 run 22 判超时并归队；run 23 起跑。
3. run 23 停在一次**不返回的模型调用**上 → `process_group_id` 为 NULL（只有 `run_command`
   回填过）→ `scheduler/mod.rs` 的 `if let Some(pgid)` 没有东西可杀：**超时只改了台账**。
4. `agent_retry_max = 3 > attempt 2` → 写重试 transition + 调 `resume`。
5. resume 钩子的有界重试是 25 次 × ≤100ms ≈ **2.3 秒**（实测 00:43:27.142 → 00:43:29.510 = 2.37s），
   全被那个还活着的执行体挡下 → 打 WARN，**永久放弃**。
6. 任务僵死到 00:54:43 有人按 `unstick`。那个执行体此后仍然活着（8 小时以上）。

### 值班长为什么报错（这批要修的第二个面）

- 它据 `prompt_tokens: 0` 推出「两次尝试连第一次 LLM 调用都没落账」并当成关键证据报了四轮——
  而失败路径给 `finish_run` 传的是 `RunTokens::default()`，**任何失败 run 都记 0**
  （同一个 0 也长在 run 9/17/18/20 上，那些的死因是已知的 `git 操作超时（180s）`）。
- 「context_window 128k 对上 466k 输入」是**单位读错**：两张表的 `prompt_tokens` 都是
  一轮 / 一次 run 内**所有模型调用之和**（本轮数据自证：41 次调用 / 573,959 ≈ 每次 14k）。
- 判超时的 run 行 `duration_ms` 记 0，于是「跑了 8 小时 51 分」只能靠读的人拿 `started_at` 心算。
- 定死根因的那一行日志在 `{home}/logs/` 下，而文件策略把整个前缀拒了——拒绝的理由写的是
  **体量**（200MB 进上下文），手段却是一堵把 877 字节也挡住的墙。

## 四条修复（决策 226）

| # | 一句话 | 落点 |
|---|---|---|
| ① | 判超时**通知执行体收口**：进程内登记表加一个取消观察点，执行体在模型调用处 `select!`；收到就按 `Error::Cancelled` 收口，**不挂 pending**（那会打回调度器刚放出的重试） | `pipeline/executor.rs`、`error.rs` |
| ② | resume 预算 2.3s → **30 秒**（20ms 起、1s 封顶的指数退避） | `app/src/runtime.rs` |
| ③ | 记账不再用 0 冒充读数：失败轮带回真实累加 token；判超时那条路记真实 `duration_ms`；中止路径**只补用量**、不碰终态与时长 | `executor.rs`、`scheduler/mod.rs`、`storage/observability.rs` |
| ④ | 体量交给结构管：`read_file` 有界读 + `tail`；`logs/` 撤掉前缀拒绝，`data/` 照旧 | `agent/tools.rs`、`agent/context.rs`、`agent/file_policy.rs` |

## 用例映射

| 用例 | 钉什么 |
|---|---|
| `executor::a_timed_out_run_is_stopped_and_reports_its_usage` | 中止后执行体在有界时间内收口、用量补记、**不碰**终态与时长、游标不 pending、执行权真的让出来 |
| `executor::a_failed_round_records_the_tokens_it_burned` | 失败轮照实记 10/5，不是 0 |
| `scheduler_tick::a_timed_out_run_records_how_long_it_ran` | 判超时记 311000ms，不是 0 |
| `tools::a_big_file_is_read_without_being_loaded_whole` | 头部读法如实说「只读了这一段」，`tail` 给到最后一行 |
| `file_policy::the_foreman_root_denies_the_key_store_but_not_logs` | `data/` 读写都拒，`logs/` 读写都放 |
| `env_mode::the_foreman_domain_covers_the_home_but_not_the_key_store`、`foreman::the_foreman_reads_the_home_but_not_the_key_store` | 端到端：日志内容真的进对话，密钥仍然一个字节都出不来 |

三条既有用例因这批**改写了语义**（不是放宽断言）：
`executor::unsticking_releases_the_in_process_dedup_and_allows_a_rerun`（中止之后「重跑」不再由
被判死的旧执行体顺手跑出来，得等一次真正的恢复）、
`env_mode::the_foreman_domain_covers_the_home_but_not_the_key_store` 与
`foreman::the_foreman_reads_the_home_but_not_the_key_store`（用例名随语义从 `…_not_data_or_logs` 改过来）。

另有 1 条既有用例在这一批里**由红转绿、断言一字未动**：
`executor::continued_run_links_back_so_tokens_are_not_double_counted`——它顶出了「失败轮的 token
记真之后，`kanban_tasks.total_tokens` 会比 run 行汇总出来的小」这个不一致，修的是实现（失败与
中止两条路都补 `refresh_task_totals`），不是断言。**这条不是「顺手改的测试」，而是这批新暴露出来的
一个真不一致。**

## 明确不做

- **不做真中止（`AbortHandle`）**：`select!` 只能在 await 点收口，停在阻塞系统调用（`git2` 的
  `open()` 那一类）里的执行体仍旧收不到——要盖住那一类得动线程，与本批无关。
- **不动 `scheduler_no_effect` 的「只报不修」**：本批让那些情形不再产生（超时那条路自己收口），
  真的收不了口时（阻塞卡死）仍旧只有 `unstick` 一条路，与决策 210⑧ 一致。
- **不换日志轮转**：决策 225 刚把日志改成缺省落文件，体积增长收口另立一条。
