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

**Status:** ready-for-agent

- [ ] 三档行为各自可见（测试按档断言：广告集 / 直接执行 / 生成提议 / 拒绝）
- [ ] `ask` 档下被拒绝或过期的命令**确实没跑过**（照 `crates/core/tests/foreman.rs:488-501` 的取证手法）
- [ ] 命令日志落库并能从会话维度读出（`GET /foreman/commands?session=`）
- [ ] persona、`FOREMAN_BASELINE`、工具纪律段按档位改写——不再说「不能执行命令」
- [ ] 长输出的卸载落在会话维度；`{root}/tasks/.context` 不被污染（有测试）
- [ ] 交付说明写明：本票落地后，182⑤ 当初排除 `run_command` 的那条理由**仍然成立**，由用户知悉后接受
