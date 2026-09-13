# 测试设计（AgentPipeline 自身）

> 由测试设计评审（t1，2026-09-12）产出，决策 140–152。本文档是 **AgentPipeline 系统自身的测试设计**——流水线阶段 `test-design`（为任务设计业务测试场景，决策 136）是另一个概念，勿混淆（词汇表已加消歧条目）。
>
> 什么时候读：写实现代码前先读 §3（可测试性接缝——对实现的硬性要求，决策 143，另见 [implementation.md](implementation.md) §11.8）；写测试时查 §5–§8 用例目录；评估「实现完成」看 §10 的验收线与映射表。

## 1. 范围

**测什么（决策 140）：** v1 全部交付物——core 流水线（executor / scheduler / 路由 / 游标）、axum API 层、Svelte 前端。落地顺序 core → API → 前端。

**明确不测什么：**

| 项 | 理由 |
|---|---|
| prompt 的**效果**（agent 判得准不准） | 外部不确定性，无法做回归门；prompt 效果回归是附录 B.6 v2 离线 eval 的范围。v1 只测 prompt 的**组装**（§5 golden） |
| 性能 / 负载 | v1 单机本地工具，无 SLA（决策 144） |
| 系统级沙箱行为 | 不存在——决策 104：v1 只有 FileToolPolicy（要测），shell 残余风险靠 `kanban_node_commands` 事后审计（§9 / §12.14），不是测试对象 |
| 非 Chromium 浏览器、真实 SIGINT 信号 | 决策 144 / 152 |

## 2. 风险优先级（决策 141）

| 优先级 | 风险区 | 测试形态 | 深度 |
|---|---|---|---|
| ① | 游标/路由状态机（决策 80 / 85 / 90 / 93 / 113 / 121 / 139——g3–g7 修复的 bug 集中区） | 路由逐 `EdgeKind` 单测 + 游标每次迁移有用例 + E2E 全路径 | 逐决策逐分支 |
| ② | merge 的 git 操作链（决策 73 / 97：rebase / 内存合入 / 引用写回） | testkit 真实仓库 fixture 演练 | 每操作必测 |
| ③ | 超时/心跳/恢复（决策 55 / 64 / 66 / 88 / 100 / 127） | 假时钟确定性测试 | 每条超时路径 |
| ④ | 前端 SSE 归约（决策 84 / 123） | 纯函数单测 + 2 条 playwright 冒烟 | 归约表逐事件 |

## 3. 测试基建

### 3.1 可测试性接缝（决策 143，权威表）

| 接缝 | 生产实现 | 测试实现 | 为什么必须有 |
|---|---|---|---|
| `Clock` trait | 系统时钟 | 手动推进 / `#[tokio::test(start_paused)]` | 决策 55 / 64 / 66 / 100 的时间语义无法等真实时间测 |
| `AGENTPIPELINE_HOME` 环境变量 | 默认 `~/.agentpipeline/` | 每测试独占临时目录 | 家目录硬编码 → 无法并行、无法隔离 |
| 进程组终止器 trait | `kill_process_group` 真杀 | 记录调用 | 测试无可杀进程组，但超时路径仍要断言「杀了」 |
| scheduler `tick()` 手动驱动 | 10s 周期 | 测试直接调用 | tick 六项职责（决策 55）逐项验证 |

> 接缝只做可替换、不改语义：超时判定仍以 `Clock` 读数为唯一时钟源（决策 64）。**实现顺序要求：四个接缝先于业务模块落地**（后补要翻全部模块签名）。

### 3.2 FakeAgent（决策 142 / 148）

**替换边界：只替换 LLM 响应流，工具层全部真实执行**——write_file 真写（临时 home 内）、run_command 真跑、FileToolPolicy 真拦（决策 104）、输出脱敏真过（决策 118）、L2 卸载真落盘、命令真记 `kanban_node_commands`。集成测试因此同时覆盖整个工具子系统；fake 只是「演员」。

| # | 脚本能力 | 验证的决策 |
|---|---|---|
| ① | 类型化脚本：按 `(stage, node)` 声明 tool_calls 序列，submit_metadata 参数直接用各阶段 serde 结构体 | 38（schema 与脚本编译期同源，不漂移） |
| ② | 工具失败注入：第 N 次某工具失败 | G13 / 33（tool_retry_max 分层，单次工具失败不触发节点重试） |
| ③ | 元数据劣化：文本 JSON / 缺字段 / 坏 JSON | §12.12 三级降级、33 |
| ④ | 心跳与流式节奏控制（停跳 / 慢滴） | 64 / 66 / 100（空闲/绝对超时、心跳源） |
| ⑤ | 超长工具结果注入 | §12.13 L1 裁剪 / L2 卸载（110）/ L3 压缩 / L4 兜底 |
| ⑥ | 伪阶段脚本：conflict_check 给 duplicate_risk 等级、validator_cross_check 给合格/不合格 | 60 / 67 / 134 / 135 |
| ⑦ | 子代理**不**脚本化 | 45（默认关闭；L4 走 `pending(context_overflow)`，开启路径实现后补） |

**真 LLM 冒烟（`#[ignore]`，手动跑）：** architect-design.execute 一次真调用，断言 rig 适配 + 结构化输出解析可用。需要真 key，不进任何自动门。

### 3.3 testkit（决策 146）

workspace 成员 `crates/testkit`，供 L2 / L4 复用：

