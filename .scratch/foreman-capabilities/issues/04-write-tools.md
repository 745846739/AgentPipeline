# 04: C 层写工具接进确认钮

**What to build:** `write_file` / `edit_file` 接进票 02 的接缝。

- `ask` 档：不执行，生成提议（票 02）；`auto` 档：直通（决策 206）；`deny`：不广告且拒执。
- **域**由 `FileToolPolicy` 给：值班长的域 = `home.root()`，`data` / `logs` 按路径前缀 deny
  （`run-command-permissions/03`）；流水线阶段 agent 仍限任务工作区（决策 207 不动那条）。
- 于是 `skills/` 与 `market-repos/` 在值班长手里**可达**——那是「像 zcode 一样」的应有之义；
  要收紧就在 deny 前缀里加一行（决策 207 已把这句话写进规格）。

**Blocked by:** 03

**Status:** done

- [x] 两个工具接进接缝；`ask` 档下不执行、生提议
- [x] 域与 deny 前缀生效（`data/` 下读写都被拒，有测试）
- [x] 提议执行成功后**文件真的变了**，且结果回灌成轮
- [x] 工具归属与票 06 一致（同属 `FOREMAN_ENV_TOOLS`，档位语义相同）
- [x] 写入的审计可追：提议 id → 执行 → 落盘的路径与内容摘要能在库里对上

## 交付

- `write_file` / `edit_file` 进 `FOREMAN_TOOL_SPECS` 的 C 层（`ForemanToolLayer::Write`），于是白名单、
  广告集、档位筛选三处**同源**地拿到它们。参数与既有内置工具**逐字同形**（`path` / `content`；
  `path` / `old_text` / `new_text`——`edit_file` 的字段名照 `docs/agents.md` 与实现，不发明第二套）。
- 执行侧走值班长自己的执行器（`foreman_tooling(..., ForemanMoment::ConfirmedPress)`）：**与对话轮
  同一个执行器**（同一份文件策略、同一个出口策略、同一份命令记录），只把确认闸关掉——提议生成时
  已经走过一次闸，执行时再拦一次会自己吃掉自己。白名单按**当前**档位重算，故档位在提议之后收紧到
  `deny` 时那条提议按不下去。
- 取证：`tests/env_mode.rs` 的 `the_ask_tier_proposes_for_both_file_writers`（两个工具各提一条，
  文件没变）、`the_confirmed_press_executes_what_the_ask_tier_proposed`（同一份参数在按键那一刻落盘）、
  `the_foreman_domain_covers_the_home_but_not_data_or_logs`（`data/` 与 `logs/` 下读与写都拒）；
  `crates/app/tests/api_contract.rs` 的 `pressing_the_button_actually_writes_the_file`（**文件真的变了**
  + 提议 id → 执行 → 落盘路径与字节数能在库里对上）与 `a_confirmed_write_into_the_key_store_is_still_refused`
  （域补偿在执行那一刻同样生效，且失败的提议**不消耗**）。
- 顺带一处收紧（已标进决策 207 的落地标注）：`edit_file` 的 `old_text` 为空时**拒绝**——空串会让
  替换变成「往文件开头插一段」，而回执说 `success`。
