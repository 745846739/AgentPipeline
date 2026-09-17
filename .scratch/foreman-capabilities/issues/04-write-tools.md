# 04: C 层写工具接进确认钮

**What to build:** `write_file` / `edit_file` 接进票 02 的接缝。

- `ask` 档：不执行，生成提议（票 02）；`auto` 档：直通（决策 206）；`deny`：不广告且拒执。
- **域**由 `FileToolPolicy` 给：值班长的域 = `home.root()`，`data` / `logs` 按路径前缀 deny
  （`run-command-permissions/03`）；流水线阶段 agent 仍限任务工作区（决策 207 不动那条）。
- 于是 `skills/` 与 `market-repos/` 在值班长手里**可达**——那是「像 zcode 一样」的应有之义；
  要收紧就在 deny 前缀里加一行（决策 207 已把这句话写进规格）。

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] 两个工具接进接缝；`ask` 档下不执行、生提议
- [ ] 域与 deny 前缀生效（`data/` 下读写都被拒，有测试）
- [ ] 提议执行成功后**文件真的变了**，且结果回灌成轮
- [ ] 工具归属与票 06 一致（同属 `FOREMAN_ENV_TOOLS`，档位语义相同）
- [ ] 写入的审计可追：提议 id → 执行 → 落盘的路径与内容摘要能在库里对上