| 组件 | 内容 |
|---|---|
| git fixture builder | **系统 git CLI** 搭建（仅测试脚手架；生产 git 层按决策 12 走 git2，2026-09-12 修订决策 146）：`Repo::clean()` / `unborn_head()` / `with_remote(local_path)` / `dirty_worktree()` / `conflict_auto()` / `conflict_hard()` / `project(Language::Rust \| Python \| Node)`（含 `{test_command}` 模板变量）/ `symlink_trap()`（macOS `/private/tmp` realpath，决策 104） |
| FakeAgent 运行时 | §3.2 的脚本执行器 + `Script::fail_tool_n()` / `Script::metadata_from(struct)` 等构建 API |
| 临时 home | `TestHome::new()` → 设置 `AGENTPIPELINE_HOME`、跑全量 sqlx migrations、返回句柄 |
| 断言助手 | 游标状态断言、run / 会话 / 命令行数断言、SSE 事件录制器、终止器调用记录 |

### 3.4 DB 测试策略（决策 145）

默认每测试一个**临时文件库** + 全量 sqlx migrations（真实行为优先，WAL / busy_timeout 可测）；内存库仅限纯查询逻辑。并发场景（决策 36 乐观锁、§12.10 写锁串行化）显式开双连接测。

## 4. 分层总览（决策 144）

| 层 | 位置 | 被测物 | 基建 | 用例目录 |
|---|---|---|---|---|
| L1 单元 | 各 crate 内 `#[cfg(test)]` | 路由 / 解析 / 策略 / prompt 组装 | insta、纯函数 | §5 |
| L2 集成 | core `tests/` | executor / scheduler / 游标 / git 链路 | testkit + 临时文件库 + FakeAgent | §6 |
| L3 API | app crate `tests/` | 端点契约 + 跨源防护 | tower oneshot（in-process，不 spawn 二进制） | §7 |
| L4 E2E | `tests/e2e/` | 全流程场景 | FakeAgent 驱动完整流水线 | §8（25 条） |
| 冒烟 | `tests/smoke/` | 真二进制启动 | spawn 进程 | E2E-00 |

## 5. 单元测试目录（L1）

| 模块 | 用例 | 决策锚点 |
|---|---|---|
| routes | `route_merge` 全 `EdgeKind`：None→NoOp（不打回）、Returned→NoOp、Pending→NoOp（不推进）、Approved→Next；gate=fail 分流 lint→KickbackDevelop / test→GotoTest | 121 / 95 / 139 / 85 |
| routes | `route_after_validate_output`：attempts 边界（=max→Pending，否则 Retry）；cross_family 分歧路径不进本路由（决策 134 的注释性断言） | 82 / 134 / 135 |
| routes | `route_by_readiness`：architect→info_insufficient；develop-design / test-design→user_decision | 94 |
| 落点表 | `entry_node` / skip 落点全表逐行（architect 分裂 / 分支 `waiting_join`+`skipped_to_join` / merge 无 skip） | 69 / 93 / 86 |
| metadata | submit_metadata 提取 → 文本 JSON 块 → 最后平衡 JSON → 失败（重试 prompt 追加） | §12.12 / 33 |
| FileToolPolicy | realpath（/tmp→/private/tmp）、deny_paths（`.env*`、`*.pem`、`id_rsa*`）、拒绝写符号链接、deny 优先 allow、workdir_bound | 104 |
| 脱敏 | `sk-*` / `ghp_*` / 长 base64 正则；`sanitize_command`（`--token`、URL 凭证）；**回填 messages 之前**执行 | 118 / §12.4.4 |
| 上下文 | L1 各工具裁剪（read 头 200 行 / run_command 错误行保留 / list_dir 200 项）；L2 阈值 4000 唯一；L3 压缩规则表逐行；L4 spawn 关闭→`pending(context_overflow)` | §12.13 / 110 |
| prompt 组装 | golden（insta）：`[基线前言][工作目录(G12)][AGENTS.md(G3)][persona][技能清单][格式规则]` 顺序；AGENTS.md 加载与缺省注入非空默认；`prompts/` 覆盖生效；`stage_configs.persona_path`（相对 home 解析、存在且非空）与 `persona_append` 生效；内置 §10.3 十二个 agent 节点模板（system+user）全部内嵌且只引用已声明变量；SystemBaseline 工具并集（mandatory 不可移除、forbidden 剔除）；`stage_configs` 的 temperature / max_tokens 透传 `LlmRequest`；user prompt 追加段（gate_recheck / backtrack-feedback / retry-feedback，**首轮为空不渲染**）；`{test_command}` / `{design_doc_path}` 等模板变量；`prompt_template_hash` 稳定、对覆盖与路径变化敏感 | 51 / 28 / 7 / 109 / 126 / 138 / 31 / 137 / §10.3 / §10.6 |
| allowed_actions | 权威总表逐行（`(type, context.kind)`→动作集）；**端点配对静态检查**：每个 side_effect 动作必须映射到已注册路由，新增动作忘配端点直接红 | 130 / 101 / 119 |
| 焦点游标 | 投影规则：pending 优先 / `updated_at` 最新 / 双 pending 取最新 | 92 / 130 |
| 指标 | 逃逸率口径、阶段聚合 SQL、`total_tokens`=Σruns、`total_calls`=LLM run 数（不含 system） | 137 / 100 / 130 |
| SSE | 事件体 `branch` 字段；`conversation_delta` / `tool_event` 字段完整 | 84 / 123 |

## 6. 集成测试目录（L2）

