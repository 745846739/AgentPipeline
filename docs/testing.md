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
| **市场客户端 trait**（决策 172⑤，票 10） | `HttpMarketClient`（reqwest，复用既有 HTTP 栈；`Policy::none()` 不跟随重定向） | testkit 的 `FakeMarket`：固定索引与字节，**不打真网络** | 市场有四条真网络**无法稳定复现**的失败路径（摘要不符 / 来源未放行 / 索引畸形 / 网络失败），而票面要求它们互不混淆——只有把网络出口换成 trait 才能钉住 |
| **主题契约模块**（决策 169） | `frontend/src/theme/contract.ts`（token / 几何常量 / 15 枚 sprite / 状态映射的唯一事实源）+ 手工镜像的 `app.css` | `theme/contract.test.ts`（纯数据断言）+ `theme/css-parity.test.ts`（读 `app.css` 两个 token 块与契约**逐条比对**，并扫描全部组件禁止 token 块外裸十六进制颜色）+ playwright 在真应用上断言计算样式 | 30+ token 与 15 枚 sprite 的漂移**人工对照不现实**；`app.css` 是手工镜像（不引入代码生成——它还承载全站基元与移动版规则，整体生成化会让手改 CSS 变危险操作），镜像与事实源之间必须由机器发现不一致 |

> 接缝只做可替换、不改语义：超时判定仍以 `Clock` 读数为唯一时钟源（决策 64）。**实现顺序要求：前四个接缝先于业务模块落地**（后补要翻全部模块签名）。第 5 条接缝（决策 172⑤，`MarketClient`）与第 6 条（决策 169，主题契约）同守此界——主题契约只承载**视觉数据**，不承载状态或业务语义，状态语义仍在 `stores` 与 `realtime/reduce.ts`。

> **唯一的一处扩展（决策 182，不是新接缝）：** FakeAgent 的脚本槽此前有「按 `(stage, node)`」与「按既有伪阶段」两路；**工头既不是阶段、也不是既有伪阶段之一**，故按伪阶段那一路**加一个工头位**（`Script::for_foreman()`，见 §3.2 ⑧）。它是既有接缝（LLM 响应流）里的一个槽位，**不引入新的替换点**——本表不因本特性增行。

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
| ⑦ | 子代理**可**脚本化（票 08） | `Script::push_subagent` 单列一个队列——子代理复用父节点的 `(stage, node)`，共用一个队列会让它悄悄吃掉父节点的一步。见 §10 决策 172③ 行 |
| ⑧ | 工头**可**脚本化（决策 182，票 01）：`Script::for_foreman()` 同样单列一个队列（`text` / `tool` / `read_task` / `read_conversation` 四种步；值班长**不用 `submit_metadata`** 收口，故没有 Submit 步） | 工头既不是阶段、也不是既有伪阶段之一，故在伪阶段那一路上加一个**槽位**——既有接缝的扩展，不引入新的替换点（§3.1）。**工具真跑**：`read_task` / `read_conversation` 读的是真 SQLite 台账（决策 148 的替换边界不变） |

**真 LLM 冒烟（`#[ignore]`，手动跑）：两条**——① 单节点：architect-design.execute 一次真调用，断言 rig 适配 + 结构化输出解析可用；② 全流程（主流程票 04）：真 key + 真模型驱动完整主流程到 `pending(merge_approval)`，fixture 为真实可构建小工程使闸门真跑，断言每节点有 run 行、`submit_metadata` 在真模型返回格式下可解析、token > 0、无节点落 `retry_exhausted`，失败时输出定位诊断（哪个 `(stage, node)` 的什么错误）。运行：`AGENTPIPELINE_SMOKE_*` 环境变量（见 `crates/core/tests/llm_smoke.rs` 头部说明）。两条都需真 key，**不进任何自动门**（决策 142）。

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

