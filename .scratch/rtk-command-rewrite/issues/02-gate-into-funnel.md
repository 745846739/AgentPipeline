# 02: 闸门接入收口——两处既有缺陷跟着修

**What to build:** `executor.rs` 的 `run_system_command` 与 `repair.rs` 的 `run_gate_command` 里那两个
`sh -c` 旁路改走 `CommandRunner`。闸门由此拿到**进程组 + 心跳 + 超时杀干净 + 脱敏台账**，两处既有缺陷
消失（它们是「两个实现」的产物，不是新需求）：repair 闸门今天**完全没有超时**（`.output().await` 裸调），
归 `test_command_timeout_sec`（默认 600）；runner 闸门今天超时**只丢 future、不杀进程**，而且闸门从不
回填 `process_group_id`，所以调度器那条超时收口也够不着它——超时后留下活着的进程树。闸门传
`Rewrite::None`。

**Blocked by:** 01

**Status:** done（已实现；交付说明见文末「闸门行为变化逐点核对」）

- [x] 闸门命令超时后进程树真的没了：新测试起一条会留子孙的命令，超时后断言子孙都不在
      （今天的 runner 闸门过不了这条）
      → `command_funnel.rs::a_timed_out_command_takes_its_descendants_with_it`（`sleep 300 &` 留一个
      脱离管道的子孙，超时后它必须不在）。闸门那侧的形状由
      `repair.rs::the_repair_gate_is_bounded_by_the_test_command_timeout` 钉。
- [x] 闸门回填 `process_group_id`，调度器的超时收口对闸门也成立
      → `command_funnel.rs::a_source_system_command_backfills_its_process_group`。
- [x] repair 闸门受 `test_command_timeout_sec`（默认 600）管——**不新增配置键**。理由：同一批命令在
      develop 闸门今天已经受这个数管，所以「测试套件超过 600s」的项目今天就已经在那边失败了，给
      repair 同一个数不是新增风险；而新键要动 `Settings` / `default` / `PipelineOverrides` / `set!`
      四处，其中宏清单那份**漏写是静默的**（决策 258）
- [x] repair 的台账补上 `sanitize_command_line`——今天四条台账路径里唯独它漏了（`repair.rs:310`）
      → 脱敏落在收口里（`exec.rs` 对每条命令统一过 `sanitize_command_line`），repair 这一路不再
      自己写库。
- [x] 闸门的 pass/fail 判据**仍然是 exit code**，逐字不变；`gate-output-*.log` 的全文日志仍在
      （它是决策 211 一脉的取证物）
      → 两处闸门的 `full_log` 组装与落盘逐字保留（executor 那侧连「超时那条路不落全文、只记一行
      摘要」的分支也照原样搬进闭包）。
- [x] 闸门**不做改写**，并有测试钉住「闸门命令原样执行、台账不出现 `original_command`」
      → `repair.rs::the_repair_gate_never_rewrites_its_command` +
      `command_funnel.rs::a_source_system_command_records_no_original_command`。
- [x] 四门 + 交付说明，含「闸门行为变化逐点核对」：哪些从无上限变成有上限、哪些从漏杀变成杀干净

## 闸门行为变化逐点核对（落地时逐条实测）

从「无上限」变「有上限」：

1. **repair 闸门**：`.output().await` 裸调（无超时）→ 受 `test_command_timeout_sec`（默认 600）管，
   超时收成 `exit_code = -1`（闸门失败）。**这是本方案里唯一可由用户推翻的默认**（票 02 的 Comments
   原话）：真实项目若因此第一次变红，先看这条。
2. **执行器闸门**：此前虽有超时，但超时只**丢掉 future**、不杀进程；现在超时杀干净进程组。

从「漏杀」变「杀干净」：

3. **两处闸门**现在都进自己的进程组（`spawn_in_own_process_group`），超时时整棵树收掉——收口之前
   只有 agent 侧那两条路有这件本事。
4. **两处闸门**现在都回填 `process_group_id`：调度器那条超时收口（`scheduler.tick()`）对闸门也成立。

补上的另外两件（不是修 bug，是「两个实现」造成的落差）：

5. **心跳**：闸门命令现在也有心跳事件，「还在跑」在长测试套件上不再静默。
6. **脱敏**：repair 闸门的台账此前直接落原串，现在与其余三条路一致过
   `sanitize_command_line`。

**唯一一处有意的行为差**（决策 297 也记了）：执行器闸门**启动失败**那条路现在落一行
`exit_code = -1` 的收尾——此前那行台账停在 NULL 上（读侧表示「还在跑」），永远不闭合。

保持不变（逐字）：判据仍是 exit code；`gate-output-*.log` 全文日志仍在；闸门不改写。

## Comments

- 行为变化的诚实记账：repair 闸门从「无上限」变成 600s 上限，长测试套件可能**新出现**超时失败。
  这条是本方案里唯一可由用户推翻的默认——如实测发现某个真实项目因此变红，先在交付说明里点出来，
  再决定是否给它一个更宽的上限。
- **闸门为什么不挂改写**（实测判据，spec §2）：exit code 原样透传（假 `cargo test` 的 101、假 `pytest`
  的 1 都保住），但输出被重排成摘要——`pytest` 的 `assert 1 == 2` **丢了文件与行号**；运行器输出不是
  rtk 认得的样子时，输出会被换成**零行**或一句**误导性摘要**（`Pytest: No tests collected`）。
  闸门输出同时是模型改代码的唯一证据与落盘取证物，而一条命令只跑一次、拿不到「既过滤又原样」两份。
  给模型省 token 不能拿证据的面做交换。