| 关注点 | 用例 | 决策锚点 |
|---|---|---|
| 游标生命周期 | 创建（`POST /tasks` 同事务 main 游标）；分裂（就地改写 + 插入）；合并 / backtrack（单事务归档 + 插入）；重试归档；partial `UNIQUE(task_id, branch)` 幂等；行永不物理删除、`kanban_node_runs.cursor_id` 外键不悬空 | 90 / 80 / 113 |
| executor 循环 | pending 移出可运行集合；无可运行且有 pending → 退出等 resume；全 `waiting_join` → `advance_join` 恰执行一次；**单游标失败不向上传播** | 89 / 82 / 83 / 107 |
| waiting_join | 唯一写入路径 = `advance_cursor` 发现 next 是 join | 107 |
| scheduler tick | 六项职责逐一：超时 / 冲突恢复 / 依赖启动 / 依赖恢复 / 准入 / 提醒+stalled（谓词 `has_runnable_cursor`）；双阈值超时与 effective 值层级（节点 > 阶段 > 全局） | 55 / 66 / 92 |
| 超时处理 | attempt < max → 干净对话重试；耗尽 → `pending(timeout)` 挂该游标（另一分支不受影响） | 33 / 82 |
| 冲突恢复 | 冲突任务**全部**终态才查；复检仍有交集 → 更新 context 不重跑节点 + SSE `pending_updated` | 102 |
| 依赖 | all done → queued；failed / cancelled → `pending(dependency_failed)` 挂 main 游标；依赖 retry → 退回 waiting | 57 / 116 / 90 |
| 准入 | 名额占用 = running + pending；queued 按 slots 放行 | 117 / 98 |
| git 链路 | init（有 remote 先 fetch、以 `origin/{default_branch}` 为基准）；merge A（rebase + `base_commit` 记录 + 闸门）；merge B（内存合入 / ff 与 `--no-ff` / 引用写回）；retry reset（`--hard` + `clean -fdx`）；cancel 清理幂等；unborn HEAD 明确报错 | 41 / 96 / 97 / 73 / 125 / 61 |
| 心跳 | 系统命令起止刷新 `last_activity_at`（600s 命令在 300s idle 下存活）；伪阶段心跳归父 run；流式 token 心跳 | 100 / 88 / 134 |
| DB 并发 | `try_claim_executor` 双连接竞争；DbWriter 写串行化 | 36 / §12.10 |
| 配置 fail fast | `cross_family_judge=true` 无 provider → 拒绝启动；不支持 vendor → 降级 `enabled=0`、被引用才 fail fast；skill 不存在 fail fast | 134 / 103 / 47 |

## 7. API 契约测试（L3）

in-process axum router（tower oneshot），不 spawn 二进制：

| 端点组 | 关键断言 | 决策 |
|---|---|---|
| POST /tasks | 一律 queued（有依赖 waiting）；循环依赖 400；未配置 provider 明确报错 | 98 / 27 / 56 |
| POST /projects | 非 git 仓库拒绝；DELETE 有活跃任务拒绝 | 61 / 29 / 101 |
| GET /tasks | `project_id` / `status` / `include_archived` 过滤；分支级摘要 | 101 |
| GET /tasks/{id} | `allowed_actions` 按 `(type, context.kind)` 下发 | 49 / 130 |
| POST /resume | cursor_id：恰一条可省略、多条缺失 → 409；动作不在 allowed 集合 → 4xx；`pending_resume_cooldown_sec` 内防连点 | 91 / 49 / §3 |
| merge/decision | approve → 阶段 B；return → develop.execute 且 `validate_attempts` 重置；单事务 | 119 / 72 |
| POST /review | human 模式 approve → test / reject → develop.execute | 2 |
| retry / cancel / archive / split / model-override | 旁路端点逐一：retry → queued 重新准入；model-override 过白名单校验、只影响本任务 | 125 / 117 / 105 / 129 |
| GET stream / flow | 唯一 SSE 通道；事件按 `type` 区分 | 76 |
| conversations / commands | 1:1 只对调 LLM 的 run；system run 无会话行；卸载输出走 `/commands/{id}/output` | 63 / 99 / 114 |
| providers | `api_key` 读接口回显 `***`，不返回原值 | 112 |
| analyze | 202 + `GET /projects/{id}/analysis` 轮询 | 130 |
| 跨源防护矩阵 | 带 `X-AgentPipeline` → 过；无 Origin/Referer（非浏览器）→ 过；恶意 Origin → 403；GET / SSE 不受影响 | 128 |

## 8. E2E 场景矩阵（L4，决策 149）

harness = FakeAgent（§3.2）+ testkit fixture（§3.3）+ 临时 home + 手动 tick + 按需假时钟。**P0 = 「实现完成」的验收线**（决策 149）。

**E2E-00（冒烟）** spawn 真二进制：启动、无 provider 创建任务报错、优雅退出（决策 54 / 56）。