> **`BEGIN IMMEDIATE` 与 `SQLITE_BUSY_SNAPSHOT`（决策 163①）：** SQLite 的 deferred `BEGIN` 在「事务内先读后写、中途其他连接提交」时返回 `SQLITE_BUSY_SNAPSHOT`（code 517）——它不是锁等待，`busy_timeout` 重试同一快照永不成功。写事务统一走 `Store::begin_write()`（`BEGIN IMMEDIATE`，建事务即取写锁）后，冲突退化为普通锁等待。任何**新增的多步读-写事务**都必须用它；钉住用例 `cursor_lifecycle.rs::concurrent_writers_do_not_fail_with_busy_snapshot`。
>
> **并发建 worktree 必须按仓库串行（决策 163②）：** libgit2 对共享的 `{repo}/.git/worktrees` 先 `path_exists` 再 `mkdir(GIT_MKDIR_EXCL)`，同仓库多任务同时启动会撞 `EEXIST`。`git.rs::worktree_creation_lock` 按仓库路径分桶串行化该窗口；钉住用例 `git_chain.rs::concurrent_worktree_creation_in_same_repo_does_not_race`。

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
| prompt 组装 | golden（insta）：`[基线前言][工作目录(G12)][AGENTS.md(G3)][persona][技能清单][格式规则]` 顺序；AGENTS.md 加载与缺省注入非空默认；`prompts/` 覆盖生效；`stage_configs.persona_path`（相对 home 解析、存在且非空）与 `persona_append` 生效；内置 §10.3 十二个 agent 节点模板（system+user）全部内嵌且只引用已声明变量；SystemBaseline 工具并集（mandatory 不可移除、forbidden 剔除）；`stage_configs` 的 temperature / max_tokens 透传 `LlmRequest`；user prompt 追加段（gate_recheck / backtrack-feedback / retry-feedback，**首轮为空不渲染**）；`{test_command}` / `{design_doc_path}` 等模板变量；`prompt_template_hash` 稳定、对覆盖与路径变化敏感、**对技能正文敏感**（决策 170，`skill_body_changes_prompt_hash`）；技能三类发现与同名覆盖（`skills.rs`）、工具型 `- {name}` 与知识型 `### {name}` + 正文两种渲染（`prompts.rs::knowledge_skill_body_is_injected` / `tool_skill_renders_as_bare_bullet`）、**节点级技能注入**（`executor.rs::node_scoped_skills_inject_different_bodies_per_node` 断言同阶段两节点各含对方没有的正文） | 51 / 28 / 7 / 109 / 126 / 138 / 31 / 137 / 170 / §10.3 / §10.6 |
| allowed_actions | 权威总表逐行（`(type, context.kind)`→动作集）；**端点配对静态检查**：每个 side_effect 动作必须映射到已注册路由，新增动作忘配端点直接红 | 130 / 101 / 119 |
| 焦点游标 | 投影规则：pending 优先 / `updated_at` 最新 / 双 pending 取最新 | 92 / 130 |
| 指标 | 逃逸率口径、阶段聚合 SQL、`total_tokens`=Σruns、`total_calls`=LLM run 数（不含 system） | 137 / 100 / 130 |
| SSE | 事件体 `branch` 字段；`conversation_delta` / `tool_event` 字段完整 | 84 / 123 |
| 对端地址与配对 | `peer.rs`：ConnectInfo 归一为 `PeerAddr`、**缺省视为回环**（无 ConnectInfo 的 tower oneshot 契约测试不因此变红）、局域网来源读到非回环、`is_loopback_bind` 覆盖各绑定写法；`stream.rs`：令牌比较 `fixed_length_eq`（长度不同即不等、内容不同即不等）；`server_info.rs`：`pairing_url` 形状固定为 `{base}/?pair={token}`、二维码白名单**允许追加 query** 而前缀伪装与异 origin 仍拒、`resolve_qr_target` 原样保留 query | 182⑦ / 167 |

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
| 配置 fail fast | `cross_family_judge=true` 无 provider → 拒绝启动；不支持 vendor → 降级 `enabled=0`、被引用才 fail fast；skill 名字不存在 fail fast；知识型技能**正文为空** fail fast（`skills.rs::empty_user_file_is_config_error`、`executor.rs::empty_knowledge_skill_body_refuses_startup`）；节点级技能名字不存在时报错须**定位到节点**（`executor.rs::missing_node_skill_refuses_startup_with_node_in_message`） | 134 / 103 / 47 / 170 |
| 值班长（决策 182，票 01 / 02 / 05） | `tests/foreman.rs` 21 条：空 home 的快照是空班且不报错、待拍板**带 `pending_reason.message` 原文**、项目列表与已完成计数（cancelled 不计入）、在跑 / 待拍板 / 失败三者分组、历史按**字符预算**裁剪而被裁的仍在库里、超预算也至少留最新一条、会话列出按时间序、空 home 可对话且重载后仍在、LLM 请求带工头身份与占位阶段、模型失败时**人的那句话已落库**、空消息不入账、`read_task` / `read_conversation` **真读台账**并回灌给下一轮、越权工具**在执行点被拒**、未知 task_id 回文本而不是让整轮失败、工具集恰为约定的两个（**安全断言**）、对话**不动全局指标**、会话合计由落库列求和、保留期到点被清理（假时钟）、清理计数分列上报、对话**不产生任何 task 行**；`tests/pairing.rs` 5 条：首读生成并持久化、换句柄打开同一 home 读到同一枚（不随启动重生成）、重置换一枚、未生成过也能重置、令牌字符集可安全落在 `?pair=` 查询里 | 182①④⑤⑥⑦ |

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
| 跨源防护矩阵 | 带 `X-AgentPipeline` → 过；无 Origin/Referer（非浏览器）→ 过；恶意 Origin → 403；GET / SSE 不受影响；**配置扩权（决策 157）**：`[server] allowed_origins` / CLI `--allowed-origin` 注入的 origin 精确放行，未配置局域网 origin 与前缀伪装仍 403 | 128 / 157 |
| 静态资源（决策 155） | `GET /` 200：内嵌时为构建产物 index.html、未内嵌时为构建提示页（按 `EMBEDDED_ASSETS` 是否为空断言）；`/assets/{*path}` 原样回放 + Content-Type；未知资产 404 | 155 |
| `/foreman/*`（决策 182） | 空 home 可读会话（响应含身份回执 `agent_type` / `stage_key` / `wired`）；空 home 可经 API 对话（`POST /foreman/messages` 落 user 行 + 取回 assistant 行与合计 token）；**未接线时三个端点 503 而不是 500**；空消息 400 且不落库；`/foreman/stream` 把工头增量送达到订阅者，而 `/tasks/{id}/stream` **收不到**工头事件（零干扰）；`/metrics` 不因对话变化 | 182②③⑥ |
| `/pairing/*`（决策 182⑦） | 局域网来源：无令牌的写请求 403（报文提示去配对）、**`/foreman/*` 即使 GET 也要令牌**；带正确令牌放行；**回环来源豁免**（局域网形态下本机零摩擦）；只读 GET（看板 / 会话 / 指标）不护；`GET /pairing/token` **仅回环可读**（局域网来源 403）；`POST /pairing/reset` 换一枚且旧令牌随即失效；**默认回环绑定时一切都不要求令牌**；二维码端点接受带 `?pair=` 的 URL（origin 留在白名单内），异 origin 与前缀伪装仍 400 | 182⑦ / 167 / 128 |

## 8. E2E 场景矩阵（L4，决策 149）

harness = FakeAgent（§3.2）+ testkit fixture（§3.3）+ 临时 home + 手动 tick + 按需假时钟。**P0 = 「实现完成」的验收线**（决策 149）。

