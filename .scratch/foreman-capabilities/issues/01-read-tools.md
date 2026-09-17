# 01: A 层只读工具补齐 + 两张清单 + 「工具集恰为 N 个」断言改写

**What to build:** 值班长的工具从「两个只读台账工具」扩到**清单驱动**。本票**只加只读工具，一个写工具
都不加**（提议接缝是票 02）。

**A 层只读工具**：`read_board`（看板态势）/ `read_metrics` / `read_projects` / `read_stage_configs` /
`read_skills` / `read_providers`（**脱敏**——key 只回显存在性或尾部几位；库里是明文，决策 112）。
数据口径**沿用后端既有读数**，不新造一套。

**清单化**：`FOREMAN_TOOLS`（`pipeline/foreman.rs:60` 的两个字面量）扩成清单族里的只读部分；
`ForemanRunner::tool_defs()`（`foreman.rs:550-580`）从清单生成，**仍然不走 `effective_tools`**
（那条路会并入基线强制工具，含 `run_command` / `write_file`，正是本模块要挡掉的东西——
`foreman.rs:547-549` 的注释原话）。

**断言改写（本票最该先改的一条）**：`crates/core/tests/foreman.rs:309-330` 的
「工具集恰为约定的两个」改为「工具集恰为清单里的 N 个」，并**保留逐个 forbidden name 的反向断言**；
`tests/foreman.rs:521` 的 `assert_eq!(FOREMAN_TOOLS, ["read_task", "read_conversation"])` 同步。
这条断言就是安全边界本身（`foreman.rs:56-59`：「这是安全边界本身，不是配置项。任何『给值班长加个
工具』的改动都必须先改这里，从而在 diff 里显式可见」），**先改断言再加工具**。

**Blocked by:** None（可立即开始）

**Status:** done

- [ ] 六个只读工具可用，返回后端既有口径的数据（不新造读数）
- [ ] `read_providers` 脱敏（有测试断言响应不含明文 key）
- [ ] 广告集**从清单生成**；执行点的白名单与广告集**同源**（不许两处各写一份）
- [ ] `tests/foreman.rs` 两条断言改写为清单驱动 + forbidden 反向断言，且是在加工具**之前**改的
- [ ] 既有 21 条用例全绿（除断言本身的改写）
- [ ] persona 的工具纪律段同步描述新工具，且**不**描述任何写权限
- [ ] 本轮不引入任何写工具、不做提议（那是票 02 / 03）

## 交付

本票已落地（2026-09-17）。**先改断言、再加工具**，顺序在 diff 里看得见（两条断言改写与六个工具在
同一批，但断言那两处改成的是「清单驱动」的形状，加工具时它不需要再动）。

- **清单是唯一事实源**：`pipeline/foreman.rs::FOREMAN_TOOL_SPECS`（`[ForemanToolSpec; 8]`：名字 +
  `ForemanToolLayer` + 广告语 + 参数 schema 文本）。`FOREMAN_TOOLS` 由它**按下标生成**，
  `ForemanRunner::tool_defs()` 也由它生成——广告集与执行点白名单同源，两处不再各写一份名字。
  加一个工具 = 改这张表（安全边界本身的可见性保住了）。
- **六个只读工具**：`read_board`（全量看板 + 按状态计数，读的是同一张任务表）、
  `read_metrics`（复用 `metrics::*` 纯函数口径，不另写 SQL）、`read_projects`、
  `read_stage_configs`、`read_skills`（与 `GET /skills` 同一个 `discover`）、
  `read_providers`（走既有的 `list_providers_masked`）。
  顺带把「技能被谁声明」那个循环收进 core（`config::declared_skill_where`），端点与工具共用一份。
- **`read_providers` 脱敏有安全断言**：判据打在**回灌给模型的那份文本**上（第二轮请求的 messages），
  不是打在工具返回值上——只有前者能证明模型看不到明文。
- **断言改写**：`tests/foreman.rs` 的广告集断言改成「与清单逐字同序」+ 逐个 forbidden 名字的
  反向断言（9 个名字，含 `read_file` / `write_file` / `run_command` / `Skill`），
  并新增「清单里此刻只有只读工具」与「参数 schema 是合法 JSON、名字不重复」两条。
- **persona 的工具纪律段按清单生成**（不再手写工具名），且写工具为空时明确写「你没有写权限」。
- **本轮不含任何写工具**：`foreman_tool_names(Write)` 为空，有断言钉住。