| 编号 | 场景 | 关键决策 | 核心断言 | 级 |
|---|---|---|---|---|
| E2E-01 | happy path 全流程 | 80 / 90 / 97 / 99 | 游标分裂→合并→归档序列；`default_branch` 前进（update-ref）；worktree + 分支清理；system run 行落库；token 汇总 | P0 |
| E2E-02 | sync-check backtrack | 83 / 126 | 双游标归档 → main 指 architect.validate_input；两文档标过期；`backtrack-feedback.md` 写入；重入 prompt 含反馈段；attempts 归零 | P0 |
| E2E-03 | review 打回循环 | 43 / 133 | rejected → pending(user_decision) → goto develop.execute；required_changes 进 prompt；re-review 通过 | P0 |
| E2E-04 | human review | 2 / 124 | pending(human_review)；`review-diff.diff` 生成；approve → test / reject → develop | P0 |
| E2E-05 | merge 冲突打回 | 74 | 自动合并失败 → `rebase --abort` → develop.execute prompt 含冲突文件；attempts=0 | P0 |
| E2E-06a | 闸门测试失败 → 全 test_issue | 85 / 108 / 109 | gate=fail、gate_failures=1；跳 test.execute、`gate_recheck=true`、prompt 含闸门输出；修用例后重跑闸门 pass | P0 |
| E2E-06b | 闸门失败 → code_issue | 85 | 存在 code_issue → pending(user_decision) → goto develop.execute | P0 |
| E2E-07 | 闸门 lint 失败 | 139 | 直接打回 develop.execute（**不经** test.execute）；`gate_failure_kind=lint`；attempts=0；gate_failures 统一累加 | P0 |
| E2E-08 | gate_failures 耗尽 → failed → retry | 86 / 108 / 125 / 117 | 耗尽 → pending(retry_exhausted) **无 skip**；终止 → failed；retry → 旧游标归档 + 新 main、worktree `reset --hard` + `clean`、置 queued 重新准入 | P0 |
| E2E-09 | 基准前移 | 96 / 108 | 阶段 A 后推进 base → approve → 校验不一致 → approval 重置 none → 重走阶段 A；gate_failures 保留 | P0 |
| E2E-10 | 脏工作区合入 | 61 / 132 | pending(user_decision, dirty_worktree)；动作集 {continue, cancel}；continue → 合入成功 | P0 |
| E2E-11 | skip 落点矩阵 | 93 / 115 / 86 | architect skip → 分裂；develop-design skip → `waiting_join`+`skipped_to_join`、sync-check 视 readiness=true、下游 prompt 降级；merge 无 skip；skip 不改产出元数据 | P0 |
| E2E-12 | 并行互不阻塞 | 82 / 89 | develop-design 分支 pending → test-design 照常跑完停在 `waiting_join`；resume 后 join 正常 | P0 |
| E2E-13 | 中断恢复 | 80 / 113 / 127 | develop.execute 中途 abort → 重启 → executor_owner 清理 → 节点级恢复；并行双游标独立恢复（决策 152：in-process） | P0 |
| E2E-14 | 超时链 | 33 / 64 / 66 / 100 / 122 | 心跳停 → idle 超时 → 杀进程组（终止器记录）→ 干净对话重试 → 耗尽 → pending(timeout) 挂游标；merge timeout 动作集无 skip；600s 系统命令不被 300s idle 误杀 | P0 |
| E2E-15 | judge_disagreement | 134 / 135 | 首判不合格 + 复判合格 → pending(user_decision, judge_disagreement)；continue 特判直接 next_stage（不重跑）；goto execute → attempts+1 | P1 |
| E2E-16 | design_refs 完整性 | 136 | high 悬空 → blocker → backtrack；medium 悬空 → 仅 warning（sync-check run metadata） | P1 |
| E2E-17 | conflict_wait | 71 / 102 / 120 | 晚者让步（同秒 id 字典序）；`conflict_task_ids` 全量；全部终态 + 复检无交集 → 自动恢复；纯 name 重合仅 warning | P1 |
| E2E-18 | duplicate_risk | 60 / 67 / 132 | 模块重叠无符号交集 → conflict_check high → pending(user_decision, duplicate_risk)；动作集 {goto develop, cancel 其一} | P1 |
| E2E-19 | 依赖三态 | 116 / 57 / 90 | waiting → queued → running；dep failed → dependency_failed 挂 main 游标；continue → queued + `dependency_overridden`；dep retry → 退回 waiting；dep cancelled → 动作集无「等待依赖重试」 | P1 |
| E2E-20 | 并发准入 | 117 / 98 | max=1 时任务 2 停 queued；pending 占名额；resume 免复检；failed → retry 重新准入 | P1 |
| E2E-21 | info_insufficient | 79 / 94 | `requires_input` 动作；continue 带 input → 注入后续 prompt；validate_input 重跑 | P1 |
| E2E-22 | context_overflow | 105 / 110 | L4 仍超限 → pending(context_overflow)；动作集 {split_task, model_override, cancel} 均有配对端点 | P2 |
| E2E-23 | 取消传播 | §12.3 | cancel → worktree 强制清理、分支删除、依赖任务 pending(dependency_failed)、SSE `task_cancelled` | P2 |
| E2E-24 | resume 防连点 | 36 / §3 | cooldown 内第二次 resume 不重复 spawn（单执行者守卫） | P2 |

## 9. 前端测试（决策 150 / 151）

| 层 | 工具 | 用例 |
|---|---|---|
| 单元 | vitest | `reduce.ts` 归约表逐事件（design §9.1 每行：列归属 / 信号色 / 待办计数 / dossier 开合）；allowed_actions 渲染分组（resume / side_effect、`requires_input`）；NotificationPolicy（cooldown、quiet_hours、cancelled 不弹） |
| 组件 | @testing-library/svelte | PendingActions（按所属游标取 cursor_id——决策 91）；DiffReviewPanel（无「拒绝」——决策 23）；StalledBadge（决策 34） |
| E2E | playwright（只 Chromium） | **真 axum 后端 + FakeAgent**（临时 home），两条：① happy path（看板 → 详情 → 页签 → diff 审批合入）；② pending → dossier 面板 → resume（琥珀面板、顶栏待办计数） |