**E2E-00（冒烟）** spawn 真二进制：启动、`/` 同源托管前端 200（决策 155）、无 provider 创建任务报错、优雅退出（决策 54 / 56）。

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
| 单元 | vitest | `reduce.ts` 归约表逐事件（design §9.1 每行：列归属 / 信号色 / 待办计数 / dossier 开合）；allowed_actions 渲染分组（resume / side_effect、`requires_input`）；NotificationPolicy（cooldown、quiet_hours、cancelled 不弹）；**值班长流式归约 `realtime/foreman.test.ts`（6 条，决策 182③）**：增量按到达顺序累积且期间保持流式态、非工头 / 非增量事件旁落（返回同一 state）、收尾把非空回话收敛为回话并熄灭方块光标、收尾时空 / 全空白回话**不清掉已到达的文字**、断流时已收到的部分原文保留只多一个说明、开新一轮丢掉上一轮的残留；**状态区急停折叠判据 `lib/talkStops.test.ts`（10 条，决策 183）**：一张急停不折叠、**两张以上一张都不展开**、一张时展开它自己、无急停时无展开项、显式收起不弹回、选中项仍在则保持、选中项被处理掉后回落到默认且不悬空、翻转一次只换一张、**展开判据对单张恒展开**、详情没到不报动作数 |
| 组件 | @testing-library/svelte | PendingActions（按所属游标取 cursor_id——决策 91）；DiffReviewPanel（无「拒绝」——决策 23）；StalledBadge（决策 34）；**像素原语 `crate.test.ts`（票 05 / 决策 169）**：量表 16 段与点亮折算、boss 条 20 段与最后一次转红、`retry_exhausted` 权威强制转红、六态映射（灯 / 描边 / 小人节奏）、sprite 非空 SVG + `currentColor` + 工头固定肤色 |
| **契约** | vitest（纯数据 + 解析） | **主题契约模块（票 02 / 决策 169，本 effort 唯一新接缝）**：`theme/contract.test.ts` 几何常量与 theme-6 §2.3 逐项一致、15 枚 sprite 网格合法、六态映射齐备、深浅两套 token 名一致、量表折算边界；`theme/css-parity.test.ts` 读 `app.css` 抽两个 token 块与契约**逐条比对** + 扫描全部组件**禁止 token 块之外出现裸十六进制颜色**（像素纪律的可机器检查形式，白名单两处并注明理由） |
| E2E | playwright（只 Chromium） | **真 axum 后端 + FakeAgent**（临时 home），**十条 35 例**：① happy path（看板 → 详情 → 页签 → diff 审批合入 → **校验合入到 main 的代码符合任务目标**）；② pending → dossier 面板 → resume（琥珀面板、顶栏待办计数）；③ 闸门真跑与失败分流（**真实 Node 工程**，闸门真执行 `npm test`）；④ provider 配错可理解可恢复（中文提示 + 原始诊断 + 「测试连接」）；⑤ UI 三步创建（×5）；⑥ 人工评审分支 + 合并「返回修改」（×3）；⑦ 日志/对话内容 + 刷新恢复（×2）；⑧ 并发第二任务（×3：互不阻塞 / 多游标分支归属 / 基准前移）；**⑨ 像素主题（×6，票 12 / 决策 169）**；**⑩ 对讲台（×10，决策 176 / 182 / 183）**。页面加载**编译期内嵌的真实 bundle**（主流程票 01） |

**像素主题 e2e（票 12 / 决策 169）——`pixel-theme.spec.ts` 六条，全部断言真应用上算出来的样式：**
① 深浅两套 token 计算值 + 切换真的换 token + 圆角 0 / 2px 描边 / `4px 4px 0` 硬投影；
② 缝合像素字体自托管（同源 `/fonts/*` 真被取回、无外部 CDN、无 404 回退）；
③ 看板 = 运转的流水线（12px 信号灯方块、9 刻度迷你轨、16 段量表、列头 sprite + 双帧小人、34px 道具栏槽位、底栏 `▪` 分隔）；
④ 详情 hero 9 站（无 sync-check）+ 站点名非字符字形 + 工位标签盒 active 是 wash 实底；
⑤ 完成横幅（trophy sprite + diff 摘要真数字、点「收下」关闭、刷新不重弹）；
⑥ 移动款（顶栏 138px、6px 纵向链节脊线、灯可跳段且 `scroll-margin-top: 148px`、槽位不缩、触控目标）。
**对讲台 e2e（决策 176 / 182 / 183）——`talk.spec.ts` 十条**：① 路由可达（`#/talk` 与原型写法
`#v-talk` 都落到对讲台、不落 not-found；顶栏入口图标按 chip 节奏 16px）；② 值班板 8 工位（与看板列一一对应、
各一枚 8px 灯，读数与看板同源）；③ 状态区的急停轮是**真数据渲染**的对话框（任务标题、中文理由短标签而**不暴露内部枚举**、
后端下发的恢复动作可下发且下发后该轮消失）；④ 移动款顶栏仍为 **138px**（§5 两处
148px 定值的依据）、值班板收成对话之上的横向灯条；⑤⑥ **两张急停同挂**（决策 183）：两张都**完整落在状态区可见
范围内**、状态区自己不需要区内滚动（几何断言；桌面取 1280×720 这最紧的一档，手机取 430×900 的 38vh 断点）、
默认**一张都不展开**、点开那张后后端下发的恢复动作仍内联可下发、收起后回到默认形态；⑦ **给值班长发话**：
Enter 发送、回话真的来自脚本、**回话里没有按钮**；⑧ 长对话**滚到底之后急停仍在第一屏**（状态区不随时间线滚动）；
⑨ **没有项目也没有任务时输入是真的、发送能拿到值班长的回话**（空 home 的验收锚点，用户故事 11）；
⑩ 发送失败：错误轮进时间线、人说过的话仍在台账里、**输入框内容保留**（决策 182②）。**这条套件的存在理由**：
对讲台是「**和值班长说话**」——⑨ 钉住「不依赖任务」，⑦ 钉住「值班长只说话、不动手」，
⑧ 钉住「全站唯一该响的信号不随时间线滚走」，⑤⑥ 钉住「多张急停同挂时最老的那张不滚出第一屏」
（把「静默滚出」换成「主动展开」），②③ 正面钉住「内容来自后端真实读数、动作来自 `allowed_actions`（决策 101 纯渲染）」。

另：`e2e/screenshots.spec.ts` 在真应用上产出 **7 路由 × 深浅 + 移动 3 视图 × 深浅** 的可重生成截图
（`.scratch/shots/app/*.png`），**默认 skip**，需 `AGENTPIPELINE_SHOTS=1` 才跑——截图是证据不是门
（像素字体跨机渲染差异会引入 flaky 门，故不做字节级 golden 回放）。

