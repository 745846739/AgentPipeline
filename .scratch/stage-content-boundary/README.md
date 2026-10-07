# stage-content-boundary：阶段内容越界的写入面收窄与 develop 申报比对

## 背景

106 上三个 dogfood 任务（决策 374–393）的实证越界集中在**流转层**与**归因层**，
已由决策 370/387/391 收口。本票处理的是由此暴露的**内容层**缺口：各阶段 agent
「做什么」的边界只有 prompt 约定，没有代码防护——所有 agent 共享同一套全量工具
（`crates/core/src/agent/client.rs:214-240`）与同一个 worktree+任务目录文件域
（`pipeline_file_policy()`，`file_policy.rs:103-115`），review 理论上可以改代码、
develop 漏报文件可以让 review 盲审。

2026-10-07 grilling 会话（Q1–Q11）完成设计裁决，本文档记录裁决与施工票。

## 事实基线（探查结论，2026-10-07）

| 事实 | 出处 |
|---|---|
| review 模板只授 `read_file`/`write_file`/`submit_metadata`，唯一写入是任务目录下 `review-report.md`，变更清单来自 develop 元数据申报（非 git diff），validate_output 只读元数据 | `templates.rs:259-287`、`model_request.rs:538-541`、`executor.rs:1010-1036` |
| test-design 三个节点零出现 run_command，唯一写入是 `test-scenarios.md`，validate_input 无写入 | `templates.rs:173-218` |
| 各设计/评审阶段产出文件的权威定义是 `ProductTarget` 表 | `continuation_brief.rs:66-89` |
| `write_root_for` 已把四个设计/评审阶段的写根锁到任务目录 | `tools.rs:392-411` |
| `FileToolPolicy` 是「允许根+拒绝名单」形状，表达不了正向白名单 | `file_policy.rs:21-31, 126-173` |
| run_command 有唯一执行点，`ctx.stage` 在手，egress（决策 179）是「启动前判定+拒绝落台账」的现成形状 | `tools.rs:2407, 2422-2444, 381` |
| develop 被模板硬性要求写单元测试并自跑 `git add/commit`（决策 391 提交契约），review 打回可含测试质量修改（决策 133）——develop 写测试**不是**越界 | `templates.rs:235-257`、`decisions.md:811-813` |

## grilling 裁决（Q1–Q11，2026-10-07）

| # | 问题 | 裁决 |
|---|---|---|
| Q1 | 范围 | 内容越界防护为主；归因（392）与验收惯例只作约束带入 |
| Q2 | 根本策略 | C：只收「合法写入面可枚举」的阶段，develop 保持全量 |
| Q3 | 排序 | **先决策 392 落地，后本票实施** |
| Q4 | 验收 | 四件套惯例：立票 → 确定性检查 → 单测/e2e 断言钉死 → 进 docs/testing.md 用例目录 |
| Q5 | shell 绕过 | C：模板零依赖 run_command 的阶段整体禁用（实证支撑后从 B 上调） |
| Q6 | 落法 | B：本票写全施工细节，实施随 392 之后 |
| Q7 | 收窄面 | C：全阶段统一规则（见下），不留在四个单文件阶段 |
| Q8 | 禁用层 | C：双层——工具定义层不给 + 执行层兜底（`tools.rs:713`「只靠 tool 定义约束是纸糊的」） |
| Q9 | 申报缺口 | B：develop 申报 vs 实际 diff 的机械比对**纳入本票** |
| Q10 | 比对失败路由 | A：develop_code_gate 内走既有 Retry 回灌，不新增 `GateFailureKind`、不加路由分支 |
| Q11 | 比对口径 | C：单向只查漏报 + 可配置噪音过滤 glob（默认含 lockfile 类） |

## 统一规则（Q7=C 的完整表述）

> **任务目录的 agent 写入一律白名单到该节点 `ProductTarget` 声明的产出文件；
> worktree 写入仅 develop 与 test 放开。**

按阶段展开：

| 阶段 | 任务目录写入 | worktree 写入 | run_command |
|---|---|---|---|
| architect-design | `design.md` | 拒 | 保留（留观，见下） |
| develop-design | `dev-plan.md` | 拒 | 保留（留观） |
| test-design | `test-scenarios.md` | 拒 | **双层禁用** |
| review | `review-report.md` | 拒 | **双层禁用** |
| develop | 拒（系统侧产物不走 agent 工具） | 全域放开 | 保留（提交契约依赖） |
| test | `test-report.md` | 全域放开 | 保留（跑测试命令） |

「留观」注：architect-design / develop-design 的模板同样大概率零依赖
run_command，本票实施时顺带核实模板；若确认零依赖，禁用与否另走决策，不在本票扩面。

## 票

- [01-stage-write-whitelist](issues/01-stage-write-whitelist.md)
  ——`FileToolPolicy` 增正向白名单 + 按 `ProductTarget` 装配（决策号预留 **395**）。
- [02-run-command-stage-deny](issues/02-run-command-stage-deny.md)
  ——review/test-design 双层禁用 run_command（决策号预留 **396**）。
- [03-develop-declaration-check](issues/03-develop-declaration-check.md)
  ——develop_code_gate 申报单向比对 + 噪音过滤（决策号预留 **397**）。

## 实施排序与边界

- **前置**：决策 392（gate-failure-attribution 票）落地合入后再开工——避免
  `run_code_gate` / 失败归因区域两批改动互相踩。
- **不做**：review 对 develop 申报清单的语义可信（改不了，靠 03 的机械比对兜）；
  shell 完全沙箱（file_policy 自认非沙箱的定位不变，本票只收窄到「模板授过的动作」）；
  architect/develop-design 的 run_command 禁用（留观）。
- **决策修订标注**：395 扩展决策 283 的操作环境域（prompt 纪律 → 代码策略）；
  397 扩展决策 391 的 develop 闸门判据（形状同「零提交守卫」），不触决策 85 的
  失败分类。

## 落地状态

**已实施**（2026-10-07，本批）。与票面的两处偏差（均已记入代码注释）：

1. 决策 392 的实装由**并行会话在同一工作树**完成，本票与其在同树合流（票面原定
   「392 合入后再开工」，实际同时施工；共享文件 executor.rs 的两批改动相互独立）。
2. 票 03 的 dirty 面**收窄为已跟踪改动**（排除 untracked）：untracked 文件不进任务
   分支 diff、也就不进 review 的评审面与 merge 的合并面，算漏报只会制造无法通过
   补申报消除的假红（`declaration_check` doc 注释）。

落地清单：`FileToolPolicy.allow_writes` + `stage_write_scope` 装配（395）、
`tool_defs` 过滤 + `run_command` 执行点拒绝 + 台账退出码 `STAGE_TOOL_DENIED_EXIT_CODE`
（396）、`declaration_check` + `undeclared_changes` 单向比对 + 噪音过滤
`declare_ignore_globs` + facts 段注入（397）。单测 core 779 全绿；L2 515 全绿；
L3 244 全绿；e2e 84 全绿（新增 `stage_boundary.rs` 4 条）；clippy `-D warnings` 干净。
既有夹具的「写了文件却空申报」6 处已改为诚实申报（smoke / restart_recovery /
implementation_ok / to_review / conflicts / crash_recovery / L2 implementation_scripts）。