**前端测试状态（2026-09-13，票 18 收尾）：** 单元层已落地并全绿（`frontend/`，85 个 vitest：`reduce.ts` 归约表逐事件、SSE 连接层主动重连、`allowed_actions` 渲染分组与 cursor_id、NotificationPolicy、provider 掩码保存规则、analyze 轮询、metrics 字段映射、stage_configs payload）。组件层以 vitest + DOM 断言覆盖 PendingActions / DiffReviewPanel / StalledBadge。**playwright 两条 E2E 已执行**（票 18）：用例在 `frontend/e2e/happy-path.spec.ts` 与 `frontend/e2e/pending-resume.spec.ts`，harness `frontend/e2e/harness.ts`（临时 home + 真 `serve --port 0` 就绪行回读 + Vite 代理），跑法 `just frontend-e2e`（或 `cd frontend && npx playwright test --project=chromium`），只 Chromium（决策 144）。

> **与决策 151 的显式偏差：** 决策 151 要求「复用 E2E harness、**不维护独立 mock server**」，票 18 的实现未复用 testkit 的 FakeAgent，而是在 `frontend/e2e/harness.ts` 里写了一个 Node 侧的 OpenAI 兼容 SSE mock（按 persona 反查 `(stage, node)`、按轮投喂）。**理由**：playwright 进程（Node）无法直接调用 Rust 的 `testkit::MockLlm`，复用需要一个额外的 Rust helper 二进制并纳入 playwright 的构建前置；v1 以「少一个构建步骤、harness 自包含」优先。**代价**：存在第二份 mock 实现，可能与 Rust 侧契约漂移——它仍必须发出真实适配器能解析的 OpenAI SSE，故「SSE 事件格式 ↔ 前端归约」这条契约仍被覆盖，但**契约漂移风险由本注记显式承担**（后续若把 testkit 的 mock 抽成 helper 二进制，应删掉 Node mock）。决策 151 的其余要求（真 axum 后端、`AGENTPIPELINE_HOME` 指临时目录、两条冒烟）均满足。

## 10. 质量闸门与 traceability（决策 147）

**justfile：** `just lint`（`fmt --check` + `clippy -D warnings`，提交前必过）、`just test`（`cargo test --workspace`，即 L1 单元 + L2 集成 + L3 API + L4 场景 + 冒烟，**只覆盖 Rust**；另有 `just unit` / `integration` / `api` / `e2e` / `smoke` 分层子集与 `just fmt`）、`just frontend-e2e`（前端 playwright 双冒烟）。
**前端测试不在 just 配方内**：单元层 vitest 与 `svelte-check` / `build` 直接在 `frontend/` 下跑（`npm test` = `vitest run`、`npm run check`、`npm run build`）——`just test` 不会跑它们。

**逐分支覆盖（不做全局数字门）：** `route_merge` 每个 `EdgeKind`、游标状态机每次合法迁移（active / waiting_join / pending / archived 之间）各有用例。

**决策 ↔ 测试映射表（验收 = 表无空行且每行有真实锚点）：** 「实现状态」列区分 `已有用例` 与 `待 executor`（行为住在执行循环里，executor（§11）落地后补）。

| 决策 | 测试锚点 | 实现状态 |
|---|---|---|
| 85 / 108 / 109 | E2E-06a / 06b / 08 / 09 | 85/108 已有用例（§5 route_merge 闸门耗尽收口、E2E-09 的 `gate_failures` 保留、L3/E2E 的 upsert 跳过计数）；109 的 `gate_recheck` 注入与 E2E-06a/06b 已有用例（票 15 / 19） |
| 121 / 95 | §5 routes 单测、E2E-01 | 已有用例 |
| 139 | E2E-07、§5 routes 单测 | routes 部分已有用例（lint→KickbackDevelop 单测 + E2E-07，票 19） |
| 90 / 113 | §6 游标生命周期、E2E-01 / 08 / 13 | 生命周期已有用例；E2E-13 已有用例（票 18，in-process 重启恢复） |
| 83 / 126 / 43 | E2E-02（backtrack 链：双游标归档 / 设计文档标过期 / 反馈文件落盘 / 重入 prompt 注入 / attempts 归零） | 已有用例 |
| 93 / 115 | §5 落点表单测、E2E-11 | 已有用例（E2E-11：architect 分裂、双分支 skip → `skipped_to_join`、决策 115 降级断言） |
| 82 / 89 | §6 executor 循环、E2E-12 | 已有用例（L2 executor 循环 + E2E-12 尾段 resume→join） |
| 96 / 97 / 74 | E2E-05 / 09、§6 git 链路 | 97 / 74 已有用例；96 的执行侧比对已接线（merge 阶段 B 入口），端到端场景已有用例（E2E-05/09/10，票 15 / 19） |
| 64 / 66 / 100 / 88 | §6 超时/心跳、E2E-14 | 64 / 66 / 88 已有用例（假时钟超时链 + executor 系统命令起止心跳）；E2E-14 已有用例（票 19）；agent `run_command` 周期心跳已落地（票 13，tools 单测）+ 流式 token 心跳（票 13，mock server 集成测试断言 `last_activity_at` 刷新） |
| 134 / 135 | E2E-15、§6 配置 fail fast | fail fast 已有用例；`resolve_validate_output` 单测已有；E2E-15 与 135 的 continue 特判已有用例（票 16 / 19） |
| 136 | E2E-16、§5 | 校验本体已实现（`compute_sync_decision`，E2E-02/11 间接覆盖 proceed/warning 侧）；E2E-16 高悬空场景已有用例（票 19） |
| 128 | §7 跨源防护矩阵 | 已有用例（严格相等 + 前缀伪装拒绝，2026-09-12 收紧） |
| 118 / 104 | §5 脱敏 / FileToolPolicy 单测 | 已有用例 |
| 91 / 119 | §7 resume / merge-decision、E2E-04 | 91 / 119 已有用例；E2E-04 已有用例（票 19） |
| 117 / 98 | §6 准入、E2E-20 | 准入已有用例；E2E-20 已有用例（票 19） |
| 130⑤ / 69 / 71② / 125 / 3 | §7 回归（dependency continue 不 spawn / goto 入口节点 / 纯 name warning / retry reset / cancel 清理）、§6 | 已有用例（2026-09-12 偏离修复回归） |
| 153 | §7 跨源防护矩阵（`X-AgentPipeline` 放行 = 桌面 webview 旁路）、E2E-00 启动冒烟（serve 沉 lib + 随机端口绑定的接线验证） | 跨源侧已有用例（随 128）；其余约束随前端（票 20–22）与桌面壳接线 |
| …… | 其余决策随实现逐条填入 | — |

