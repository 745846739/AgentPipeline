# 02: 重试裁定纯函数

**What to build:** 决策 356 后半——「重试裁定」抽成纯函数：输入 outcome 序列
（传输类 / 配置类 / 上下文超窗 / 连续超时轮数…）→ 下一动作（原样重试 / 空白重跑 /
挂起 / 收口）。合并现散在 respond_inner 循环里的各分支判据：决策 298（按错误类别
分流）、295（超窗压一次再试不按 agent_retry_max 盲试）、320（连续超时四段梯子）、
288/233（attempt 级重试与墙钟界）。

**Blocked by:** 01

**Status:** done（已实现，决策 356；**落点见注记 ①**）

- [x] `foreman/turn_plan.rs`（或同 effort 旁挂文件）：`fn adjudicate(outcomes) -> Action`
      纯函数；循环各分支改查裁定——落成 `crates/core/src/pipeline/retry.rs`：
      `after_attempt_failure(&Error) -> AttemptRetry` 与 `timeout_retry(streak) -> TimeoutRetry`；
      调用点 `model_invoke::agent_node` 的轮循环与 `scheduler::handle_timeout` 的三岔
      各自改成一个 `match`，只留与库 / 进程 / 游标打交道的那一半
- [x] 单测：四段梯子全路径（第 2/3 次不挂、第 4 次挂起）、传输类重试不追加错误 turn、
      配置类不进下一轮、超窗压缩一次重试、attempt 耗尽——`pipeline/retry.rs` 四条：
      `the_timeout_ladder_is_four_steps_and_pins_every_rung`（四档逐档钉）、
      `a_transport_failure_retries_without_an_error_turn`（含适配器层未分类失败）、
      `a_useless_wait_never_gets_another_round`（超窗 / 鉴权 / 模型名 / 额度四类）、
      `an_output_contract_failure_retries_with_the_error_turn`（278 本体 + 未识别类别）。
      「超窗压缩一次重试」的压与试在调用点上（`agent_attempt_inner`，决策 295 原样），
      本票不动它——裁定只回答「压不动之后还进不进下一轮」
- [x] FakeAgent 接缝零改动（裁定不引入新替换点，决策 250 姿势）——两处改动都是
      「把 `if` 链换成对纯函数的 `match`」，没有新 trait / 新注入轴
- [x] 验证：超时/重试族 e2e（含 e2e-14）照绿；core 全量 + lint 绿——`e2e` 40 条过
      （含 `e2e_14_timeout_chain_kills_retries_then_pends_and_merge_has_no_skip` 与
      `e2e_14_long_system_command_heartbeat_survives_idle_timeout`）、
      `scheduler_tick` 42 条过、`executor::` 75 条过、往返族 10 条过；fmt / clippy 见提交闸门

**注记（留给后来者）**：

- ① **票面把落点写成 `foreman/turn_plan.rs`，但四条验收判据（四段梯子 / 传输类 / 配置类 /
  e2e-14）说的全是流水线的两张表**——读代码当场坐实：决策 298 与 295 在
  `model_invoke::agent_node` 的轮循环里，决策 320 的梯子在 `scheduler::handle_timeout`
  的三岔里，两条都不在 `respond_inner`。按判据落地，落点取 `pipeline/retry.rs`
  （票面允许的「同 effort 旁挂文件」，且这是流水线层的裁定，不能反向依赖 foreman 模块）。
  **值班长这一轮自己的表**（成本门与窗口门）已在票 01 落进 `TurnPlan`
  （`cost_verdict` / `check_window_budget`），不需要第三处。
- ② **两条表为什么不合并成一个 `adjudicate`**：动作词汇不同——流水线这边是「续接 /
  空白重跑 / 挂起」，值班长那边是「下一次调用 / 收口」。硬拉成一套会把两个状态机的
  语义压扁（与决策 355「两套词汇不动」同一姿态）。共同的部分（判据按 `kind` 字段、
  不按报文字样）本来就在 `agent/providers` 里共享着。
- ③ **纯判定的边界划在「等一等有没有用」上**：`after_attempt_failure` 只回答进不进下一轮
  与要不要带错误 turn；**重试几次**（`agent_retry_max`）与**超窗那次就地压缩**仍在调用点
  ——它们是循环的形状，不是一次裁定的输入（决策 356 明文不做宽边界）。
- ④ **`TIMEOUT_AUTO_CONTINUES_MAX` 随之搬家**（scheduler 里那份删掉）：梯子的档位与它的
  常量同址，改档位只碰一个文件。

## Comments

- 2026-09-30 实施完毕（决策 356 后半）。两轴评审各一轮：
  - **Spec 轴把落点问题判清了**：票面第一格的 `foreman/turn_plan.rs` 与其余四格判据
    （四段梯子 / 传输类 / 配置类 / e2e-14）冲突，判据那一侧成立（决策 356 没有指定文件，
    只说「梯子若要单测，走纯函数裁定这条路」），故落 `pipeline/retry.rs` 是对票面意图的
    忠实实现，不是漏项。评审同时指出两条措辞差异，如实记：**票面写一个
    `fn adjudicate(outcomes) -> Action`，实交两个函数**（两条表的动作词汇不同，见注记 ②）；
    **「超窗压缩一次重试」与「attempt 耗尽」两条没有被写成新单测**——它们的行为在调用点
    原样未动，钉住它们的是既有用例
    （`executor::a_context_window_failure_compacts_and_retries_that_one_call`、
    `scheduler_tick::each_failed_attempt_of_a_retrying_task_is_a_row`），本票的判据是
    「照绿」而非「新写」。
  - 头注补了一节「这是一次搬家，不是改口径」，并附上「既有用例一个字没改」这条证据。
- **评审指出的可做未做（Judgement call，记下）**：`scheduler::handle_timeout` 的两条
  `AutoContinue` 分支仍各写一遍 `insert_transition`（分支体差异只有文案与是否标续接）。
  搬梯子时本可顺手收掉，但那是**改形状**而不是搬判定，与「零行为变化」的验收线相抵，
  故留原样。