**主流程端到端补齐（2026-09-13，`.scratch/agentpipeline-mainflow-e2e/`）：** 详见该目录 spec 与票面。**全部 13 票 done**：01 / 02 / 03 / 04 / 05 / 06 / 07 / 08 / 09 / 10 / 11 / 12 / 13（其中 04 为 `#[ignore]` 真模型冒烟、10 为闸门扩展；08 / 11 / 12 / 13 为过程中暴露并修复的真实缺陷，09 一次暴露 3 个）。

- **票 01 · 浏览器走真实产物**：`webBase` = `apiBase`（后端同源托管内嵌 dist，决策 155），不再经 Vite dev server——此前两条用例**从未加载过用户实际会加载的那份产物**。新增 `watchBundle` / `settleBundle` 守卫（静态资源非 2xx、页面未捕获异常、产物类 `console.error` 在业务断言**之前**裁决）+ `assertEmbeddedBundle` 前置守卫（未内嵌产物时直接提示先 `npm run build && cargo build`，而非静默退化成假绿）；`App` 新增 `repoDir` 供用例用 `git -C <repoDir> show main:<path>` 断言合入产物。
  - **不可用 `waitForLoadState('networkidle')`**：看板与任务详情常驻 SSE 流，连接永不空闲，必然超时；用 `load`（`type="module"` 脚本是 deferred，`load` 会等其执行完）。
  - happy path 增加**合入产物断言**：`main` 上 `src/lib.js` 含脚本写入的实现（且初始「未实现」占位已被替换）、`tests/acceptance.js` 存在、`main` 最新提交主题为该任务提交、任务分支已删除（决策 3）。设计文档 `design.md` 是**任务产物**（落任务目录、不进主干，见 `tools.rs` 的 `task_dir` 语义），故经任务产出文件 API 读取并断言含验收标准 `AC-1`。
- **票 02 · 闸门真跑**：fixture 从「无语言标记空仓库」改为**真实 Node 工程**（`package.json` + 零依赖 `node run-tests.js`，实测约 0.4s）——此前因无语言标记，闸门命令退化为 `true`，闸门成了空操作，而它是主流程必然经过的一环。新增 `expectGateReallyRan`（命令记录里必须有系统测试命令）与 `findFailedGateCommand`（退出码非 0 的失败证据）；新用例 `gate.spec.ts` 覆盖「失败被观测 → 修复后恢复 → 推进到 merge_approval」。
- **两条守卫都做过反向验证**：故意打坏 `index.html` 的资源引用 → 票 01 守卫 1.1s 报 404 根因（而非 60s 超时）；故意把 fixture 退化成无语言标记 → 票 02 守卫报「闸门未真正执行测试命令」。守卫本身是被验过的，不是文档里的一句话。
- **票 11 · 本轮暴露并修复的真实缺陷**：合入只移动 `refs/heads/{default}`、不同步被检出的工作区 → 用户主仓库留下**已暂存**的 M/D（一次 `git commit` 即回滚合入）、磁盘是旧代码、且下一个任务会被决策 61 误判为脏工作区而挂起。修复见决策 158 与 `issues/11-merge-worktree-stale.md`；缺陷由 `git_chain.rs::merge_leaves_default_branch_worktree_consistent` 钉住（修复前红）。
- **票 03 · provider 配错可理解、可恢复**：新增 `provider-misconfig.spec.ts`（E2E-④）——坏 provider（恒 401 mock）→ 断言面板给中文可操作提示（`.msg` 不混原始英文串）、原始诊断保留在 `.ctx`（`PendingContext.diagnostic`，分类信息经 `agent_node` 的重试耗尽包装穿透，见决策 03 的 `LlmErrorKind`）→ `fixProvider()` + 重试 → 推进到 merge_approval。配套「测试连接」端点（`POST /providers/test`，决策 160：对未保存表单值发最小真实请求，成功/失败都 200，掩码语义不弱化）与 provider 表单按钮，L3 契约用例 ×3 + vitest ×3。
- **票 12 · 本轮暴露并修复的真实缺陷**：设计类阶段（architect-design 等）的 `retry_exhausted`「重试执行」goto 落点硬编码 Execute，被 resume 端点的决策 69 入口校验必然 400，而前端把动作错误吞掉 → 用户视角死按钮（触发面是主流程第一步：provider 配错恰挂在 `architect-design.validate_input`）。修复见决策 159 与 `issues/12-retry-goto-deadend.md`；由 `actions.rs::retry_exhausted_goto_lands_on_stage_entry_for_every_stage` 钉住，TaskDetail / Board 补「动作提交失败」横幅。
- **票 05 · UI 三步创建**：`create-flow.spec.ts`（E2E-⑤×5）——空 home 的两处空状态引导、走 UI 建 provider（列表只回显掩码 + 「测试连接」）、建项目（坏路径先得明确报错）、建任务（跳详情 + 描述真的进 prompt）、依赖任务 ID 字段解析。harness 为此增 `seedless`（不播种）与 `prompts()`（读 mock 记录的新节点运行请求）。
- **票 06 · 人工评审分支 + 合并「返回修改」**：`review-branch.spec.ts`（E2E-⑥×3）——human 模式面板三件套、approve/reject 同端点反结论断言、打回意见进流转原因、merge「返回修改」→ 二次推进 → done。harness 增 `reviewMode` 与 `backendLogs()`。过程中暴露缺陷票 13（决策 161）。
- **票 07 · 日志/对话内容 + 刷新恢复**：`logs-reload.spec.ts`（E2E-⑦×2）——命令内容与对话文本可断言（`text` 步骤须置 `submit` 之后，否则工具循环被纯文本提前终止）、无刷新实时推进、刷新恢复 pending 面板、`setOffline` 断网容错 + 收敛、刷新后合入到 done。
- **票 08 · 真进程重启恢复**：`crates/app/tests/restart_recovery.rs`（Rust spawn 形态，票面降级预案；**不推翻决策 152**，补其未覆盖的进程边界）——并行分支窗口 `SIGKILL` → 同 home 重启 → 归队续跑到 done，join 恰一次、无 worktree / 分支残留。暴露孤儿 `running` 挂起缺陷（决策 162）+ mock `Submit` 后收尾文本修正。
- **票 09 · 并发第二任务**：`concurrent.spec.ts`（E2E-⑧×3）——双任务互不阻塞 + 看板多卡归位 + 双 pending 待办计数（`*2`）/ 并行双分支分组渲染 `['[dev]','[test]']` + resume 带对 `cursor_id`（决策 91）/ 基准前移后 approval 重置、重走阶段 A 再审批（决策 96）。harness 增 `additionalTasks` 与**按任务 id 路由**（决策 165；任务标题只出现在 architect prompt，dev/review/test 段落按 id 才分得清是哪个任务）。**一次暴露 3 个串行测试不可见的缺陷**：看板卡动作按钮被整卡导航链接覆盖（决策 164，用户点按钮只跳详情）、SQLite `BUSY_SNAPSHOT`（决策 163①）、libgit2 建 worktree 的 TOCTOU（决策 163②）。
- **票 04 · 真模型全流程冒烟**：`llm_smoke.rs::real_llm_drives_full_flow_to_merge_approval`（`#[ignore]`，不进任何自动门）——真 key + 真模型驱动完整主流程，断言 12 节点各有 run 行 / `submit_metadata` 在真模型格式下可解析 / token 计量 > 0 / 闸门 `npm test` 真跑且退出码 0 / 无 `retry_exhausted`。**实测一轮通过**：本地 OpenAI 兼容代理 + `deepseek-flash`，570s / 794k tokens / 26 runs，途中自动应答 3 次 `UserDecision`。修了冒烟装置三处缺陷：goto 候选固定取首个导致 `gate_recheck` 死循环（改为按序轮换）、失败命令只打退出码丢掉真实原因（补 stdout/stderr 尾部与阶段元数据）、设计文档断言不认绝对路径（两根兜底 + 列实际文件）。详见票面。
- **票 10 · 纳入闸门 + 产物新鲜度守卫**：`scripts/e2e-artifacts.sh` 守卫两层陈旧（前端源码 vs `dist` → 重建 dist；随后 `cargo build -p app` 增量重编，`frontend/dist` 在 `build.rs` 的 `rerun-if-changed` 里）；`make check` 聚合 `lint + test + frontend + e2e`（决策 168 起 Makefile 是唯一权威，justfile 已删除）。守卫经反向验证：改源码不构建 → 触发重建；注入必败断言 → `make check-e2e` 退出码 2。无 CI 已显式记录。见决策 166 / 168。