## 11. 实现状态（2026-09-12，票 15–22 后）

票 01–22 全部实现（**已记录的行为级缺口见文末**，非静默遗漏）：骨架 + executor + prompt 模板消费 + 生产 LLM 适配器 + merge 收尾（rebase 自动合并 / `gate_recheck` 注入）+ 三个伪阶段 + 生产进程接线 + 崩溃恢复 + E2E 矩阵 + 前端三页。质量闸门全绿：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` = **476 个用例全过**（另有 1 个 `#[ignore]` 真 LLM 冒烟）；前端 `frontend/` = **85 个 vitest 全过** + 2 条 playwright E2E 全过 + `svelte-check` 0 error/0 warning + `npm run build` 成功。

**workspace 布局**（与 project-structure 决策一致，`crates/core` 的包名改为 `agentpipeline-core`——包名 `core` 会在宏展开里遮蔽 Rust 内置 `core`）：

```
crates/core/     库：pipeline / agent / storage / scheduler / git / actions / sse / metrics / clock / home / process / config
crates/app/      二进制 agent-pipeline + axum router（lib 供 in-process 测试）
crates/testkit/  决策 146 测试基建
tests/e2e/       L4 场景
frontend/        Svelte 5 + TS + Vite（票 20–22）：看板 / 任务详情与 resume / 配置与指标；vitest 85
justfile         lint / test / unit / integration / api / e2e / smoke
```

**已实现并有用例覆盖（按层）：**

