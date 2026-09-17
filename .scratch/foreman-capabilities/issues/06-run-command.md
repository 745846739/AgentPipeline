# 06: E 层 `run_command` 上线

**What to build:** 按决策 206 / 207 把 `run_command` 交给值班长：

- `auto` 档**直通**——直接执行并落 `kanban_node_commands`（会话归属由迁移 0012 给）；
- `ask` 档**生成提议**（走票 02 的表与票 03 的钮）；
- `deny` 档**不广告且拒执**。

**E 层不需要提议表做日志**：`auto` 档不生成提议，提议只服务 `ask` 档——「提议」与「审计」两件事不混。
域、`data` / `logs` 的 deny 前缀、命令日志与会话维度读取面、L2 卸载的空 task_id 守卫，全部由
`run-command-permissions/03` 提供。

**一条要写进交付说明的实话**：域检查对命令**几乎无安全价值**（命令自己 `cd` 就出去了）。
`auto` 档下经命令读走 `data/agentpipeline.db` 的明文密钥这条**无补偿**，已由决策 206 登记，
`docs/operations.md` 的改写由 `run-command-permissions/04` 负责。

**Blocked by:** `run-command-permissions` 02、03

**Status:** done

- [x] 三档行为各自可见（测试按档断言：广告集 / 直接执行 / 生成提议 / 拒绝）
- [x] `ask` 档下被拒绝或过期的命令**确实没跑过**（照 `crates/core/tests/foreman.rs:488-501` 的取证手法）
- [x] 命令日志落库并能从会话维度读出（`GET /foreman/commands?session=`）
- [x] persona、`FOREMAN_BASELINE`、工具纪律段按档位改写——不再说「不能执行命令」
- [x] 长输出的卸载落在会话维度；`{root}/tasks/.context` 不被污染（有测试）
- [x] 交付说明写明：本票落地后，182⑤ 当初排除 `run_command` 的那条理由**仍然成立**，由用户知悉后接受

## 交付

`run_command` 已进值班长的清单（`FOREMAN_TOOL_SPECS` 的 E 层），三档行为各有取证：

- **`auto` 直通**：命令真的跑（落 `kanban_node_commands`，`task_id` NULL、`session_id` 是当前班次），
  并可按会话维度读出（`GET /foreman/commands?session=`）。取证在 `tests/env_mode.rs` 的
  `an_auto_command_lands_under_the_session_and_nowhere_else` 与
  `crates/app/tests/api_contract.rs` 的 `pressing_the_button_actually_runs_the_command`。
- **`ask` 生成提议**：命令**确实没跑过**——取证看副作用（标记文件不存在、命令台账空行），
  再看同一条参数在按下那一刻真的跑起来（`the_ask_tier_command_waits_for_the_press`）。
- **`deny` 两侧**：广告集里摘掉（`tests/foreman.rs::the_deny_tier_removes_the_environment_layer_from_both_sides`）
  且硬发也被执行点拒（同一用例的后半，附「命令台账零行」）。

**一条要写进交付的实话：182⑤ 当初排除 `run_command` 的那条理由仍然成立。** 域检查对命令
几乎没有安全价值（命令自己 `cd` 就出去了），`auto` 档下经命令读走 `data/agentpipeline.db` 的明文
密钥这条**无补偿**——根本解是 OS 级沙箱（决策 19 修订 / 104 / 179 已登记，尚未落地）。
分档（缺省 `ask` = 每次按键）让这件事**可拦、可见**，不是边界。这一条已落进
`docs/operations.md` §12.14 的残余风险表与 §12.15 的沙箱出口，由用户知悉后接受。

另：长输出的卸载落在**会话维度**（`{root}/foreman/context/{session}/`），
`{root}/tasks/.context` 一个字都不写（`a_long_command_output_offloads_into_the_session_dimension`）；
persona 与工具纪律段按档位改写，不再说「不能执行命令」（`the_system_prompt_tells_the_truth_about_what_it_can_do_in_each_tier`）。
