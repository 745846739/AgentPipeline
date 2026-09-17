# 03: 诊断包工具（含工具集冻结断言改写）

**What to build:** 给值班长**一个**新只读工具，一次调用拿到定因所需的全部证据。

今天它走不动诊断树：`read_task` 只回标题 / 状态 / 游标（**不带 `run_id`**）/ `allowed_actions` /
`stalled`（`crates/core/src/agent/tools.rs:915-934`）；**没有任何工具能列出 run**；命令台账
（`kanban_node_commands`）、闸门输出（`gate-output-{stage}.log`）、阶段产出
（`kanban_stage_outputs.metadata_json`）都对它不可见；`read_metrics` 只有全局聚合
（`tools.rs:1006-1036`），拿不到单任务的每 run 耗时与 token。

诊断包要给出：

- 该任务的全部 run：`id` / stage / node / attempt / agent_type / status / 耗时 / token /
  `error` / `prompt_template_hash` / **`process_group_id` 是否为空**（后面这一项是判「超时杀不杀得掉」
  的唯一线索，见票 04）
- 命令台账（含 `exit_code` / `duration_ms` / `stdout_path`）与 `gate-output-{stage}.log` 的路径与尾部
- 阶段产出与验收标准（`metadata_json`，含 architect 的 `acceptance_criteria`）
- pending 原因的 `message` 与 `diagnostic` 原文
- 组装后的 prompt 原文（票 02 落地后）

**为什么不扩 `read_task`**：`read_task` 是高频、便宜的看状态；诊断包低频，一击就撞 12k 上限。
混在一起会让「看一眼任务状态」这件事开始烧 12k 字符，而它在**每一轮值守**里都会被调用。

**Blocked by:** 01, 02

**Status:** ready-for-agent

- [ ] **先改断言**：`crates/core/tests/foreman.rs:598-632` 的「工具集恰为这 17 个」按顺序钉着，
      加工具前先改它；`foreman.rs:56-59` 的原话「这是安全边界本身，不是配置项」
- [ ] 新工具加进 `FOREMAN_TOOL_SPECS`（`crates/core/src/pipeline/foreman.rs:94`），
      **广告集与执行点白名单同源**（不许两处各写一份名字）
- [ ] 粒度守决策 207④：**一族一个工具**，不拆成五个
- [ ] 结果截断沿用 `FOREMAN_TOOL_RESULT_MAX_CHARS`（12k），**但截断要留标记**；
      重点证据（失败那一轮的 error + 最后一次工具调用）**优先保证在截断后的前 12k 里**
- [ ] 新增用例：造一个失败的任务，断言诊断包能一次给出失败 run 的 `error`、
      `process_group_id` 为空、命令台账条数、闸门输出路径——**四项都在同一次调用的返回里**
- [ ] persona 里的工具纪律段按清单自动更新（它已是从清单生成，不需手写工具名）

## 备注

票 03 是「能分析出代码 / prompt / 环境问题」的**唯一入口**。它的验证必须打在
「一次调用够不够定因」上，不是打在「字段齐不齐」上。