| 层 | 位置 | 用例数 | 覆盖 |
|---|---|---|---|
| L1 单元 | `crates/core/src/**`（in-crate） | 229 | routes 全 `EdgeKind`（含 sync-check backtrack、merge 闸门耗尽收口、code_gate 通过即放行、review 不通过→user_decision）、落点表逐行、metadata 三级降级、FileToolPolicy（realpath / deny / symlink）、脱敏、L1 裁剪 / L2 唯一阈值 / L3 压缩规则表 / L4 兜底、prompt 组装 golden（§10.3 十二节点内嵌模板 + AGENTS.md + stage_configs 消费 + `prompt_template_hash`，票 12）、backtrack 反馈注入范围（决策 126：仅 architect validate_input / execute、首轮不渲染）、allowed_actions 权威表 + 端点按行配对静态检查、焦点投影、指标口径、SSE 事件体（`stage_changed` / `task_done` / `task_cancelled`）、工具真实执行（test-report.md 落任务目录 + **`run_command` 运行期周期心跳**，票 13）、**生产适配器协议解析**（票 13：OpenAI 兼容 / Anthropic 的请求体映射、流 chunk 分片聚合、usage 与 cache token 解析、`[DONE]` / `message_stop` 终止、坏载荷干净报错、base_url 回落） |
| L2 集成 | `crates/core/tests/` | 91 | 游标生命周期（创建 / 分裂 / 合并 / 回退 / 重试 / partial UNIQUE / 永不物理删除 / run 外键不悬空 / **损坏行 fail fast** / **cancel 只挂未启动依赖方** / **backtrack 标过期同事务 + upsert 清除**）、git 链路（init / rebase / 冲突 abort / ff 与非 ff 合入 + `update-ref` 写回 / reset --hard + clean / 清理幂等 / unborn HEAD 明确报错 / 非 origin remote 的基准回落）、scheduler tick 六项职责（超时链 + 进程组终止器 + 节点/阶段/全局超时层级、冲突恢复含复检、依赖三态与恢复、准入、stalled 谓词 `has_runnable_cursor`、**纯 name 重合降级 warning**）、executor 循环（FakeAgent 驱动完整 happy path + sync-check system run 恰一次、单分支 pending 不阻断另一分支、单执行者双保险、元数据失败干净对话重试、会话截断、prompt 组装消费、**`tool_event` start/end 成对发射**）、**生产适配器对 mock server 全链路**（票 13：`testkit::mock_llm` 手写 HTTP server；OpenAI 兼容流聚合 + cache token + conversation_delta + 心跳刷新、Anthropic 双头鉴权 + system 顶层 + tool_result 合并 + 计量归一、deepseek 分发、provider 解析优先级（决策 129 四级：node_overrides > 任务覆盖 > 阶段配置 > 系统默认）、HTTP 401 / 坏流 / 未知 vendor / 无 provider / 禁用 provider 的干净报错） |
| L3 API | `crates/app/tests/api_contract.rs` | 33 | POST/GET /tasks 与过滤、循环依赖与 provider fail fast、`GET /tasks/{id}` 的 allowed_actions 与 blocks、resume 的 409 / 动作集 / 冷却防连点 / **dependency continue 不 spawn** / **goto 入口节点校验**、merge/decision（approve 与 return）、人工评审（**comments 进流转原因**）、retry（**worktree 硬重置 + system 命令入账**）/ cancel / archive / split / model-override、项目 CRUD 与 202 异步分析、provider `***` 回显、**跨源防护矩阵全覆盖**（自定义头 / 无 Origin / 本机 Origin 严格相等 / 恶意 Origin 与**前缀伪装** 403 / 同源 Referer 带路径放行 / GET 不受影响）、SSE 通道、会话与命令 API（**按 task 隔离**，含卸载输出）、任务产出文件与目录逃逸防护 |
| 冒烟 | `crates/app/tests/smoke.rs` | 3 | E2E-00：spawn 真二进制 → 就绪 → 0700 目录权限 → 无 provider 创建任务明确报错 → SIGINT 优雅退出（退出码 0）；端口占用明确报错 |
| L4 E2E | `tests/e2e/tests/` | 38 | `happy_path.rs`：E2E-01 happy path（游标分裂 → join → 合并 → 归档序列、真 git worktree / 提交 / ff 合入、`default_branch` 前进、worktree 与分支清理、system run 落库、token 与调用次数汇总、命令日志、流转时间线）、E2E-09 基准前移使 approval 失效、决策 108 的 `gate_failures` 不被 upsert 清零、E2E-08 的 retry 段（worktree 硬重置 + 重新准入）。`join_and_skip.rs`：E2E-02 sync-check backtrack（双游标归档 → main 指 architect.validate_input、设计文档标过期、`backtrack-feedback.md` 落盘、重入 prompt 含反馈段、attempts 归零）、E2E-11 skip 矩阵（architect skip → 分裂；develop-design / test-design skip → `waiting_join`+`skipped_to_join`、sync-check 视 readiness=true、不伪造产出元数据、下游 prompt 决策 115 降级）、E2E-12 尾段（pending 分支 resume 后 join 恰一次） |
| testkit | `crates/testkit/src/**` | 21 | 临时 home、假时钟、记录型终止器、git fixture（干净 / unborn / remote / 脏 / 可自动合并 / 不可自动合并 / 多语言 / symlink 陷阱）、FakeAgent 脚本能力、断言助手、**mock LLM HTTP server**（票 13：路由前缀匹配 + 请求记录） |

**票 15–22 交付（2026-09-12）：** L1 新增伪阶段（`pipeline/pseudo.rs`：conflict_check 语义层 / validator_cross_check 异族复判 / project_analysis 摘要）与 decision 135 的 continue/goto 特判；L2 新增 merge 闸门复检环（`gate_recheck` 注入 → test 复检 → 重跑闸门）、rebase 自动合并（可机械判定的冲突自动解决，硬冲突仍 abort 打回）、崩溃恢复（决策 152 in-process：中断节点续跑 / 双游标独立恢复 / `waiting_join` 跨重启 / `executor_owner` 清理重准入）、单执行者 `try_run` 信号（resume 重试防静默丢弃）、进程组真实 pgid 回填；L3 新增 `stage_configs` CRUD 契约（未知阶段 / 不可用 provider / 不可读 persona / `cross_family_judge` 依赖的拒绝）与 `project_analysis` 接入 `analyze`（LLM 不可用时保留确定事实并记 `summary_error`）；L4 补齐 §8 矩阵（E2E-03/04/05/06a/06b/07/08/10/14/15/16/17/18/19/20/21/22/23/24，E2E-13 归票 18）；testkit 新增 `fail_tool_n` / `long_tool_result` / `backdate_run` / 伪阶段脚本 / mock-LLM responder。前端 `frontend/`：看板与实时流（fetch 流式 SSE + `reduce.ts` 归约表）、任务详情与 resume dossier（`allowed_actions` 纯渲染）、项目 / provider / stage_configs 配置页与轨道分段指标图。