**playwright 八条 E2E（本批）**：`happy-path`（①）/ `pending-resume`（②）/ `gate`（③）/ `provider-misconfig`（④）/ `create-flow`（⑤×5）/ `review-branch`（⑥×3）/ `logs-reload`（⑦×2）/ `concurrent`（⑧×3），**17 passed**，全过。加上此后的 `pixel-theme.spec.ts`（6 条，决策 169）与 `talk.spec.ts`（10 条，决策 176 / 182 / 183），现行 E2E 共 **十条 35 例**（另有 `screenshots.spec.ts` 2 例默认 skip，见 §9）。

**前端测试状态（2026-09-13，票 18 收尾 + 主流程补齐）：** 单元层已落地并全绿（`frontend/`，vitest，**96 passed / 13 files**：`reduce.ts` 归约表逐事件、SSE 连接层主动重连、`allowed_actions` 渲染分组与 cursor_id、NotificationPolicy、provider 掩码保存与测试连接规则、analyze 轮询、metrics 字段映射、stage_configs payload）。组件层以 vitest + DOM 断言覆盖 PendingActions / DiffReviewPanel / StalledBadge（`svelte-check` 0 error / 0 warning）。**playwright 八条 E2E 已执行 → 17 passed**：用例在 `frontend/e2e/`（`happy-path` / `pending-resume` / `gate` / `provider-misconfig` / `create-flow` / `review-branch` / `logs-reload` / `concurrent`），harness `frontend/e2e/harness.ts`（临时 home + 真 `serve --port 0` 就绪行回读 + 同源内嵌产物 + 按任务路由脚本），跑法 `make check-e2e`，只 Chromium（决策 144）。

> **与决策 151 的显式偏差：** 决策 151 要求「复用 E2E harness、**不维护独立 mock server**」，票 18 的实现未复用 testkit 的 FakeAgent，而是在 `frontend/e2e/harness.ts` 里写了一个 Node 侧的 OpenAI 兼容 SSE mock（按 persona 反查 `(stage, node)`、按轮投喂）。**理由**：playwright 进程（Node）无法直接调用 Rust 的 `testkit::MockLlm`，复用需要一个额外的 Rust helper 二进制并纳入 playwright 的构建前置；v1 以「少一个构建步骤、harness 自包含」优先。**代价**：存在第二份 mock 实现，可能与 Rust 侧契约漂移——它仍必须发出真实适配器能解析的 OpenAI SSE，故「SSE 事件格式 ↔ 前端归约」这条契约仍被覆盖，但**契约漂移风险由本注记显式承担**（后续若把 testkit 的 mock 抽成 helper 二进制，应删掉 Node mock）。决策 151 的其余要求（真 axum 后端、`AGENTPIPELINE_HOME` 指临时目录、两条冒烟）均满足。

## 10. 质量闸门与 traceability（决策 147）

**Makefile（闸门唯一权威，决策 147 / 166 / 168）：** `make check-lint`（`fmt --check` + `clippy -D warnings`）、`make check-test`（`cargo test --workspace`，即 L1 单元 + L2 集成 + L3 API + L4 场景 + 冒烟，**只覆盖 Rust**；另有 `make unit` / `integration` / `api` / `e2e` / `smoke` 分层子集与 `make fmt`）。原先并存的 `justfile` 已删除（决策 168：开发机未装 `just`，两份定义只会漂移），其分层目标已原样搬入 Makefile。分层子集接受三个可选参数（决策 178）：**`PKG=<crate>`** 把作用域收敛到单个 crate（`unit` 层专用，其余层已自带 `-p`；不传则仍是整 workspace），**`TESTS=<文件名>`** 只编/跑某一个集成测试文件（省**编译链接**，是大头：core 的 L2 全部二进制 3m41s vs 单个 `market` 58s），**`FILTER=<用例名>`** 走 cargo 的用例名过滤只跑匹配的用例（省**执行**，很小）。牙齿检查「停用防护 → 确认对应用例变红」用 `make integration TESTS=market FILTER=<用例名>`，实测 6.2s（对照 `make integration` 3m41s）。**作用域收敛一律用 `PKG`，不要写 `make unit -p <crate>`**——make 会把 `-p` 当成自己的 `--print-data-base` 吞掉：cargo 收不到作用域参数（实际跑整 workspace）、近 1900 行 make 数据库被 dump 到 stdout、`<crate>` 被当成不存在的 target 报 `No rule to make target` 并**以退出码 2 结束**（串在 `&&` 之后会静默截断后续步骤）。注意 `PKG` 与 `--workspace` 属两套 unit graph，首次来回切换会整体重建一次，故一次会话内选定一种模式用到底。

