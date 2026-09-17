# 02: 环境层拦截——执行点按档位分三路

**What to build:** `ToolExecutor::execute`（`agent/tools.rs:337-353`）是唯一执行点，今天已有两道闸
（`with_allowed_tools` 的名单、egress 的出口分类）。本票加第三道：按 `(工具属于哪一层, 当前档位)`
决定 **直接执行 / 生成提议 / 拒绝**。

- `auto`：直接执行（与今天一致，落 `kanban_node_commands`）；
- `ask`：**不执行**，生成一条提议（走 `foreman-capabilities` 02 的表与端点、03 的钮）；
- `deny`：拒绝（不广告已经挡掉大多数，这里是兜底）。

**环境层的工具集合**（`FOREMAN_ENV_TOOLS`）：`run_command`、`write_file`、`edit_file`，加上 B 层的
`read_file` / `list_dir` / `Skill` / `spawn_sub_agent`。**本服务写接口不属于这一层**——它走
`FOREMAN_SERVICE_WRITE_TOOLS`，恒提议、不读档位（决策 206）。

**Blocked by:** `foreman-capabilities` 02、03（`ask` 档要落到那张提议表与那颗钮上）

**Status:** done

- [x] 三档在执行点分叉，且 **`auto` 档与今天的行为逐字相同**（有测试）
- [x] `ask` 档生成提议而不执行；**被拒绝或被过期的命令确实没跑过**——照
      `crates/core/tests/foreman.rs:488-501` 那条「越权命令不得真的跑起来」的取证手法写断言
- [x] `deny` 档在**广告与执行两处**都挡
- [x] 两段清单是真常量（`FOREMAN_ENV_TOOLS` / `FOREMAN_SERVICE_WRITE_TOOLS`），加工具的改动必须
      先改这里，从而在 diff 里显式可见
- [x] 只读台账工具不受档位影响（有测试）
- [x] 不是逐工具打补丁：判断落在**一处**（层 → 档位的映射表），加一个新环境层工具不需要碰分叉逻辑

## 交付

- 判据收在**一处**：`agent::tools::gate_decision(name, env_mode) -> Execute | Propose | Refuse`
  （分成三态而不是一个布尔——「要按键」回答不了「然后呢」：拦下来是生成提议还是拒绝，取决于有没有
  提议通道）。`ToolExecutor::confirm_gate` 按它分派，`needs_confirmation` 是它的一个读数，
  测试与实现因此不会各说一套。
- **`deny` 从广告的工具表里摘掉**：`denied_by_tier` 是唯一谓词，阶段节点的 `tool_defs` 与值班长的
  `foreman_available_tools` 都调它（两处各写一份的后果是「一侧摘掉了、另一侧还广告着」）。
  执行点那一道仍在：`tests/env_mode.rs::the_deny_tier_refuses_and_leaves_nothing_behind` /
  `the_deny_tier_covers_commands_too`，值班长一侧的完整取证在
  `tests/foreman.rs::the_deny_tier_removes_the_environment_layer_from_both_sides`（广告集摘掉 +
  硬发也被拒 + 命令台账零行 + 不生成提议）。
- **只读台账工具不受档位影响**（`ledger_read_tools_are_outside_the_tiers`）：`deny` 收的是「能碰机器」
  的手，不是「能读台账」的眼。`ask` 同理只收**动手**那一半——`read_file` / `list_dir` / `Skill`
  在 `ask` 档下直接执行，「读一个文件也要人按键」是把确认钮变成噪声。
- `auto` 档与档位出现之前**逐字相同**（`the_auto_tier_writes_the_file_like_before_the_tier_existed`）。
- `ask` + 没有提议通道（流水线节点的现场）**拒绝**而不是静默降级：
  `the_ask_tier_without_a_proposal_channel_refuses`，同时 `executor.rs` 在阶段被配成 `ask` 时
  打一条 `tracing::warn!`；另有「不许把真实阶段配成 ask」那道门（见票 01）。