**测试暴露并修复的三个生产缺陷（2026-09-12）：** ① 脏工作区挂起后 `merge_phase_b` 仍返回 Route，`route_merge` 按 approval=approved 把游标推进到 `done`（改为 `NodeOutput::Pending`，挂起不推进游标）；② `first_layer_conflicts` 的纯 name 重合 Low 告警被 executor 用 `!is_empty()` 当作硬冲突（改为只对 `duplicate_risk=High` 挂 `conflict_wait`，Low 仅告警，与 scheduler 复检的精确 (module,name) 判定一致）；③ resume 请求落在旧 executor「已读完游标、未释放注册表」窗口内会被静默丢弃、任务永久 pending（新增 `Executor::try_run` 报告是否真正执行，resume 钩子据此有界重试）。三者均有用例钉住。

 **2026-09-12 偏离修复（文档-实现对齐 pass）：** 依据文档权威裁决，修正了已实现代码与文档相悖的行为——路由四处（code_gate 先判通过、merge 闸门耗尽进 `pending(retry_exhausted)`、review 不通过进 user_decision、sync-check backtrack 用独立 `EdgeKind::Backtrack`）、goto 落点校验、cancel_task 只对未启动依赖挂 dependency_failed、dependency_failed 的 continue 不 spawn、cancel/archive 回收 worktree 与分支、retry 的 `git reset --hard` + `clean -fdx`（记 system 命令）、纯 name 重合降级 warning、test 阶段 `test-report.md` 写任务目录、跨源严格相等（含 `localhost`）、db 文件 0600 + `-wal`/`-shm` 纳管 + 检查前 realpath、git.rs 三处（unborn 判定 / origin 基准 / 兜底删除的 worktree 标记核验）、skills 校验改对真实 PATH 可执行集合、`[server] host/port` 接线、SSE 线协议命名（§12.7）、human_review 动作名 `approve`/`reject`（端点按行配对）、`NodeStatus` 更名、`MergeResult` 必填字段 + `gate` 无 Default（缺行 ≠ 通过）、merge `output_type` 定名 `merge_result`、损坏数据 fail-fast 分类（执行语义字段报错，观测字段 warn + 兜底）。文档同步修订：决策 70 / 128（改旧行）、§11.5 schema 补全、SSE 表补 `stalled`、agents.md 配置示例对齐。

**尚未实现（下一步）：**

1. **票面剩余**：无。票 01–22 与 `.scratch/agentpipeline-v1-closeout/` 的票 01–18 全部实现（见下方「收尾批次」）。
2. **行为级缺口（文档已定义、当前无生产者）**：仅剩 `task_failed` 终态——决策 70 已裁决 `failed` 变体保留但 v1 无生产者，用户主动终止走 cancel → `cancelled`，系统级失败终态留待有真实生产者时启用（非缺口）。其余原缺口均已关闭：
   - `review_diff` 产出（决策 124）→ 票 13，人工评审前生成 `review-diff.diff` + stage output；
   - review 打回后 `required_changes` 注入 develop prompt（决策 133）→ 票 07；
   - 重试摘要回架构设计注入（决策 138）与 info_insufficient 补充输入注入（决策 79）→ 票 08；
   - `context_overflow`（决策 105 / 110 的 L4 兜底）→ 票 04，E2E-22 走真实执行路径触发；
   - `dependency_overridden` 警告（决策 116）→ 票 06；
   - review / test `code_issue` 的 pending 补 `context.kind`（决策 130 ①）→ 票 05；
   - retry 的会话归档（§12.2）→ 票 11（迁移 0003 加 `archived_at`）；
   - `GET /tasks/{id}/conversations/{run_id}/messages` 端点（§12.4.3）→ 票 12；
   - 项目级伪阶段独立观测行（决策 100 / 130②）→ 票 10（迁移 0004 放开归属）。
3. **真 LLM 冒烟**（`#[ignore]`）——已落地（`crates/core/tests/llm_smoke.rs`，`AGENTPIPELINE_SMOKE_*` 环境变量驱动，验收流式 + 计量 + 结构化输出解析）；未用 rig，生产适配器为手写 reqwest 实现（`crates/core/src/agent/providers/`），`client.rs` 的「rig 适配层」注释以本条为准。
4. **文档已定义、实现留空的配置面**：全部接线完毕（票 16 / 17 / 04 / 14 / 15）——`[logging] format / file`、`prompts.dir`、`adaptive_timeout_enabled`（告警侧）、`run_command` 输出流式 SSE、脱敏的环境变量值、上下文 L2 泛化 / L3 按轮 / L4 接线、`model_context_window`（provider 行）。
5. **测试基建注记**：FakeAgent 的 agent loop 会耗尽同节点脚本队列（每 attempt 吃到队列干涸为止），多轮行为测试须按 `set_script` 分轮投喂（`tests/e2e/tests/common/mod.rs` 头注）；executor 注册表以 task_id 为进程全局键，同进程并发测试须用互不相同的 task_id。

**收尾批次（`.scratch/agentpipeline-v1-closeout/`，2026-09-13）：** 票 01–18 全部实现，§11 原「代码评审记录的偏差与遗留」逐条关闭：
- 决策 100 偏差 → 票 10（项目级 run / 会话行，迁移 0004）；
- 决策 109 偏差 → 票 09（闸门完整日志落 `gate-output-{stage}.log`，复检读全文）；
- 决策 153⑤ 未实现 → 票 02（`serve` 沉 lib + `127.0.0.1:0` 回读端口 + 就绪行）；
- playwright 两条 E2E 未执行 → 票 18（`frontend/e2e/`，只 Chromium，`just frontend-e2e`）；
- 结构性待清理四处 → 票 01（e2e `Flow` 回灌公共模块）/ 票 03（rebase 逻辑回 git 层、action→endpoint 单一事实来源、judge continue 落点复用 `StageLanding`）。

**git 层技术选型（2026-09-12 用户裁决）：** 决策 12（git2 + `spawn_blocking`）与决策 146 原文（生产走系统 git CLI）此前互斥，用户拍板统一 git2——生产 git 层（`crates/core/src/git.rs`）已全部重写为 git2，testkit fixture 保留系统 git CLI 仅作测试脚手架（决策 146 已改旧行）；merge 阶段 B 随之改为内存合入（决策 73 / 97 已补注），git 链路 14 条测试全部在 git2 实现上通过。