**提交前必过 = `make check`（决策 166，扩展 147）：** `lint` + `test` + `frontend` + `e2e` 四项聚合（`make check-frontend` = vitest / svelte-check / vite build，直接在 `frontend/` 下跑 `npm test` / `npm run check` / `npm run build`；`make check-e2e` = playwright，前置 **产物新鲜度守卫** `scripts/e2e-artifacts.sh`）。守卫解决两层陈旧：前端源码比 `frontend/dist` 新则重建 dist；随后 `cargo build -p app` 增量重编（`frontend/dist` 的每个文件都在 `crates/app/build.rs` 的 `rerun-if-changed` 里，故 dist 一变必然重编内嵌资产表）。

**本项目无 CI**（无 `.github/workflows/`，无 git remote）：闸门靠本地执行，这是当前形态而非遗漏。

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
| 170 / 172 | §5 `skills.rs` 单测（两类发现 / 同名让位 / frontmatter 剥离 / 空正文拒绝 / 三态渲染 / 兄弟展开）、`prompts.rs` 三种渲染 + hash 只对全文态敏感、`config.rs::node_skills_*` 与技能声明形态校验、§6 `executor.rs::node_scoped_skills_inject_different_bodies_per_node` 与 `stage_level_skills_still_apply_and_union_with_node_level`、`skill_tool_injects_body_into_next_round_messages`、§7 `stage_config_accepts_node_scoped_skills_and_rejects_unknown` / `stage_config_validates_skill_declaration_shapes` / `stage_config_rejects_skill_with_missing_sibling` / `stage_config_rejects_empty_knowledge_skill_body` | 已有用例（markdown 技能 + 节点级技能 2026-09-14；决策 172 的运行时补齐同批：`Skill` 工具、三态、兄弟展开、声明形态） |
| 171 | §5 `config.rs::default_server_port_is_8788`（缺省 `port` 与 `host` 钉住；缺省绑定与跨源白名单均由 `port` 派生） | 已有用例（默认端口 8787→8788，2026-09-14） |
| 172③（票 08·只读子代理） | §6 `executor.rs::subagent_tool_set_is_read_only`（**安全断言**：子代理工具集恰为 `read_file` / `list_dir`）、`subagent_does_not_inherit_declared_tools`（阶段声明也扩不了权）、`subagent_run_row_carries_parent_and_agent_type`、`subagent_tokens_are_counted_once_on_its_own_run`、`parent_spawns_readonly_subagent_and_gets_summary_back`、`spawn_sub_agent_absent_unless_declared`；§5 `tools.rs::spawn_sub_agent_*` 四条（未启用 / 缺参 / 缺 run 上下文 / 正常摘要）；§6 `scheduler_tick.rs::subagent_runs_are_not_swept_as_node_timeouts`（子代理 run 不得被超时扫描当节点 run 处置） | 已有用例（只读子代理，2026-09-15） |
| 172⑤（票 09·技能导入） | §5 `skill_import.rs` 单测 30 条——结构校验（含 `SKILL.md` / 正文非空 / frontmatter `name` 一致）、**路径穿越**（`..` / 绝对路径 / 深层穿越 / 反斜杠伪装，外加「穿越包不在技能根外留下任何文件」的断言）、同名冲突默认拒绝 + 报出来源 + 显式覆盖整目录替换、目录扫描（描述 / `exists` / 杂物不进清单）、批量逐项结果（一项坏不中断整批）、卸载（含工具型技能拒绝、路径穿越名拒绝）；§7 `api_contract.rs` 十一条（合法 zip 落盘带兄弟文件 / 缺 `SKILL.md` 400 且不落盘 / 穿越 400 / 同名 409 → 确认后覆盖 / 扫描清单 / 扫描描述 / 扫描目录不存在 400 / 批量逐项 / **卸载后引用 fail fast** / 卸载未知 404 / 无网全链路可用）；testkit `skill_fixture.rs` 提供技能目录与 zip fixture（票 11 / 15 复用） | 已有用例（技能导入，2026-09-15） |
| 172⑤（票 10·远程 registry） | §5 `market.rs` 单测 18 条——索引解析（五字段往返 / 缺字段 / 非 JSON / 缺 `skills` 数组 / 摘要非 64 位十六进制 / `source` 非合法 origin，全部 fail fast 而非静默跳过）、搜索（名字 + 描述命中 / 未放行来源不进候选 / 空白名单返回空）、来源白名单（**默认空拒绝一切** / 未放行拒绝并给出可操作报文 / 按 origin 判定，端口不同与后缀伪装都不成立）、摘要校验（命中通过 / 不符时**同时报出期望值与实际值** / 传输层谎报被拒 / sha256 对 NIST 向量）、`origin_of`（仅 http/https、带 userinfo 与 `file://` 拒绝）；§6 `tests/market.rs` 十二条（正常安装落盘 / 摘要不符拒绝且不落盘 / 来源未放行拒绝 + **下载地址跨源同样拒绝** / 索引畸形 / 网络失败且三类报文分得开 / 未知技能 not_found / **穿越包即使摘要对得上也被拒** / 同名冲突默认拒绝 / **传输层摘要是不可信的**——索引钉诚实摘要、传输送换过的字节并谎称摘要相符，必须拒 / **字节实际来源须放行**（挡重定向）/ 索引里的不可落盘名字在**下载前**被拒 / 空白名单在**拉索引前**被拒）；§5 `config.rs` 五条（`[market]` 默认空 / 归一去重保序 / 非法来源 fail fast / 未知键拒绝）；§7 `api_contract.rs` 十四条（搜索 + 安装落盘 / 关键词筛选 / 摘要不符 400 报期望与实际 / 未放行来源在搜索与安装两侧都被拒 / 空白名单 400 且说明怎么开 / 索引畸形 400 / **网络失败 502** / 未知技能 404 / 同名 409 → 确认后覆盖 / 穿越 400 / **市场失败不阻塞本地导入** / 跨源下载地址在搜索与安装两侧都被拒 / `detail` 字段带原始诊断且不混进 `error`）；testkit `market_fixture.rs` 提供 `FakeMarket`（唯一新增接缝，五条路径全部不打真网络） | 已有用例（远程 registry，2026-09-15） |
| 179（票 12·出口控制） | §5 `egress.rs` 单测 17 条——放行侧：本地命令形态一条不误判（`git add && git commit` / `cargo test` / `npm run build` / `make check-lint`）、**提交信息里的 URL 不算出口**（只看每段首个命令词，全串扫描被明确否决）、回环恒放行；拒绝侧：未放行主机（报错三段式：拒了什么 / 怎么放行 / 不是安全边界）、`curl -d @.env <URL>` 的 exfiltrate 形态、精确主机与端口不敏感、**子域通配落在点边界上**（`*.example.com` 不匹配 `notexample.com`）、git 网络子命令 vs 本地子命令、包管理器 install vs test、解释器 + URL、ssh 家族目标抽取（`user@host` / `host:path` / 单目标）、多段命令任一段命中即拒、`sudo` / `VAR=x` 包装不是隐身衣、**选项的取值不顶掉子命令**（`git -C /tmp/repo push` 仍是出口，`npm --prefix x install` 同理；同形本地子命令仍放行）、`allow_all` 显式开关、`check_allow_host` 解析期校验、`NetworkPolicy::from_settings` 的保守默认；§6 `tests/egress.rs` 三条（**被拒的命令真的没跑**——副作用文件不存在、且落 `kanban_node_commands` 同表并带拒绝原因 / 放行路径照常执行 / `allow_all` 是唯一的全放行入口） | 已有用例（出口控制，2026-09-15） |
| 180（票 13·会话续接） | §6 `executor.rs::attempts_start_with_an_empty_conversation_by_default`（**补锁既有行为**：改动前全仓没有一条用例钉住「每次 attempt 对话为空」）、`resume_continuation_carries_the_previous_attempt_messages`（开启后重入带上上一轮的工具往来；且 `messages` 里不混入 system）、`clean_retry_after_a_tool_failure_stays_empty_even_with_continuation_on`（决策 33 不变）、`continued_run_links_back_so_tokens_are_not_double_counted`（必要条件二：`continued_from_run_id` 指向历史 run，`total_tokens` 排除被续接的历史，落库任务总量与函数口径同源）、`context_overflow_path_writes_a_conversation_row`（必要条件一：真实执行路径触发 L4 后该 run 有会话行）；§5 `context.rs::l3_anchor_ignores_the_loaded_history_and_takes_the_current_round`（压缩锚点边界：载入的历史里的 user 消息不当锚点，锚点取本轮第一条；`current_start = 0` 与原行为逐字等价——既有 `l3_summary_inserted_after_first_user_message` 不动） | 已有用例（会话续接，2026-09-15） |
| 181（票 11 / 15 / 16·预览与推荐） | §7 `api_contract.rs` 十六条——预览三项返回 / **特征命中列出具体行号**（对着源文件可定位）/ 未信任 + 全文 400 且报文可操作且不落库 / 信任转换生效（阶段级 + 节点级两处都转、之后全文可存）/ 撤销信任撞全文 400 且配置一字未动 / 无引用时 `changed: 0` 如实回报 / **装前预览不落盘** / 未安装 404；推荐清单按阶段下发并标注装没装 / 一键安装落盘 + 写配置（name + 未信任）/ 正文有特征也进得来但**只能名字态** / 既有声明逐字保留且重复安装不重复追加（已在技能根里则**不重新下载**、只补启用那一步，`note` 如实说明）、**已安装技能可直接启用**（未配市场来源也能落进配置） / 技能不存在 404 且不写配置 / 未配置来源 400 可操作 / 伪阶段 400 / 停用后配置行移除；§5 `config.rs` 七条（信任转换就地改写 / 裸字符串物化 / **降信任撞全文拒绝而非静默降级** / 节点级覆盖 / 无关技能不动配置 / 转换后仍过写入门）；§5 `skill_preview.rs` 十条（三类特征分行命中 / 大小写不敏感 / 一行两类 / 宽松匹配的对照样本 / 推荐映射与手动触发型排除）；前端 vitest `stageConfigs.test.ts`（混合数组读写 / 旧格式零迁移往返 / 未信任不可切全文 / 撤销信任撞全文拒绝 / 节点级技能写回保留其余键 / 空列表删键） | 已有用例（装前预览 + 信任转换 + 推荐与一键安装，2026-09-15） |
| 176 / 182 | §6 `tests/foreman.rs`（21 条：快照字段与原因原文 / 历史字符预算 / 会话循环与收口 / **工具白名单在执行点生效** / 指标不动 / 保留期清理）+ `tests/pairing.rs`（5 条：生成即持久化 / 重置换枚）；§7 `/foreman/*` 六条（会话、空 home 对话、503 未接线、空消息、流式送达且任务流零干扰）+ `/pairing/*` 七条（缺令牌 403、带令牌通过、回环豁免、只读 GET 不护、读取口仅回环、重置使旧令牌失效、缺省回环绑定不要求令牌）；§5 `peer.rs` / `stream.rs` / `server_info.rs` 的对端地址与配对比较；§9 `realtime/foreman.test.ts`（归约 6 条）+ e2e `talk.spec.ts`（8 条，含空 home 可对话、回话里没有按钮、急停滚动后仍在第一屏） | 已有用例（对讲台与配对令牌，2026-09-16） |
| 183 | §9 `lib/talkStops.test.ts`（10 条：一张不折叠 / **两张以上一张都不展开** / 一张时展开它自己 / 无急停无展开项 / 显式收起不弹回 / 选中项仍在则保持 / 选中项被处理掉后回落到默认且不悬空 / 翻转一次只换一张 / **展开判据对单张恒展开** / 详情没到不报动作数）+ §9 e2e `talk.spec.ts` 两条（**几何断言**：两张急停都完整落在状态区可见范围内、状态区不需要区内滚动——桌面 1280×720 与手机 430×900 的 38vh 两档；默认一张都不展开；点开后后端下发的「补充信息并继续」与「取消任务」两钮都在；**在已有一张展开时点另一张会换过去（同时只展开一张）**；收起回到默认形态） | 已有用例（状态区急停折叠，2026-09-15） |
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
Makefile          build / run / desktop / check-* / unit / integration / api / e2e / smoke / fmt
```

**已实现并有用例覆盖（按层）：**

| 层 | 位置 | 用例数 | 覆盖 |
|---|---|---|---|
| L1 单元 | `crates/core/src/**`（in-crate） | 231 | routes 全 `EdgeKind`（含 sync-check backtrack、merge 闸门耗尽收口、code_gate 通过即放行、review 不通过→user_decision）、落点表逐行、metadata 三级降级、FileToolPolicy（realpath / deny / symlink）、脱敏、L1 裁剪 / L2 唯一阈值 / L3 压缩规则表 / L4 兜底、prompt 组装 golden（§10.3 十二节点内嵌模板 + AGENTS.md + stage_configs 消费 + `prompt_template_hash`，票 12）、backtrack 反馈注入范围（决策 126：仅 architect validate_input / execute、首轮不渲染）、allowed_actions 权威表 + 端点按行配对静态检查、焦点投影、指标口径、SSE 事件体（`stage_changed` / `task_done` / `task_cancelled`）、工具真实执行（test-report.md 落任务目录 + **`run_command` 运行期周期心跳**，票 13）、**生产适配器协议解析**（票 13：OpenAI 兼容 / Anthropic 的请求体映射、流 chunk 分片聚合、usage 与 cache token 解析、`[DONE]` / `message_stop` 终止、坏载荷干净报错、base_url 回落） |
| L2 集成 | `crates/core/tests/` | 91 | 游标生命周期（创建 / 分裂 / 合并 / 回退 / 重试 / partial UNIQUE / 永不物理删除 / run 外键不悬空 / **损坏行 fail fast** / **cancel 只挂未启动依赖方** / **backtrack 标过期同事务 + upsert 清除**）、git 链路（init / rebase / 冲突 abort / ff 与非 ff 合入 + `update-ref` 写回 / reset --hard + clean / 清理幂等 / unborn HEAD 明确报错 / 非 origin remote 的基准回落）、scheduler tick 六项职责（超时链 + 进程组终止器 + 节点/阶段/全局超时层级、冲突恢复含复检、依赖三态与恢复、准入、stalled 谓词 `has_runnable_cursor`、**纯 name 重合降级 warning**）、executor 循环（FakeAgent 驱动完整 happy path + sync-check system run 恰一次、单分支 pending 不阻断另一分支、单执行者双保险、元数据失败干净对话重试、会话截断、prompt 组装消费、**`tool_event` start/end 成对发射**）、**生产适配器对 mock server 全链路**（票 13：`testkit::mock_llm` 手写 HTTP server；OpenAI 兼容流聚合 + cache token + conversation_delta + 心跳刷新、Anthropic 双头鉴权 + system 顶层 + tool_result 合并 + 计量归一、deepseek 分发、provider 解析优先级（决策 129 四级：node_overrides > 任务覆盖 > 阶段配置 > 系统默认）、HTTP 401 / 坏流 / 未知 vendor / 无 provider / 禁用 provider 的干净报错） |
| L3 API | `crates/app/tests/api_contract.rs` | 40 | POST/GET /tasks 与过滤、循环依赖与 provider fail fast、`GET /tasks/{id}` 的 allowed_actions 与 blocks、resume 的 409 / 动作集 / 冷却防连点 / **dependency continue 不 spawn** / **goto 入口节点校验**、merge/decision（approve 与 return）、人工评审（**comments 进流转原因**）、retry（**worktree 硬重置 + system 命令入账**）/ cancel / archive / split / model-override、项目 CRUD 与 202 异步分析、provider `***` 回显、**跨源防护矩阵全覆盖**（自定义头 / 无 Origin / 本机 Origin 严格相等 / 恶意 Origin 与**前缀伪装** 403 / 同源 Referer 带路径放行 / GET 不受影响 / **`allowed_origins` 扩权仅精确放行**，决策 157）、SSE 通道、会话与命令 API（**按 task 隔离**，含卸载输出）、任务产出文件与目录逃逸防护、**前端静态资源同源托管**（决策 155：`/` 内嵌 index 或构建提示页、`/assets/{*path}` 原样回放 + Content-Type、未知资产 404） |
| 冒烟 | `crates/app/tests/smoke.rs` | 3 | E2E-00：spawn 真二进制 → 就绪 → **`/` 同源托管 200（决策 155）** → 0700 目录权限 → 无 provider 创建任务明确报错 → SIGINT 优雅退出（退出码 0）；端口占用明确报错 |
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
- playwright 两条 E2E 未执行 → 票 18（`frontend/e2e/`，只 Chromium，`make check-e2e`）；
- 结构性待清理四处 → 票 01（e2e `Flow` 回灌公共模块）/ 票 03（rebase 逻辑回 git 层、action→endpoint 单一事实来源、judge continue 落点复用 `StageLanding`）。

**git 层技术选型（2026-09-12 用户裁决）：** 决策 12（git2 + `spawn_blocking`）与决策 146 原文（生产走系统 git CLI）此前互斥，用户拍板统一 git2——生产 git 层（`crates/core/src/git.rs`）已全部重写为 git2，testkit fixture 保留系统 git CLI 仅作测试脚手架（决策 146 已改旧行）；merge 阶段 B 随之改为内存合入（决策 73 / 97 已补注），git 链路 14 条测试全部在 git2 实现上通过。

