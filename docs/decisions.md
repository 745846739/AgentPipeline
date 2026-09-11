# 已确认的设计决策

> 拆分自 agent-pipeline.md（原 §13）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

## 13. 已确认的设计决策

本节记录所有已确认的设计决策（2026-09-11）。**决策 89–112 来自第四次评审（g4），113–120 来自第五次评审（g5），121–132 来自第六次评审（g6）**；10 / 19 / 26 / 42 / 46 / 56 / 69 / 71 / 73 / 90 / 93 / 98 / 99 被后续决策修订，修订关系在各行显式标注。

| # | 决策 | 结论 | 来源 |
|---|---|---|---|
| 1 | 依赖任务分支策略 | 基于 `default_branch`，前置任务合入后后续任务才开始 | — |
| 2 | human review 阻塞行为 | 阻塞（pending 等人工操作） | — |
| 3 | worktree 回收策略 | 代码合入后立即删除 | — |
| 4 | 通知渠道 | SSE 作为默认通知方式 | — |
| 5 | LLM 调用执行方式 | node 内部直接调用 rig，petgraph 只负责调度 | — |
| 6 | 本地合并 | 无远程 PR，用户在 GUI 审核 diff 并点击合入 | — |
| 7 | prompt 管理 | 默认嵌入二进制，用户可通过 `~/.agentpipeline/prompts/` 覆盖 | — |
| 8 | ctrl+c 优雅关闭 | 等当前 node 完成后退出，不回滚；第二次 ctrl+c 强制终止 | — |
| 9 | 子代理最深层级 | 保持一层 | — |
| 10 | API Key 管理 | **已由决策 112 修订**：provider 密钥**明文**存 `providers.api_key`，依赖 `~/.agentpipeline` 目录权限（§12.14）。原"AES-256-GCM 加密存 `api_keys` 表"的方案已废弃 | — |
| 11 | API 层 | REST API + SSE（axum），后期可替换为 Tauri IPC | — |
| 12 | git2 !Send/!Sync | `tokio::task::spawn_blocking` 包装所有 git2 调用 | — |
| 13 | 数据库迁移 | sqlx migrations | — |
| 14 | sync-check 行为 | 两种结果：都通过→proceed，任一方有 blocker→backtrack | g1-R1 |
| 15 | 测试代码语言 | Rust（`*_test.rs`），在 worktree 内 `tests/` 目录 | g1-R1 |
| 16 | 前端方案 | 纯 Web（Svelte + TypeScript + Vite），Tauri 延后 | g1-R1 |
| 17 | 数据库文件名 | `agentpipeline.db` | g1-R1 |
| 18 | 预算功能 | v1 不实现，移除所有预算相关功能 | g1-R2 |
| 19 | 沙箱 | **已由决策 104 修订**：v1 **不做系统级沙箱**——只有文件工具层面的 `FileToolPolicy`（workdir_bound + deny_paths + realpath + 拒绝写符号链接），shell 不受限 | g1-R4 |
| 20 | 技能 | 技能全部走用户配置，`mandatory_skills` 默认空；rtk / codegraph 仅作示例配置（被决策 47 修订，保留仅为 MCP 延后的记录） | g1-R4 |
| 21 | 并发任务限制 | 可配置，默认 5 | g1-R6 |
| 22 | Provider 配置 | 通过界面配置，不在 config.toml 中 | g1-R6 |
| 23 | Merge proposal | 只有"合入"和"返回修改"，移除"拒绝"按钮 | g1-R6 |
| 24 | 项目创建 | agent 静态分析（语言、测试框架、AGENTS.md 等），用户确认（探测部分改由代码实现，见决策 78） | g1-R5 |
| 25 | review_mode | v1 只支持 `agent` 和 `human`，`human_if_risk` 延后 | g1-R7 |
| 26 | Agent 工具 | **已由决策 45 修订**：7 个内置工具（write_file / edit_file / read_file / delete_file / list_dir / run_command / submit_metadata）+ 扩展工具 spawn_sub_agent（默认关闭） | g1-R7 |
| 27 | 循环依赖检测 | API 层（POST /tasks）检测，返回 400 | g1-R5 |
| 28 | AGENTS.md 加载 | 从 worktree 根目录读取，不存在时用默认上下文（非空，见决策 51） | g1-R5 |
| 29 | 项目关联模型 | 以本地仓库路径为唯一事实来源；删除"关联 git remote URL"表述与中心化 `~/.agentpipeline/repo/` 目录 | g2-R1 |
| 30 | 结构化元数据存储 | `kanban_stage_outputs` 增加 `metadata_json` 列与 `UNIQUE(task_id, stage, output_type)`，文件路径与路由元数据同表 upsert | g2-R1 |
| 31 | 目标项目测试语言 | 流水线自身用 Rust；目标项目的测试语言与命令由 `test_framework` 决定，prompt 使用 `{test_command}` / `{test_file_convention}` 模板变量（修订决策 15） | g2-R1 |
| 32 | MCP | v1 不实现，整节移入附录"v2 预留"；`SystemBaseline` 删除 `mandatory_mcp` | g2-R1 |
| 33 | 重试分层语义 | `tool_retry_max` 管 loop 内工具失败；`agent_retry_max` 管节点内 loop 整体失败（含元数据解析/校验失败、超时）；`validate_retry_max` 管 execute ↔ validate_output 循环；超时先走 agent 重试，耗尽才 `pending(timeout)` | g2-R1 |
| 34 | pending / stalled / archived 表示 | `status = pending` 表示等人，`pending_reason` 记原因；`stalled` 为 pending 上的超时标志字段；`archived` 为 `archived_at` 时间戳，不新增 TaskStatus 枚举值 | g2-R1 |
| 35 | resume 动作模型 | `Action` 改为 `{action: "continue" \| "skip" \| "goto", target_stage?, target_node?, input?}`；允许动作集由后端按 `pending_reason.type` 下发（扩充见决策 69） | g2-R1 |
| 36 | executor 并发保护 | 每任务单执行者（进程内 Mutex 集合 + DB 乐观锁）；`max_concurrent_tasks` 在 scheduler `start_task` 处准入 | g2-R1 |
| 37 | 集成测试归属 | 随任务合入 `default_branch`；硬规则：test 阶段产出必须全部通过，`merge.execute` 合入前重跑单元 + 集成测试；`review-report` 记录"集成测试未做语义评审" | g2-R1 |
| 38 | 阶段元数据 schema | 每阶段一个 Rust serde 结构体，用 schemars 派生 JSON Schema 动态生成 `submit_metadata` 的 parameters；无独立 schema 文件 | g2-R2 |
| 39 | 条件边实现 | 显式 Rust 路由函数集中 `pipeline/routes.rs`，key 为 (stage, node)，不用声明式表达式语言 | g2-R2 |
| 40 | 测试代码评审 | 不新增 test-review 阶段；阶段数维持 10，改由决策 37 的硬规则覆盖 | g2-R2 |
| 41 | 基础分支 | 统一用 `project.default_branch`；有 remote 时 fetch 后以 `origin/{default_branch}` 为基准，无 remote 用本地分支；`{base}` 为 init 时该分支 HEAD | g2-R2 |
| 42 | Task 记录创建 | 由 `POST /tasks` API 层创建（含循环依赖检测与初始状态判定——初始状态后由决策 98 修订为 queued / waiting）；init.execute 只建 worktree 并推进 stage | g2-R2 |
| 43 | validate_attempts 重置 | 任何跨阶段跳转（normal next / kickback / backtrack / 打回）重置为 0；语义为"当前阶段内 validate_output 打回 execute 的次数" | g2-R2 |
| 44 | merge 合入执行位置 | 用户批准后 resume 重新进入 `merge.execute`，检测到已批准则执行真正的合并 | g2-R2 |
| 45 | 内置工具集 | 7 个内置工具（write_file / edit_file / read_file / delete_file / list_dir / run_command / submit_metadata）；`spawn_sub_agent` 为可开关扩展工具，默认关闭；`offload_threshold_tokens` 默认 4000（**由决策 110 成为唯一的工具结果阈值**）（修订决策 26） | g2-R2 |
| 46 | 模型上下文窗口 | 随 `providers` 表的一行存 DB（界面可改），内置注册表提供默认值；从 config.toml 移除 `context_window_size`。**由决策 111 补充**：阶段只引用 `provider_id`，不再单列 model，`context_window` 查找路径唯一 | g2-R2 |
| 47 | mandatory_skills | 默认空，skills 全部走用户配置；rtk / codegraph 作为示例配置写入文档；skill 不存在时 fail fast | g2-R2 |
| 48 | 项目分析 | 实现为伪阶段 `project_analysis`（复用 L2 阶段配置与白名单校验，无 checkpoint / pending），由 `POST /projects/analyze` 触发 | g2-R2 |
| 49 | pending 允许动作 | 后端按 pending type 返回 `[{action, label, requires_input, target?}]`，前端纯渲染（key 与分层见决策 69） | g2-R2 |
| 50 | 对话 agent | v1 不实现，延后 v2（自然语言创建 kanban 任务，复用现有对话窗口 + `create_task` 工具） | g2-R3 |
| 51 | AGENTS.md 注入 | 拼入 system prompt 固定段落，顺序 `[基线前言][AGENTS.md][persona][格式规则]`；不存在时注入非空默认上下文（项目根路径 + 语言/测试框架 + "本仓库无 AGENTS.md"） | g2-R3 |
| 52 | 测试文件布局 | 按目标项目语言惯例放置（Rust 集成测试在 `tests/` 根、单元测试内联），由 `test_framework` 决定；用元数据字段 `unit_test_files` 区分 | g2-R3 |
| 53 | 冲突检测判据 | 比较规范化后的 `affected_files` 集合，有交集即 `conflict_wait`；`conflict_overlap_threshold` 默认 0；真实冲突由 merge 阶段兜底（判定细节见决策 71） | g2-R3 |
| 54 | ctrl+c 优雅关闭 | 第一次 SIGINT：停止派发新任务，当前节点跑完后在节点边界退出，checkpoint 停在节点级；第二次立即退出；退出时不改任务状态（保持 running） | g2-R3 |
| 55 | 调度器职责 | 10s tick 做超时 / 冲突恢复 / 依赖启动 / 并发准入 / pending 提醒 / stalled 标记；会话过期清理与指标聚合放小时级维护任务；补 `tick_interval_sec` 等配置 | g2-R3 |
| 56 | config.toml 边界 | 只保留 `[server]` / `[pipeline]` / `[logging]` / `[prompts]`；移除 `default_model` / `context_window_size` / `node_max_tokens`；未配置 provider 时创建任务返回明确错误。**由决策 111 补充**：`stage_configs.model` 一并删除 | g2-R3 |
| 57 | dependency_failed 恢复 | 依赖任务被重试转回 running 时，scheduler 将该 pending 任务清 pending 并退回 waiting | g2-R3 |
| 58 | 多项目目录 | worktree / task 目录不加 project_id 维度，前端按 project_id 过滤看板 | g2-R3 |
| 59 | 合入顺序 | 批准 → 应用到 default_branch（ff 优先，退化 `--no-ff`）→ status=merged → done → 清理 worktree / 分支；"返回修改"清 pending 回 develop.execute 且保留 worktree | g2-R3 |
| 60 | 语义重复检测 | 两层：① architect 元数据 `new_symbols` 符号名交集 → conflict_wait；② 模块路径重叠但符号名不重合时同步调 `conflict_check` 伪阶段，高风险 → `pending(user_decision)`（`context.kind = duplicate_risk`）；跨模块语义重复为已知风险 | g2-R4 |
| 61 | 脏工作区处理 | 建 worktree 时记录警告不阻塞；合入时目标分支被检出且不干净 → `pending(user_decision)`，不自动 stash；非 ff 用 `--no-ff`；`allow_dirty_worktree_merge = false`；非 git 仓库拒绝创建项目；unborn HEAD 明确报错 | g2-R4 |
| 62 | validate_output 职责 | develop / test 的 validate_output 改纯代码（系统跑测试 + 读元数据路由），删除其 agent prompt；失败根因由 execute 的 agent 通过 `failure_cause: test_issue \| code_issue` 给出；architect / develop-design / test-design 的 validate_output 保持 agent | g2-R4 |
| 63 | 会话记录粒度 | 每个节点尝试一行（`kanban_node_runs` 与 `kanban_node_conversations` 1:1），loop 内多轮调用累积进同一 `messages_json`；子代理的归属见决策 77。**由决策 99 / 100 收窄**：1:1 只对"调用 LLM 的 run"成立——纯代码节点有 run 无会话，伪阶段则 run 与会话都有 | g2-R4 |
| 64 | 超时时钟 | 取自 `kanban_node_runs.started_at`；超时适用于所有跑 agent loop 的节点；纯代码阶段由 `tool_timeout_sec` + 阶段级超时兜底；`updated_at` 不参与超时判定（阈值已由决策 66 的双阈值取代） | g2-R4 |
| 65 | 离线通知渠道 | v1 只做 SSE 应用内通知（含 cooldown / quiet_hours / pending 提醒 / stalled 高亮）；Webhook / 邮件 / 飞书延后 v2 | g2-R4 |
| 66 | 动态超时 | 两层：`node_idle_timeout_sec`（默认 300，以流式 token / 工具与命令活动为心跳）+ `node_max_duration_sec`（默认 1800 绝对上限）；超时杀整个进程组；系统测试命令用 `test_command_timeout_sec`（默认 600）；有效值 = 节点 > 阶段 > 全局；自适应 P50/P90 仅告警，`adaptive_timeout_enabled = false` | g2-R5 |
| 67 | 冲突比对配置 | 注册 `conflict_check` 伪阶段，prompt 在 `prompts/conflict_check/compare.md`，默认用 validate 档位便宜模型，受 `supported_adapters` 校验（决策 103）；开关 `semantic_conflict_check = true` | g2-R5 |
| 68 | 并行分支保留 | **保留** develop-design 与 test-design 并行，不改串行。执行状态改由游标模型承载，配套决策见 #80–84 | g3-A1 |
| 69 | 动作模型分层 | `ResumeRequest` 只承载恢复动作 `continue \| skip \| goto`；取消 / 拆分 / 换模型 / 放弃合入等为旁路动作，走各自专用 API，`allowed_actions` 以 `kind: "resume" \| "side_effect"` 区分；下发 key 由 `pending_reason.type` 改为 `(type, context.kind)`；`goto` 落点用 `entry_node(stage)` 查表（architect/develop-design/test-design → `validate_input`，其余 → `execute`）。**由决策 93 修订**：`skip` 落点单独成表，不复用 `entry_node`（architect-design 会分裂，并行分支不得越过 join）。**由决策 132 修订**：「放弃合入」因无配对端点移出动作集 | g3-A2 |
| 70 | failed 的唯一入口 | 任务进入 `failed` 的唯一途径是用户在 pending 卡片选择"终止任务"（旁路动作，决策 69）；其余情况任务永远停在 pending 等人。`retry_exhausted` 卡片给三个动作：重试 execute / 强制进入下一阶段 / 终止任务。`failed` 可经"重试"回到 init | g3-A3 |
| 71 | 冲突检测判定细节 | ① 活跃任务 = `status ∈ {running, pending, waiting, queued}`（**含 `queued`**，决策 98 引入该状态后同步补充；queued/waiting 尚无 architect 产出，比对自然为空，决策 120），已终态或已归档不参与比对；② 符号判重用 `(module_path, name)` 组合，纯 `name` 重合降级为 warning；③ 环消除：仅 `created_at` 较晚者进入 `conflict_wait`（同秒以 `id` 字典序大者让步），避免互等死锁。**由决策 102 补充**：`context.conflict_task_ids` 存**全部**冲突任务，全部终态后重跑第一层比对再决定恢复 | g3-A4 |
| 72 | merge 阶段重入判定 | `merge_result` 增加 `approval: "none" \| "pending" \| "approved" \| "returned"` 字段（落 `kanban_stage_outputs.metadata_json`）：`none` / `returned` 走阶段 A 生成 proposal，`approved` 走阶段 B 执行合入，"返回修改"写 `returned` | g3-A5 |
| 73 | 合入执行方式 | 阶段 B 统一在**临时 worktree**（`git worktree add --detach` 指向 `default_branch`）内执行 ff 或 `--no-ff` merge，完成后移除；`update-ref` 仅保留给确定可 ff 的场景（原"未被检出则 update-ref"无法生成 merge commit，已废弃）。**由决策 97 修订**：detached worktree 里的 `--no-ff` merge commit 不属于任何分支，必须用 `update-ref` 显式写回 `default_branch`（ff 与非 ff 都适用），原"仅保留给确定可 ff"是错的 | g3-A6 |
| 74 | rebase 中断的清理责任 | 打回 develop **之前**由系统（merge.execute）执行 `git rebase --abort` 恢复干净 worktree，不再要求 develop agent 自己 abort（原 §6 表述与 §8 幂等策略、§12.4.4 命令归属冲突） | g3-A6 |
| 75 | run_command 超时上限 | `tool_timeout_sec`（60）是默认值而非硬上限：agent 显式传 `timeout_sec` 时取该值；未传时 test / merge 阶段取 `test_command_timeout_sec`（600），其余阶段取 `tool_timeout_sec`；系统驱动命令始终用 `test_command_timeout_sec` | g3-A7 |
| 76 | API 收敛 | `GET /tasks/{id}` 为唯一状态查询入口（含 `allowed_actions`）；`GET /tasks/{id}/stream` 为唯一 SSE 通道（事件按 `type` 区分 flow / command / pending / 终态）；删除 `GET /tasks/{id}/pending/actions` 与 `GET /tasks/{id}/flow/stream`，`/flow` 保留为历史时间线查询 | g3-A10 |
| 77 | 子代理会话落库 | 子代理**各自占一行** `kanban_node_runs`（`agent_type` 非 `main`、`parent_run_id` 指向父 run），对应自己那一行会话；1:1 的表述修正为"main 会话与 run 1:1"，`run_id` 对所有会话恒非空 | g3-A8 |
| 78 | project_analysis 职责划分 | 六项探测（语言 / 测试框架 / AGENTS.md / 默认分支 / 目录结构 / .gitignore）全部由**代码**实现（符合 G7，可单测、结果稳定）；`project_analysis` 伪阶段保留，agent 只基于事实清单写摘要并标注可疑项，prompt 可省略 | g3-A11 |
| 79 | v1 前端范围 | v1 只发布看板视图，不含对话窗口（与决策 50 一致）；§12.11 的"复用"改为把 Markdown / diff / tool 渲染抽为公共组件供 v1 会话查看器使用、v2 复用；看板的自由输入仅限 `info_insufficient` 的补充说明 | g3-A9 |
| 80 | 并行执行状态 | 新增 `kanban_node_cursors` 表并为 `NodeCursor` 建模，作为执行状态的**唯一事实来源**（`branch` / `stage` / `node` / `status` / `validate_attempts` / `pending_reason`）。`kanban_tasks` 的 `current_stage` / `current_node` / `validate_attempts` 退化为**焦点游标投影**，只供看板展示与筛选；`kanban_node_runs` 增 `cursor_id`。游标各行合起来即 checkpoint | g3-B1 |
| 81 | executor 并发形态 | 仍是**每任务一个** executor（沿用决策 36 的进程内 Mutex + DB `executor_owner` 乐观锁），内部用 `buffer_unordered` 并发驱动多条游标；不引入 per-branch executor，避免"同任务被并发改动"重新成立 | g3-B2 |
| 82 | 分支状态独立 | `validate_attempts` 每游标独立、按游标判定 `validate_retry_max`；pending 挂在**游标**上，任务级 `status = pending` 是"任一游标被阻塞"的投影；一个分支进 pending **不打断**另一分支，后者跑完本阶段后停在 join 边界（`waiting_join`），不再推进；`ResumeRequest` / `AllowedAction` 增加 `cursor_id`（省略时取 main 游标） | g3-B3 |
| 83 | join 与 backtrack | join 条件 = 所有分支游标都到达边界（`waiting_join`）且均无 pending，汇聚节点**只执行一次**（落实 G5）；backtrack 时**两条游标一起**重置到 `architect-design.validate_input`，`dev-plan.md` / `test-scenarios.md` 标记为过期（文件保留供回溯，下次执行覆盖写入） | g3-B4 |
| 84 | 并行可观测性 | SSE 事件体带 `branch` 字段用于分支消歧；`kanban_transitions` 增 `branch` 列，并行区间出现两条交错记录（用 `∥` 标识）；新增 `cursor_changed` 事件；看板卡片在并行区间渲染两个"当前节点"药丸，pending 药丸按分支着色、动作集独立下发 | g3-B5 |
| 85 | merge 测试闸门失败路径 | 闸门失败**不直接打回 develop**：把失败输出交给 `test.execute` 重新分析根因，复用决策 62 的 `failure_cause` 分类——全部 `test_issue` → agent 自己修用例后重跑闸门；存在 `code_issue` → pending 由用户选择改用例或回开发。闸门失败次数 `gate_failures` 记在 merge metadata，超 `validate_retry_max` 才 `pending(retry_exhausted)`，且**不跨阶段跳转重置**（否则会死循环）。不给纯代码的 merge 阶段新增 agent 调用与 prompt。**落点与复检上下文由决策 108 / 109 补充**（存 `kanban_stage_outputs.metadata_json`；`gate_recheck` 触发 prompt 追加） | g3-B6 |
| 86 | merge 无 skip | merge 的 `retry_exhausted` 只给"重试 / 终止任务"，**移除 skip**——merge 的 skip 等于越过测试闸门直接合入 `default_branch`，是全流程风险最高的动作 | g3-B7 |
| 87 | 伪阶段 persona 校验 | §10.6.4 的校验范围由"所有阶段配置"改为"所有注册阶段"，伪阶段按各自要求校验：`project_analysis` 的 persona **允许为空**（省略时只输出确定性探测的事实清单，与决策 78 一致）；`conflict_check` **强制非空**；其余校验（provider 白名单 / 工具并集 / 超时覆盖）与正式阶段一致 | g3-B8 |
| 88 | 伪阶段心跳归属 | 伪阶段是正式节点内**同步**发起的第二次 LLM 调用，其流式 token 与工具活动必须计入**父节点心跳**（同一 `kanban_node_runs.last_activity_at` 与 `process_group_id`），否则 `node_idle_timeout_sec`（300s）会在比对期间误杀 `architect-design.execute`；同时给 `conflict_check` 设较短的阶段级 `max_duration_sec`，避免拖到 `node_max_duration_sec` 上限。**由决策 100 修订**：观测上伪阶段有**独立** run / 会话行，计量不重复计入父行 | g3-B9 |
| 89 | executor 的 pending 语义 | pending 游标**移出可运行集合**，而非让整个 executor `return`——否则决策 82 的"分支互不阻塞"根本无法成立。只有"无 active 游标且有 pending"时才暂停等 resume；且**单条游标的节点失败不得向上传播**中止整个 executor，它只把该游标置 pending | g4-Q1 / Q28 |
| 90 | 游标生命周期 | `POST /tasks` 与 Task **同事务**插入单条 main 游标（`init.execute`），`waiting` / `queued` 任务也有游标（`dependency_failed` 因此有处可挂）；分裂 = main 行就地改写为 `develop-design` + 插入 `test-design` 行；合并 / backtrack = **单事务**删两条分支行 + upsert 单条 main 行；终态保留游标行供审计，仅"重试"时重置为单条 main。**由决策 113 修订**："删 / 清空"改为"归档（status=archived）+ 插入新行（新 cursor_id）"，游标行永不物理删除 | g4-Q6 |
| 91 | 多游标下的 resume 目标 | 并行区间**不存在 main 游标**（已被改写为 develop-design），因此删除了未定义的 `main_cursor_id` 兜底：恰好一条游标时 `cursor_id` 可省略，否则**必须显式提供**，缺失返回 409 | g4-Q6 |
| 92 | 焦点游标与调度器谓词 | 焦点游标 = 优先取 pending 游标，否则取 `updated_at` 最新者，串行时即 main；`current_stage` 只驱动排序 / 筛选，**不驱动看板列归属**（并行区间用独立槽位 + 两个分支药丸）。scheduler 的 pending 提醒 / stalled 必须谓词化为 `has_runnable_cursor`，不得只看 `task.status`——一个分支 pending、另一分支在跑的任务 `status = pending` 但并未卡住（并发准入的名额占用见决策 117：`status ∈ {running, pending}`） | g4-Q7 / Q15 |
| 93 | skip 的落点 | `skip` **不复用 `entry_node`**，落点单独成表：architect-design → 游标分裂到两个设计阶段的 `validate_input`；develop-design / test-design → 本分支置 `waiting_join` + `skipped_to_join = true`（**不得越过 join**，否则绕过 G5）；develop → review.execute；review → test.execute；test → merge.execute；merge 无 skip（决策 86）。`skipped_to_join` **不改写产出元数据**（不伪造 readiness），由 `sync-check.execute` 读取该标志。**由决策 115 补充**：被跳过分支的产出文件可能不存在，下游 execute 的 prompt 显式降级为"直接基于 design.md 工作" | g4-Q4 / Q16 |
| 94 | validate_input 的 pending 类型 | 类型**按阶段定死**并落到路由：`architect-design.validate_input` → `info_insufficient`（走 `continue` + 输入框补充）；`develop-design` / `test-design.validate_input` → `user_decision`。路由函数返回带 type 的 pending，不再返回裸 `Pending` | g4-Q5 |
| 95 | merge 闸门与审批正交 | `merge_result` 增独立的 `gate: "pass" \| "fail"` 字段承载测试闸门结果，**不得塞进 `approval` 枚举**；删除路由中不存在的 `Approval::GateFailed`；`Approval::Pending` 改为**不推进**（原 `=> Next` 会把未审批的任务直接送进 done） | g4-Q2 |
| 96 | 阶段 B 的基准校验 | 阶段 A 生成 proposal 时把 `{base_ref}` 的 commit SHA 记入 `merge_result.base_commit`；阶段 B 入口重新 fetch 并比对，**不一致说明 diff 已过期** → `approval` 重置为 `none`，回阶段 A 重新 rebase + 重跑闸门 + 重新审批 | g4-Q3 / Q17 |
| 97 | 合入结果必须写回分支 | 阶段 B 在**临时 worktree** 内合入后，用 `git update-ref refs/heads/{default_branch} <merge-commit>` 显式写回（ff 与非 ff 都适用）。原"`update-ref` 仅用于确定可 ff"是错的——detached worktree 里的 `--no-ff` merge commit 不属于任何分支，不写回就随 worktree 移除而丢失，`default_branch` 永不前进（修订决策 73） | g4-Q3 |
| 98 | 并发准入与 `queued` | TaskStatus 增 `queued`（排队等并发名额，与 `waiting` 等依赖正交）；`POST /tasks` 一律以 `queued`（有依赖则 `waiting`）落库，`start_task` 是唯一准入闸门。否则无依赖任务"立即启动"会绕过 `max_concurrent_tasks`，worktree 也会在名额之外被创建。**由决策 117 修订**：名额占用谓词 = `status ∈ {running, pending}`，resume / 超时重试免复检，failed → retry 置回 queued 重新准入 | g4-Q9 / Q18 |
| 99 | 纯代码节点的 run 行 | `init` / `sync-check` / `merge` / `done` 同样写 `kanban_node_runs`（`agent_type = "system"`，token 0，无会话行），否则超时检测与耗时统计漏段。决策 63 的 1:1 收窄为"**调用 LLM 的 run** 才与会话 1:1"。**由决策 114 推广**：清单扩展为所有不调 LLM 的节点（含 develop / test 的纯代码 validate_output） | g4-Q10 |
| 100 | 伪阶段独立观测 | 伪阶段落**独立** run + 会话行（`agent_type = "pseudo:*"`，`parent_run_id` → 父 run，`cursor_id` 继承），用户可查看它为何判定 `duplicate_risk`；失败等同父节点失败、**不触发独立节点级重试**；心跳写父 run 的 `last_activity_at`（决策 88 初衷不变）；`total_tokens` = 所有 run 行求和，父行 token **不含**子行。另：`run_recorded_command` 须在系统命令起止刷新 `last_activity_at`，否则 merge 闸门（最长 600s）会被空闲超时（300s）误杀 | g4-Q11 / Q19 |
| 101 | 残缺的端点补全 | 补 `GET /tasks`（看板唯一数据源，含分支级摘要；支持 `project_id` / `status` / `include_archived` 过滤——原决策 106 的内容已并入本行）、`PATCH` / `DELETE /projects/{id}`；`allowed_actions` 的每个 `side_effect` 动作必须与端点成对，否则前端渲染出点不动的按钮 | g4-Q13 |
| 102 | conflict_wait 的列表与重评估 | `context.conflict_task_ids` 存**全部**冲突任务（原单个 `conflict_task_id` 在多冲突时会提前恢复）；**全部**终态后才重跑第一层比对——仍有交集则保持 pending 并更新 id 列表（不重跑节点），无交集才清 pending 拉起 executor | g4-Q12 / Q20 |
| 103 | 厂商能力边界 | `SystemBaseline.allowed_providers` 拆为 `supported_adapters`（代码硬编码的适配器集合，改它要发版）；DB `providers` 表中不受支持的行**降级 `enabled=0` + UI 告警，不崩溃**，但被 `stage_configs` 引用的则**配置加载 fail fast**（非对称处理） | g4-Q8 / Q23 |
| 104 | 沙箱降级为文件工具策略 | **修订决策 19**：v1 **不做系统级沙箱**。原 `sandbox` 块更名为 `FileToolPolicy`，只约束 6 个文件工具（`run_command.cwd` 仅为卫生默认值）；判定前须 realpath 解析（macOS `/etc`→`/private/etc`、`/tmp`→`/private/tmp`，否则 deny 静默失效），拒绝写符号链接。G11 同步降级为**策略而非系统保证**（shell 无法强制执行），残余风险记入 §9 与 §12.14 | g4-Q22 / Q24 / Q25 |
| 105 | `split_task` 与 `model-override` 的作用域 | `model-override` 为**任务级**（`Task.model_override`，只影响本任务后续节点，不改全局 stage config——pending 的语境是"这个任务塞不下"）；v1 的 `split_task` 收窄为"按用户给定方案用 `POST /tasks` 创建 N 个新任务 + 原任务置 `cancelled`"，自动拆分规划留 v2 | g4-Q21 |
| 106 | 任务列表与端点配对 | **已并入决策 101**（g5 文档卫生，决策 120）：本行原与 101 重复，仅保留编号占位，避免 107–112 改号 | g4-Q13 |
| 107 | join 的游标归属 | `sync-check` **不占游标行**——它是游标无关的屏障，由 `advance_join` 在"无可运行游标且全部 `waiting_join`"时执行一次（落实 G5）。`waiting_join` 由 `advance_cursor` 在 `next` 指向 join 时写入，无其他写入路径；join 落库为一次 `agent_type="system"` 的 run（`cursor_id` = 同事务内新建的 main 游标，决策 113） | g4-Q27 |
| 108 | `gate_failures` 的落点 | 记在 `kanban_stage_outputs.metadata_json`（merge 行），与决策 30 一致；merge 的 upsert 路径**显式跳过该字段**，而不是靠"不重置"的约定。跨阶段跳转不重置（否则 merge ↔ test 循环不终止），超 `validate_retry_max` 才 `pending(retry_exhausted)` | g4-Q29 |
| 109 | 闸门复检的 prompt | test 游标被闸门打回重入时置 `test_result.gate_recheck = true`，execute 的 prompt 追加闸门失败输出（`kanban_node_commands` 完整日志 + 失败用例）让 agent 重新判定 `failure_cause`。两个计数分属两侧：`gate_failures` 在 merge（不重置），test 自身的 `validate_attempts` 照常按跨阶段跳转归零 | g4-Q28 配套 |
| 110 | 工具结果阈值合并 | `tool_result_max_tokens` 与 `offload_threshold_tokens` 合并为**单一**配置项 `offload_threshold_tokens`（默认 4000），消除二者声明相等却都可配置的冗余；另：`model_context_window()` 对未注册模型显式失败，不静默取默认 | g4 小项 |
| 111 | provider / model 的正规化 | `providers` 表一行 = 一个 `(vendor, model, context_window)`；**删除 `stage_configs.model`**，阶段只引用 `provider_id`。换模型即换 `provider_id`，使 L0 容量预估（§12.13.3）查找窗口大小的路径唯一 | g4-Q32 |
| 112 | provider 密钥明文存储 | **修订决策 10**：`providers` 增明文 `api_key` 列，**删除整张 `api_keys` 表**（密文 + nonce 列），`GET/POST/DELETE /providers/{id}/api-keys` 收敛为 provider CRUD（读接口只回显 `***`）。依据：ZCode 自身即明文存储且不加密；在**同机 shell 不受限**的前提下，加密的密钥与密文同处一机、可被同样取到，只增加一次可被执行的解密步骤，挡不住本应防住的对手。配套新增 §12.14（目录 0700 / DB 0600 / 启动权限校验告警 / 残余风险显式化） | g4-Q26 / Q30 / Q31 |
| 113 | 游标归档而非物理删除 | **修订决策 90**：`kanban_node_cursors` 行**永不物理删除**——合并 / backtrack / 重试把旧行置 `status = "archived"` 后插入新行（新 `cursor_id`，ULID）；`UNIQUE(task_id, branch)` 改为 partial unique index（`WHERE status != 'archived'`）。依据：`kanban_node_runs.cursor_id` 为 NOT NULL 外键，"删除两条分支行 / 重试删全部行"会让并行区间与上一轮的 run 行悬空；sync-check 的 system run 在推进事务内**先插 main 游标、再落 run 行**（`cursor_id` = main），补全决策 107 的 run 归属 | g5-Q1 |
| 114 | 纯代码节点 run 行泛化 | **推广决策 99**：落 run 行的清单从四个纯代码阶段扩展为**所有不调 LLM 的节点**——`develop.validate_output` / `test.validate_output` 同样落 `agent_type="system"` 的 run 行。否则系统测试命令（最长 `test_command_timeout_sec`=600s）没有所属 run 供 `kanban_node_commands` 挂靠与心跳刷新（决策 100），该段耗时也从统计中缺失 | g5-Q2 |
| 115 | 设计阶段 skip 的空产出语义 | **补充决策 93**：并行分支在 `validate_input` 被 skip 时产出文件不存在（无 `dev-plan.md` / `test-scenarios.md`），sync-check 仍按 `skipped_to_join` 放行；下游语义固定为"跳过该设计阶段 = 直接基于 `design.md` 工作"，`develop.execute` / `test.execute` 的 prompt 模板显式写明该降级，不视为错误。**g6 补充**：若 `architect-design` 本身被 skip（execute loop 失败耗尽），`design.md` 可能同样缺失——由下游 `develop-design` / `test-design` 的 validate_input 读不到文档自然 pending(user_decision) 兜住，prompt 降级语义不扩展 | g5-Q3 |
| 116 | dependency_failed 的"继续执行" | "继续执行" = **忽略失败依赖**：任务置回 `queued` 正常走并发准入，记 `dependency_overridden` 警告；不得清 pending 后退回 `waiting`（会被 `check_waiting_tasks` 重新 pending，死循环）。动作集按失败依赖终态裁剪：`failed` → 含"等待依赖重试"；`cancelled` → 仅"继续执行 / 取消任务" | g5-Q4 |
| 117 | 并发名额占用谓词 | **修订决策 98 的计数**：名额占用 = `status ∈ {running, pending}`（准入后、终态前恒占）——pending 任务仍持有 worktree，"worktree 数 ≤ max"才成立；resume / 超时重试免复检（名额未释放）；`failed` → retry 须置回 `queued` 重新走 `start_task` 准入。代价：卡在 merge_approval / human_review 的任务占名额，与 G14 有张力，接受 | g5-Q5 |
| 118 | 命令输出脱敏时机 | 输出脱敏（`sk-*` / `ghp_*` / 长 base64 等正则）在结果**回填 agent messages 之前**执行：agent 看到的工具结果即脱敏后文本，落库与 context 同源——§12.14"密钥不进入日志与会话"的缓解声明据此成立；误伤面（合法长 base64 被打码）为已知代价 | g5-Q6 |
| 119 | merge_approval 的动作端点 | "合入 / 返回修改"均为 `side_effect`，配对端点 `POST /tasks/{id}/merge/decision {decision: "approve" \| "return"}`：单事务内写 `merge_result.approval`（approved / returned）+ 清 pending + 置游标（approve → merge.execute 重入阶段 B；return → develop.execute，`validate_attempts` 重置）。二者都要先写字段再推进，不是纯 resume（补决策 101 的端点配对） | g5-Q7 |
| 120 | g5 文档卫生小项 | 决策 106 并入 101（106 行保留占位）；修正 §4.1 / §5 / §11.7 对 101 / 105 / 106 的错引（含 model_override 注释、PATCH/DELETE /projects）；§13 修订列表补 26（→45）/ 42（→98）；`kanban_tasks.status` DDL 默认值 `'running'` → `'queued'`；词汇表 StageConfig 去掉 model；提醒谓词统一为 `has_runnable_cursor`；`NewSymbol.kind` 补 `class` / `interface` / `type`；决策 71① 注明 queued/waiting 比对为空；§9 符号判重对齐 `(module_path, name)`（纯 name 重合仅 warning） | g5 小项 |
| 121 | route_merge 的 None/Returned 分支 | 路由中 `Approval::None \| Returned => Kickback` 是 bug：与 §6"none / returned 走阶段 A"矛盾，首次进入 merge 会被直接打回 develop，永远生成不了 proposal。修正：None / Returned → `NoOp`；打回 develop 由 execute 内部（冲突）与 decision 端点（return）完成，不经路由 | g6-Q1 |
| 122 | merge 的 timeout 无 skip | §5"任何节点 timeout 耗尽后 goto / skip"未排除 merge；决策 86 的理由（越过测试闸门直接合入是最高风险动作）对 timeout 同样成立。merge 的 timeout 动作集 = 重试 / 终止任务 | g6-Q2 |
| 123 | SSE 会话流式事件 | 排掉前端差距①⑤：`/tasks/{id}/stream` 增补 `conversation_delta`（run_id / agent_type / branch / role / text + prompt_tokens / completion_tokens 增量）与 `tool_event`（tool / phase / 参数摘要）两类事件（§12.7）；SSE 只是渲染通道，落库仍走 §12.4.3 会话 API。§12.11 的流式承诺由此有事件承载 | g6-Q4 |
| 124 | review 产出 diff | 排掉前端差距③：review_mode=human 时 review.execute 完成后由系统生成 `git diff {base_ref}..kanban/{task_id}`（`run_recorded_command`，source=system），写任务目录 `review-diff.diff` + 落 `kanban_stage_outputs`（output_type="review_diff"），经 `/files/{path}` 下发给 dossier | g6-Q5 |
| 125 | retry 的 worktree 重置 | failed 可在任意阶段终止、worktree 留有半成品，而 init 幂等策略"已存在则复用"不清场。retry 的 reset 事务显式执行 `git reset --hard {base_ref}` + `git clean -fdx`（记 system 命令），与"重试 = 从头走"语义一致 | g6-Q6 |
| 126 | backtrack blockers 传递 | §7"传入双方 blocker"一直无机制。backtrack 事务把 blockers 写入任务目录 `backtrack-feedback.md`；architect-design 重入时 validate_input / execute 的 user prompt 注入该内容（首轮为空不渲染），信息不丢、符合 G2 | g6-Q7 |
| 127 | executor_owner 启动清理 | 乐观锁 `WHERE executor_owner IS NULL` 在 kill -9 后残留，任务永久无法重新 claim。应用启动时执行一次 `UPDATE kanban_tasks SET executor_owner = NULL`（单机单进程，启动瞬间无其他持有者），列入 §11.6 恢复流程第一步 | g6-Q8 |
| 128 | 本机 API 跨源防护 | 127.0.0.1 绑定不防跨站 POST（form / no-cors fetch 可驱动 merge/decision、cancel、resume）。axum 中间件：写请求须带自定义头 `X-AgentPipeline`，或 Origin/Referer 缺失（非浏览器）或等于本机 origin，否则 403；SSE 纯 GET 不受影响（§12.14） | g6-Q9 |
| 129 | model_override 解析优先级 | 运行时 provider 解析：`node_overrides > task.model_override > 阶段 provider > 全局默认`；`Task.model_override`（决策 105）补进 §10.6.1 分层与 §10.6.4 合并表，设置时过同一 provider 白名单校验 | g6-Q10 |
| 130 | g6 实现级小项 | ① §5 新增 **allowed_actions 权威总表**（(type, context.kind) → 动作集，resume / side_effect 分层）；② `total_calls` 口径 = 调 LLM 的 run 行数（main + 子代理 + 伪阶段，不含 system）；③ SSE 全量推、cooldown / quiet_hours 只作用于前端 toast（后端吞事件会丢状态同步）；④ 双游标同时 pending 时焦点游标取 `updated_at` 最新；⑤ dependency_failed 的 continue = 清 pending + status 置回 queued 交还准入，不直接 spawn executor；⑥ 决策 115 补充：architect 本身被 skip 时 design.md 缺失由下游 validate_input 的 user_decision 兜住；⑦ `POST /projects/analyze` 改异步 202 + `GET /projects/{id}/analysis` 轮询。**遗留待定（已由决策 132 裁决）：** `duplicate_risk` 的"合并任务"与 dirty_worktree 的"放弃合入"均无配对端点（决策 101 要求成对）→ 二者均已移出动作集 | g6-Q11 |
| 131 | g6 文档卫生小项 | §12.4.4 DDL `run_id` 注释对齐决策 99 / 114（节点内命令恒非空，仅取消 / 归档的任务级清理命令可为 NULL）并补 FK；删除主文档 §12.4.2 与前端规格中的 ¥ 金额 mock（差距④：v1 只显示 token 与调用次数）；frontend-design.md 引用 105 / 106 → 101；前端过滤桶补"已结束（失败·取消）"。**注：** §5 review 行的 retry_exhausted → user_decision 已在并行前端评审中先行修正，本项不再重复处理 | g6-Q3 |
| 132 | g6 遗留端点裁决（收官） | 收编 allowed_actions 总表时暴露的两处配对缺口（违反决策 101）落定：① dirty_worktree 的「放弃合入」**移出动作集**（无端点，与前端评审决议④"merge 无放弃合入"对齐）——保留 `continue`（继续合入）+ `cancel`；② duplicate_risk 的「合并任务」**移出 allowed_actions**（v1 无端点）——保留 `goto develop` + `cancel 其一`，合并由用户自行取消一方后重建（与决策 105 的 split_task v1 姿态一致），自动任务合并留 v2。frontier 清空 | g6-Q1/Q2 收官 |
