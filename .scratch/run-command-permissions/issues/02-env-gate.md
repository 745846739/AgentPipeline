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

**Status:** ready-for-agent

- [ ] 三档在执行点分叉，且 **`auto` 档与今天的行为逐字相同**（有测试）
- [ ] `ask` 档生成提议而不执行；**被拒绝或被过期的命令确实没跑过**——照
      `crates/core/tests/foreman.rs:488-501` 那条「越权命令不得真的跑起来」的取证手法写断言
- [ ] `deny` 档在**广告与执行两处**都挡
- [ ] 两段清单是真常量（`FOREMAN_ENV_TOOLS` / `FOREMAN_SERVICE_WRITE_TOOLS`），加工具的改动必须
      先改这里，从而在 diff 里显式可见
- [ ] 只读台账工具不受档位影响（有测试）
- [ ] 不是逐工具打补丁：判断落在**一处**（层 → 档位的映射表），加一个新环境层工具不需要碰分叉逻辑
