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
| **仓访问的远端地址替换点**（决策 194 修订决策 172⑤ / 143，票 01；原 `SkillRepo` trait 已删，**决策 250**） | `Libgit2Repo`（`crates/core/src/agent/repo.rs`：`head` 只 ls-remote、**不下载 pack**；`list_skills` / `read_skill` 走 libgit2 git 通道，`RemoteRedirect::None` **显式设**、`depth(1)`、字节上限在流式回调里守），缺省对真 GitHub；替换原语是 `with_base(url)` / `AGENTPIPELINE_MARKET_GIT_BASE`（**非类型缝**） | testkit 的**两层离线 fixture**：本地裸仓（快单测）与**离线 smart HTTP**（核心用例），经上述替换点指向它们；**不打真网络** | 这条来源下有多条真网络**无法稳定复现**的失败与策略路径（commit 取不到 / 技能目录不存在 / 对象哈希不符 / 传输超限与中断 / 不跟随跨站重定向），而票面要求它们互不混淆——**替换远端地址**即可钉住（裁决⑦ 的要求不靠 trait：全仓只有一处 `impl`，类型从未变过，变的一直是 URL——决策 250 按删除测试删掉假 seam） |
| **主题契约模块**（决策 169） | `frontend/src/theme/contract.ts`（token / 几何常量 / 15 枚 sprite / 状态映射的唯一事实源）+ 手工镜像的 `app.css` | `theme/contract.test.ts`（纯数据断言）+ `theme/css-parity.test.ts`（读 `app.css` 两个 token 块与契约**逐条比对**，并扫描全部组件禁止 token 块外裸十六进制颜色）+ playwright 在真应用上断言计算样式 | 30+ token 与 15 枚 sprite 的漂移**人工对照不现实**；`app.css` 是手工镜像（不引入代码生成——它还承载全站基元与移动版规则，整体生成化会让手改 CSS 变危险操作），镜像与事实源之间必须由机器发现不一致 |

> 接缝只做可替换、不改语义：超时判定仍以 `Clock` 读数为唯一时钟源（决策 64）。**实现顺序要求：前四个接缝先于业务模块落地**（后补要翻全部模块签名）。第 5 条接缝（决策 194 立、**决策 250 校正形状**——**条数仍是五条**：从「网络出口加一条 `MarketClient`」换到「仓访问」时落地成了一条只有一处 `impl` 的 `SkillRepo` trait，是假 seam，已删；现指**远端地址替换点**（`with_base` / `AGENTPIPELINE_MARKET_GIT_BASE`——生产对 GitHub、测试对离线 fixture 服务，两条「adapter」在服务端））与第 6 条（决策 169，主题契约）同守此界——主题契约只承载**视觉数据**，不承载状态或业务语义，状态语义仍在 `stores` 与 `realtime/reduce.ts`。

> **第二处扩展（决策 210，同样不是新接缝）：`StewardActionRunner`**（`crates/core/src/agent/tools.rs`）。
> 托管放行的自动动作（`task resume continue` / `task unstick`）**怎么执行**由注入的那一位决定——
> core 不认识 HTTP 那一层，而 resume 的唯一实现住在端点里（票 08 / 09）。测试里注入一个记账替身
> 就能断言「闸放行了、执行者被叫到了、**账留下了**」，而生产注入的是走同一份
> `pipeline::resume::apply_resume` 的实现。**不注入 = 不放行**（D 层照旧恒提议），
> 故这条接缝的两侧语义都不是「可选的功能」而是「授权本身」。它与 `for_foreman` 同一个姿态：
> 既有替换点上的一个槽位，不新增一层。
>
> **唯一的一处扩展（决策 182，不是新接缝）：** FakeAgent 的脚本槽此前有「按 `(stage, node)`」与「按既有伪阶段」两路；**工头既不是阶段、也不是既有伪阶段之一**，故按伪阶段那一路**加一个工头位**（`Script::for_foreman()`，见 §3.2 ⑧）。它是既有接缝（LLM 响应流）里的一个槽位，**不引入新的替换点**——本表不因本特性增行。

### 3.2 FakeAgent（决策 142 / 148）

**替换边界：只替换 LLM 响应流，工具层全部真实执行**——write_file 真写（临时 home 内）、run_command 真跑、FileToolPolicy 真拦（决策 104）、输出脱敏真过（决策 118）、L2 卸载真落盘、命令真记 `kanban_node_commands`。集成测试因此同时覆盖整个工具子系统；fake 只是「演员」。

| # | 脚本能力 | 验证的决策 |
|---|---|---|
| ① | 类型化脚本：按 `(stage, node)` 声明 tool_calls 序列，submit_metadata 参数直接用各阶段 serde 结构体 | 38（schema 与脚本编译期同源，不漂移） |
| ② | 工具失败注入：第 N 次某工具失败 | G13 / 33（tool_retry_max 分层，单次工具失败不触发节点重试） |
| ③ | 元数据劣化：文本 JSON / 缺字段 / 坏 JSON | §12.12 三级降级、33 |
| ④ | 心跳与流式节奏控制（停跳 / 慢滴） | 64 / 66 / 100（空闲/绝对超时、心跳源） |
| ⑤ | 超长工具结果注入 | §12.13 L1 裁剪 / L2 卸载（110）/ L3 压缩 / L4 两级兜底（压缩后仍超限 → `pending(context_overflow)`，决策 154） |
| ⑥ | 伪阶段脚本：conflict_check 给 duplicate_risk 等级、validator_cross_check 给合格/不合格 | 60 / 67 / 134 / 135 |
| ⑦ | 子代理**可**脚本化（票 08） | `Script::push_subagent` 单列一个队列——子代理复用父节点的 `(stage, node)`，共用一个队列会让它悄悄吃掉父节点的一步。见 §10 决策 172③ 行。**本行重开了决策 148⑦**（原文「子代理**不**脚本化，开启路径实现后补」）：子代理已实现，故补上脚本能力；**L4 的收口不变**（压缩后仍超限仍走 `pending(context_overflow)`，决策 154） |
| ⑧ | 工头**可**脚本化（决策 182，票 01）：`Script::for_foreman()` 同样单列一个队列（`text` / `tool` / `read_task` / `read_conversation` 四种步；值班长**不用 `submit_metadata`** 收口，故没有 Submit 步） | 工头既不是阶段、也不是既有伪阶段之一，故在伪阶段那一路上加一个**槽位**——既有接缝的扩展，不引入新的替换点（§3.1）。**工具真跑**：`read_task` / `read_conversation` 读的是真 SQLite 台账（决策 148 的替换边界不变） |

**真 LLM 冒烟（`#[ignore]`，手动跑）：两条**——① 单节点：architect-design.execute 一次真调用，断言 rig 适配 + 结构化输出解析可用；② 全流程（主流程票 04）：真 key + 真模型驱动完整主流程到 `pending(merge_approval)`，fixture 为真实可构建小工程使闸门真跑，断言每节点有 run 行、`submit_metadata` 在真模型返回格式下可解析、token > 0、无节点落 `retry_exhausted`，失败时输出定位诊断（哪个 `(stage, node)` 的什么错误）。运行：`AGENTPIPELINE_SMOKE_*` 环境变量（见 `crates/core/tests/integration/llm_smoke.rs` 头部说明）。两条都需真 key，**不进任何自动门**（决策 142）。

### 3.3 testkit（决策 146）

workspace 成员 `crates/testkit`，供 L2 / L4 复用：

| 组件 | 内容 |
|---|---|
| git fixture builder | **系统 git CLI** 搭建（仅测试脚手架；生产 git 层按决策 12 走 git2，2026-09-12 修订决策 146）：`Repo::clean()` / `unborn_head()` / `with_remote(local_path)` / `dirty_worktree()` / `conflict_auto()` / `conflict_hard()` / `project(Language::Rust \| Python \| Node)`（含 `{test_command}` 模板变量）/ `symlink_trap()`（macOS `/private/tmp` realpath，决策 104） |
| 技能来源仓 fixture（决策 194，票 01） | `repo_fixture.rs` 的**两层，各司其职**：**`RepoFixture`**——临时目录里的裸仓 + 一个搭内容的工作树（`new` / `add_file` / `commit` / `tip` / `dir`，默认开 `allowAnySHA1InWant`，好让「按旧 commit 取」与 GitHub 的行为一致），用于扫目录（多形态深度）、递归读子树、三类 not_found、`RepoId` 校验；**它不能带 `depth`**（local transport 直接报 `shallow fetch is not supported by the local transport`，也覆盖不到传输策略）。**`SmartHttp`**——把 `git upload-pack --stateless-rpc` 包成一个离线 HTTP 服务（`serve` / `url` / `requests`），用于真 HTTP 传输 + `depth(1)` + `RemoteRedirect::None` + 中断；`requests()` 收到的请求行供断言「没点添加之前零请求」；`trace()` 是每趟请求的细读（header / 响应状态 / 响应后在途字节），断言失败时一并报出来。**API 里不出现 core 类型**（testkit 经 dev-dependency 链接的是另一份 core，类型过不来）。**两个只有真写一遍才知道的服务端事实**（实现期踩到）：① **`accept()` 返回的 socket 会继承监听 socket 的 `O_NONBLOCK`**（macOS 实测）——`read_line` 在客户端数据未到时立刻 `EAGAIN`，服务端当场收线，客户端报 `unexpected EOF` / `Broken pipe`，而 fixture 一个请求行都没记上；症状是并发跑约 1/8 轮次才中、单跑必过，修法是 `set_nonblocking(false)`，回归用例是`a_client_that_speaks_late_is_still_served`。② **一趟请求一条连接**（`Connection: close`）：改成 keep-alive 后 21 条里 13 条当场红（libgit2 1.9.7 靠服务端关连接标记响应结束），而收线要 `shutdown(Write)` 发 FIN 而不关读半边，免得迟到的字节在 `close()` 那刻攒成 RST（RST 会把已发出的响应一起丢掉）。另：`git upload-pack` 对协议级拒绝（`want` 一个它没有的对象）是**写 ERR pkt-line 后非零退出**，fixture 必须照 `git http-backend` 的样子把 stdout 原样流回、状态仍是 200——当成 500 的话，「那个 commit 取不到」会表现成「网络坏了」 |
| FakeAgent 运行时 | §3.2 的脚本执行器 + `Script::fail_tool_n()` / `Script::metadata_from(struct)` 等构建 API |
| 临时 home | `TestHome::new()` → 设置 `AGENTPIPELINE_HOME`、跑全量 sqlx migrations、返回句柄 |
| 断言助手 | 游标状态断言、run / 会话 / 命令行数断言、SSE 事件录制器、终止器调用记录 |
| **真 GitHub 冒烟**（决策 194，票 01；`#[ignore]`，手动跑） | `crates/core/tests/integration/repo_live.rs`：一条跑三件事——真仓 `head()` 取 tip、**扫描层**在真仓的深度形态上认出技能（多技能、2–5 段）、按一个**非 tip** 的旧 commit（tag 剥出来的那个提交）列技能并读一个目录。运行：`AGENTPIPELINE_MARKET_LIVE=1 cargo test -p agentpipeline-core --test repo_live -- --ignored --nocapture`。**两把锁**：`#[ignore]` 挡"顺手全跑"，环境变量挡"`--ignored` 全跑"。实测一轮通过（tip `959a8e9f…` 上 37 个技能、旧 commit `00ff03c` 上 34 个、读到 3 文件的技能包） |

> **真 GitHub 的用例必须 opt-in**（决策 194，票 01）：默认 `test.skip` + 显式环境变量，照
> `frontend/e2e/screenshots.spec.ts` 的先例。落地形态是 `crates/core/tests/integration/repo_live.rs`
> （`#[ignore]` + `AGENTPIPELINE_MARKET_LIVE`，见上表最后一行）。理由不是洁癖——真 GitHub 的连通性历史上**会偶发中断**
> （实测 `github.com:443` 超时一次 75 s、raw 取较大文件超时两次），放进默认门就是给自己埋 flaky。
> **默认质量门不打真网络。**（这是「截图是证据不是门」的同一条姿态。）

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
| 上下文 | L1 各工具裁剪（read 头 200 行 / run_command 错误行保留 / list_dir 200 项）；L2 阈值 4000 唯一；L3 压缩规则表逐行 + 硬限谓词边界（`compact_and_hard_limit_predicates`）。**L4 的 pending 断言在 L2**（构造它的是执行器，不是 `context.rs`）：`crates/core/tests/integration/executor.rs::context_overflow_ctx`（超限 → `pending(context_overflow)`）+ `context_overflow_path_writes_a_conversation_row`（退出路径补写会话行） | §12.13 / 110 / 154 |
| prompt 组装 | golden（insta）：`[基线前言][工作目录(G12)][AGENTS.md(G3)][persona][技能清单][格式规则]` 顺序；AGENTS.md 加载与缺省注入非空默认；`prompts/` 覆盖生效；`stage_configs.persona_path`（相对 home 解析、存在且非空）与 `persona_append` 生效；内置 §10.3 十二个 agent 节点模板（system+user）全部内嵌且只引用已声明变量；SystemBaseline 工具并集（mandatory 不可移除、forbidden 剔除）；`stage_configs` 的 temperature / max_tokens 透传 `LlmRequest`；user prompt 追加段（gate_recheck / backtrack-feedback / retry-feedback，**首轮为空不渲染**）；`{test_command}` / `{design_doc_path}` 等模板变量；`prompt_template_hash` 稳定、对覆盖与路径变化敏感、**对技能正文敏感**（决策 170，`skill_body_changes_prompt_hash`）；技能三类发现与同名覆盖（`skills.rs`）、工具型 `- {name}` 与知识型 `### {name}` + 正文两种渲染（`prompts.rs::knowledge_skill_body_is_injected` / `tool_skill_renders_as_bare_bullet`）、**节点级技能注入**（`executor.rs::node_scoped_skills_inject_different_bodies_per_node` 断言同阶段两节点各含对方没有的正文） | 51 / 28 / 7 / 109 / 126 / 138 / 31 / 137 / 170 / §10.3 / §10.6 |
| allowed_actions | 权威总表逐行（`(type, context.kind)`→动作集）；**端点配对静态检查**：每个 side_effect 动作必须映射到已注册路由，新增动作忘配端点直接红 | 130 / 101 / 119 |
| 工具集准入（决策 154 的后续票） | `client::known_tool_names` = 8 内置 + `spawn_sub_agent`（判据只有一处，多认一个名字 = 放行一个不存在的工具）；`config::validate_startup` 对未知名 / 非数组 / 非字符串项**拒绝**且报文含未知名字 + 已知集合；`pipeline/model_request.rs::tool_defs`（决策 249 随迁）对同一个错误给**同一种**处置与同一句报文（`client::unknown_tools_message`），不再静默丢弃 + warn | 45 / 172③ / 154 |
| 焦点游标 | 投影规则：pending 优先 / `updated_at` 最新 / 双 pending 取最新 | 92 / 130 |
| 指标 | 逃逸率口径、阶段聚合 SQL、`total_tokens`=Σruns、`total_calls`=LLM run 数（不含 system） | 137 / 100 / 130 |
| SSE | 事件体 `branch` 字段；`conversation_delta` / `tool_event` 字段完整 | 84 / 123 / **244**（`channel` 缺省落 `content`；`tool_event` 带 `agent_type` / `session_id`，故它走同一个 `is_foreman_event`——值班长的工具调用实时到达对讲台，流水线的照旧不进） |
| 适配器的推理声道 | `openai.rs`：`reasoning_content` / `reasoning` 两个字段名都收、与 `content` 并存时**两条都收且不混**、空串不产块、不产推理的模型照旧只有正文；`anthropic.rs`：`thinking_delta` 进同一声道（本适配器**不开启** `thinking` 请求参数，这条只为「对端自己发了」那种情形） | 244 |
| 对端地址与配对 | `peer.rs`：ConnectInfo 归一为 `PeerAddr`、**缺省视为回环**（无 ConnectInfo 的 tower oneshot 契约测试不因此变红）、局域网来源读到非回环、`is_loopback_bind` 覆盖各绑定写法；`stream.rs`：令牌比较 `fixed_length_eq`（长度不同即不等、内容不同即不等）；`server_info.rs`：`pairing_url` 形状固定为 `{base}/?pair={token}`、二维码白名单**允许追加 query** 而前缀伪装与异 origin 仍拒、`resolve_qr_target` 原样保留 query | 182⑦ / 167 |
| 端口绑定策略（决策 213） | `serve.rs` 六条：首选端口空闲时**就用它且不改来源** / 被占用时按开关退让到随机端口并标成 `PortSource::Fallback` / 未开退让时明确报错且错误链里是 `AddrInUse` / **只有 `AddrInUse` 才算「被占用」**（按错误链判而不按报文判——`bind_listener` 那条上下文同时罩着权限与地址不可用，按报文判会让它们静默退让）/ 退让**缺省是关的**（`ServeOptions::default()`）/ `port_source` 的三个串即契约 | 213 |
| 配置清单同一性 | `Settings` 的字段集 == `PipelineOverrides` 的字段集（32 == 32），且**每一个字段都真的合得上**：按类型给每个字段喂一个非默认值、机械化填满整个覆盖层、走 `apply` 后断言**每一项都 ≠ 默认**。**判据遍历默认值对象、不手写字段数组**（手写就又是抄一遍，而那正是本条要消灭的形状）；`env_mode` 走 `apply` 里的特殊路径（`Option<String>` 换更好的报错文案），故探针写成显式常量 `"deny"` 并由第二条用例单独钉住「豁免的是填值方式、不是覆盖关系」——**不设豁免表**（豁免表本身就是一份会漂的清单）。这条抓的是四份平行清单里**唯一静默**的那一份：字段进了两个 struct 却忘进 `set!` 宏清单，该键在 `config.toml` 里写了读回来还是默认值，而其余三份漏写是编译错误 | 258 |

## 6. 集成测试目录（L2）

| 关注点 | 用例 | 决策锚点 |
|---|---|---|
| 游标生命周期 | 创建（`POST /tasks` 同事务 main 游标）；分裂（就地改写 + 插入）；合并 / backtrack（单事务归档 + 插入）；重试归档；partial `UNIQUE(task_id, branch)` 幂等；行永不物理删除、`kanban_node_runs.cursor_id` 外键不悬空 | 90 / 80 / 113 |
| executor 循环 | pending 移出可运行集合；无可运行且有 pending → 退出等 resume；全 `waiting_join` → `advance_join` 恰执行一次；**单游标失败不向上传播** | 89 / 82 / 83 / 107 |
| waiting_join | 唯一写入路径 = `pipeline::advance` 的 `Landing::JoinBoundary`（`crates/core/src/pipeline/advance.rs`，决策 245），executor 与 resume 的并行 skip 共用 | 107 / 245 |
| scheduler tick | 六项职责逐一：超时 / 冲突恢复 / 依赖启动 / 依赖恢复 / 准入 / 提醒+stalled（谓词 `has_runnable_cursor`）；双阈值超时与 effective 值层级（节点 > 阶段 > 全局） | 55 / 66 / 92 |
| 超时处理 | attempt < max → 干净对话重试；耗尽 → `pending(timeout)` 挂该游标（另一分支不受影响） | 33 / 82 |
| 冲突恢复 | 冲突任务**全部**终态才查；复检仍有交集 → 更新 context 不重跑节点 + SSE `pending_updated` | 102 |
| 依赖 | all done → queued；failed / cancelled → `pending(dependency_failed)` 挂 main 游标；依赖 retry → 退回 waiting | 57 / 116 / 90 |
| 准入 | 名额占用 = running + pending；queued 按 slots 放行 | 117 / 98 |
| git 链路 | init（有 remote 先 fetch、以 `origin/{default_branch}` 为基准）；merge A（rebase + `base_commit` 记录 + 闸门）；merge B（内存合入 / ff 与 `--no-ff` / 引用写回）；retry reset（`--hard` + `clean -fdx`）；cancel 清理幂等；unborn HEAD 明确报错 | 41 / 96 / 97 / 73 / 125 / 61 |
| 心跳 | 系统命令起止刷新 `last_activity_at`（600s 命令在 300s idle 下存活）；伪阶段心跳归父 run；流式 token 心跳 | 100 / 88 / 134 |
| DB 并发 | `try_claim_executor` 双连接竞争；DbWriter 写串行化 | 36 / §12.10 |
| 配置 fail fast | `cross_family_judge=true` 无 provider → 拒绝启动；不支持 vendor → 降级 `enabled=0`、被引用才 fail fast；skill 名字不存在 fail fast；知识型技能**正文为空** fail fast（`skills.rs::empty_user_file_is_config_error`、`executor.rs::empty_knowledge_skill_body_refuses_startup`）；节点级技能名字不存在时报错须**定位到节点**（`executor.rs::missing_node_skill_refuses_startup_with_node_in_message`） | 134 / 103 / 47 / 170 |
| 值班长（决策 182 / 188 / 204 / 205 / 206 / 207，票 01–06） | `tests/foreman.rs` **42 条**（2026-09-17：多会话 9 条 + A 层读数 3 条 + 文件域与卸载 3 条 + 档位与 D 层 3 条 + 提议与操作台 3 条）、`tests/env_mode.rs` **17 条**（档位三档语义、两段清单、两层解析、值班长的域与 C / E 层、命令落库与卸载维度）、`tests/foreman_proposals.rs` 13 条（提议生命周期：TTL / 一次一按 / 终态 / 扫描与清理 / 会话归属）、`foreman_sessions_migration.rs` 2 条（旧库升级）、`cursor_lifecycle.rs` 的 `resume_cause_is_recorded_once_and_only_for_real_pending_exits` / `human_decisions_record_their_cause_too` / `restarting_does_not_record_a_resume_cause`：空 home 的快照是空班且不报错、待拍板**带 `pending_reason.message` 原文**、项目列表与已完成计数（cancelled 不计入）、在跑 / 待拍板 / 失败三者分组、历史按**字符预算**裁剪而被裁的仍在库里、超预算也至少留最新一条、会话列出按时间序、空 home 可对话且重载后仍在、LLM 请求带工头身份与占位阶段、模型失败时**人的那句话已落库**、空消息不入账、`read_task` / `read_conversation` **真读台账**并回灌给下一轮、越权工具**在执行点被拒**、未知 task_id 回文本而不是让整轮失败、**工具集恰为清单里那一组（安全断言，2026-09-17 由决策 188 / 207 改写为清单驱动 + 逐个 forbidden 名字的反向断言）**、对话**不动全局指标**、会话合计由落库列求和、保留期到点被清理（假时钟）、清理计数分列上报、对话**不产生任何 task 行**、**人格对人的称呼与界面名牌同词**（决策 193）、**班次之间消息与合计互不污染**、**归档从列表里消失而消息仍在**、**标题取自首句且改名不被冲掉**、**值班长的命令挂会话不挂任务**（迁移 0012 的 CHECK）、**A 层六个读数都答得上话**、**`read_providers` 绝不回显明文密钥**（安全断言）、**值班长读得到家目录与 `logs/`、读不到 `data/`**（补偿而非边界，安全断言；`logs/` 那一半由决策 226 修订，用例随之改名为 `…_the_home_but_not_the_key_store`）、**大结果卸载落会话维度且不写 `tasks/.context`**（硬规矩）、**`deny` 档两侧取证**（广告集摘掉 + 硬发也被拒且什么都没跑）、**`auto` 档下 D 层仍只提议**（决策 206 的「本服务写接口不读档位」）、**三档的 system prompt 逐句核**（不得再出现「读不到文件系统 / 不能执行命令 / 没有动手的权力」那几句已经不成立的话）、**任务族的提议带着态势指纹且取自参数里的 `task_id`**（照调用上下文取会让整条拒执规则静默失效）、**操作台记的那几轮不得被读回成值班长自己的话**（决策 207）、**工具调用在轮次进行中就推事件出去**（`start` / `end` 两相位、身份串与会话对上、任务级流收不到它；**放弃那条 `read_task` 的旧假设**：「查不存在的任务」是**正常回答**而不是故障，故失败那一路改用 `read_file` 造）、**工具失败也走实时声道且相位是 `error`**、**思考原文留在台账那一行**（不产推理时是 `NULL` 而不是空串）、**一轮里跨次模型调用的思考按次累积**（决策 244）；`tests/pairing.rs` 5 条：首读生成并持久化、换句柄打开同一 home 读到同一枚（不随启动重生成）、重置换一枚、未生成过也能重置、令牌字符集可安全落在 `?pair=` 查询里 | 182①④⑤⑥⑦ |

**值班长能力扩面（决策 188 / 206 / 207）：三层都已落地，两条断言各自换了主人。** 2026-09-17 分两批落地。**只读那一半**（票 01）把工具集从「两个台账工具」扩到**清单驱动的 11 个只读工具**（A 层 8 个 + B 层 `read_file` / `list_dir` / `Skill`），那条「工具集恰为约定的两个」的安全断言**改成清单驱动 + forbidden 反向断言**（广告集与执行点白名单同源，都由清单生成）。**写的那一半**（票 03 / 04 / 05 / 06，本批）把 C / D / E 三层接进确认钮，清单扩到 **17 个**（`write_file` / `edit_file` / `run_command` + D 层三族 `task` / `config` / `skills`）。

**落地后当场量到并修掉的一处**（决策 208）：确认钮内联在时间线里，而 1280×720 的窗口下时间线只剩 **26px**（内容区为 0），那颗钮**点不到**——修法是给状态区上限加第二项（先扣掉留给时间线的 160px），几何由 `expectProposalReachable` 四条断言钉住。

两条断言的**归属**（改它们之前先读这一段）：

| 断言 | 在哪 | 现在钉的是什么 |
|---|---|---|
| 工具集清单驱动 + forbidden 反向断言 | `crates/core/tests/integration/foreman.rs::the_foreman_tool_set_matches_the_frozen_contract`（本批扩到读写两层的名字） | **安全边界本身**。「不得含」那一列仍是**够不到手**的三个：`delete_file`（删除不可逆，本批也不给它开）、`spawn_sub_agent`（要注入子代理运行器才有意义）、`submit_metadata`（它是节点向状态机提交结构化元数据的口子）。`write_file` / `edit_file` / `run_command` **已从这一列移走**——它们进了清单，由确认钮兜住（`ask` 档提议、`auto` 直通、`deny` 不广告）。修复轮之后 `repair` 也在清单里，归**环境层**（决策 210③④ 的落地）：档位就是这件事的开关——`ask` 下每一步要人按键、`auto` 下整轮自己跑，而**合入永远人按**；放进 D 层会变成两层按不完的钮（`finish` 的产物本身就是一条提议） |
| 「回话里没有按钮」 | `frontend/e2e/talk.spec.ts` ⑦（本批改写为「回话里没有按钮 + 时间线上**唯一**的钮是操作台提议轮里的确认钮」，并新增两条按下它的用例） | 原意是「值班长不动手」，那时没有反例可造。现在钉的是**两件事同时成立**：值班长的**发言**里一颗钮都没有，而它的**提议**有且只有执行 / 拒绝两颗，且那两颗**不直接改状态**——它们发一次 `POST /foreman/proposals/{id}/execute`，由后端按既有端点重走一遍校验（同一条硬约束的取证在 `crates/app/tests/integration/api_contract.rs`：按下之后文件真的出现在磁盘上、参数过不了端点校验时提议**不消耗**） |

| **值守与修复**（决策 209–212，票 01–13） | `tests/scheduler_tick.rs` 新增 11 条（调度器处置未生效 / 宽限期内不报 / owner 持有超时 / 健康在跑零待办 / 两次 tick 不重复写行 / 停滞提醒发 SSE + `task_stale` / 超时说清「当时在哪一步」/ 慢跑落一条**只播报不唤醒**的待办且一条 run 只落一行 / 项目级 run 的终止者三条 / …）、`tests/foreman.rs` 新增 **23 条**（诊断包「一次调用够不够定因」两条 / 值守轮六条：一次唤醒、攒批不丢、窗口内不醒、无待办零调用、静默、失败不消费 / 节流三条：冷却、触顶留痕、分级工具集正反面 / 托管七条：放行、未托管仍提议、永不自动的动作、触顶与指纹、unstick 进自动集而重启不进、失败回合留痕、空回话的类别 / **修复那条链的线上通路两条**（`repair` 工具 start 真建 worktree、finish 真落提议，且两处留痕都在：班次里一条 + **任务行的 pending 说明**里一条，原来那句「为什么停」不丢；另一条走 `discard`：回收、**保留分支**，并覆盖「又调一次」那一支）、**没人按过的修复 worktree 按年龄回收两条**：一条正面（过了保留期就收 worktree、保留分支、台账里也不挂着它），一条反面（回收失败——项目连目录一起没了——**不许把整趟维护带走**，行照旧被年龄清理删掉））、`tests/repair.rs` **12 条**（worktree 可达性正反面 / 并发建 worktree / 回收规则 / 闸门不过无 diff / 闸门过后带标记的 commit 与范围 / 项目工作区不被碰 / 非 git 仓说清理由 / 提议不按时间过期 / rebase 检查的正反面 / **`finish_repair_round` 的两条收口分支**：闸门不过时无 commit 无 diff 文件无提议、闸门过了时三样齐全）、`tests/executor.rs` 新增 6 条（失败落会话三条 + prompt 原文逐字相等 + 截断标记） | 209 / 210 / 211 / 212 |
| **确认钮够得到**（决策 208） | `frontend/e2e/talk.spec.ts::expectProposalReachable`（**三个**调用者：按下执行 / 按下拒绝 / **修复提议**——最后那个是最高的那一档，闸门读数一行 + 折叠的补丁） | 「内联在时间线里」这件事的**可达性**。四条一起断：时间线 ≥160px、钮**整颗**落在时间线里、钮的中心点用 `elementFromPoint` 命中的是它自己（2026-09-17 截走点击的是输入坞那颗悬出框沿 16px 的名牌）、输入坞与整页都没被顶坏。**牙齿**：把状态区上限退回 `46vh` → 第一条即以 `Received: 26` 变红。两条按下用例因此回到 playwright 默认的 **1280×720**，不再显式设 1280×1000 |

**「状态区为空时零按钮」那条断言保留**（`talk.spec.ts` 的空看板用例）：它钉的是「后端下发的动作集是唯一动作来源」，与上面那条不是一件事。

**端口跨重启稳定（决策 213，2026-09-17）**：`crates/app/tests/integration/port_stability.rs` 2 条——① 写一份
`[server] port = P` 的配置，**不传 `--port`** 起真二进制两次，两次就绪行必须是同一个 P，且
`/server-info.port_source = config`（这是「手机里那本书签下次还打得开」的形态，也是桌面壳的形态）；
② 占住配置里那个端口，用**桌面壳那套 `ServeOptions`**（`port_override = None` +
`port_fallback_to_ephemeral = true`）起服务：必须**换一个端口起来**，且 `/server-info` 的
`port` 是真实端口、`port_source = fallback`。两条的形态差异是**被测行为的差异**：前者要的正是
「跨进程重启」，后者只存在于一条非 CLI 选项上（命令行不暴露退让，`--port` 绑不上就报错）。
故本文件**只允许一处 in-process `serve`**——`TestHome::install_env` 改的是进程级环境变量。

**值班长的定位面（决策 227–239，`/.scratch/foreman-run-failure/`，票 01–06）**：这一批把
「定位成功」的四项判据（决策 230）落成可测的读数与校验点，用例分五处落。**四项判据本身有一条
总闸**：`foreman::one_watch_round_closes_all_four_criteria_on_the_same_run`——一次播报轮里
①（run_id）②（哪一环，失败原文挂在那条 run 上）③（可复核的原始证据：闸门输出真文件 + 命令回执）
④（`【归因】` 结构块，四类之内）齐备，且**四项指向同一条 run**（模型请求台账那一节也归到同一个
`run_id`）。为什么单列：四个读数分开各绿而合起来给不出四项，正是 2026-09-19 的现场（采到正确方法、
却把 run 27 的活栈归到已 failed 的 run 26 名下）。

- **模型请求台账（决策 231，票 01）**：`tests/model_requests.rs` **9 条**——收场请求带 run 归属与
  用量 / 在飞的请求**先于收场可读** / 被丢掉的请求由 Drop 兜底收成 `timeout`（**不留假「在飞」**）/
  失败与中止分得开且**不记假的 0** / 重启把上一进程遗留的在飞请求收成终态 / 值班长的请求落它自己的
  阶段键与班次（哨兵 `run_id = 0` 归一成 NULL）/ 一次 run 内序号自增 / 无归属的调用照样落账。
  另两条在既有文件里：`foreman::the_diagnosis_pack_carries_the_model_requests_of_each_run`
  （读数**必须进诊断包**）与 `app::serve::tests::the_three_model_log_lines_form_a_timeline_in_the_file`
  （三行日志按时间顺序落在文件里——「日志读得到」与「日志里有东西」是两件事）。
- **只读取证（决策 232 / 237，票 02）**：`tests/readonly.rs` **6 条** + `agent::tools::tests` 内联 5 条。
  牙齿都在**副作用**上：`; touch <文件>` 作为参数递进去后文件**不存在**（不经 shell 的证明）/
  白名单外的名字（`sh` / `rm` / `curl`）拒且**留一行台账** / 路径参数越出文件域即拒（`data/`、
  `..`、绝对路径各一，**连着选项写进去的那一种也拒**：`--files0-from=/etc/passwd` 的值半边同样
  要过文件域）/ `sample` 的 pid 只认本进程及其子进程（外人拒、按进程名取样拒）/
  **超时杀掉整个进程组**（`a_timed_out_readonly_command_kills_its_process_group`：`tail -f` 挂住
  → 报「命令超时」+ 终止器被叫到 + pgid 非 0；这条是抽公共管道时补的回归——原先只读那一支抄
  `run_command` 时漏了这句，超时的取证进程会留到天荒地老，而白名单里有 `sample`）/
  `deny` 档照旧广告照旧执行（它改不了任何东西，故不归档位管）。冻结断言按决策 209 的次序**先改后加**。
- **归因结构块（决策 235 / 238，票 03；判据①的校验面见下）**：`foreman::attribution` **6 条**（稳定标识与中文词都认 /
  没给与不可用分开记 / 四类之外不收口 / 重复与自相矛盾都不收口 / 行内提及不算块 /
  **指名的 run 与类别同批走**——`a_named_run_travels_with_the_category`：正整数带上、
  缺省与 `null` 都是「没指名」、`0` / 负数 / 字符串 / 小数 / 布尔都判 `run_id 不是正整数`、
  **两处类别一致而 run 不同判「多处自相矛盾」**）+
  `app::api_contract::the_session_wire_carries_the_parsed_attribution`（解析点在后端，
  界面拿 `attribution` + `attribution_label`，未定位给 `unlocated` 而不是编一个类别）+
  `frontend/src/realtime/foreman.test.ts` 3 条（四类各一个词、未定位 **不给词**）+ 诊断包带出
  「最近一次的归因类别」两条（定位成功与未定位各一；定位成功那条另钉 `run_id` 与类别
  一并带出，未定位那条钉它是 `null`——**判据①在下一轮可对账**）。

**判据①「哪一个 run」的落点（2026-09-19 实测补，决策 230）**：`parse_attribution` 把
`run_id` 与类别**绑在同一行结构块**上校验，`latest_attribution` 把它带进诊断包，
`foreman::one_watch_round_closes_all_four_criteria_on_the_same_run` 在**播报轮端到端**钉住
「回话指名的 run 就是证据实际挂的那条 run」（回话按真实的 `run_id` 拼，断言拿它与结构块比对）
——这一条此前是空的：2026-09-19 那次回话把 run 27 的活栈记在 run 26 名下，
行文、引用格式与类别**全都合规**，没有任何断言拦得住它。
- **`node_overrides` 的读法（决策 236，票 04）**：`foreman::read_stage_configs_echoes_the_node_overrides`
  （回显带 `node_overrides_json` / `persona_append` / `env_mode`）+
  `api_contract::a_config_set_that_would_drop_node_overrides_is_refused`（**工具那一侧**的校验：
  没带而旧有 → 拒、报文说清几个覆盖、**提议不消耗**、旧配置一字未动；带上 / 本来没有 / 显式 `{}` 各一）。
  校验点不在 `PUT /stage-configs` 上：整条替换是那个端点的**既有语义**（界面表单总带全整行）。
- **一轮的收尾语义（决策 233 / 239，票 06）**：`foreman.rs` 3 条——触顶**不再整轮作废**
  （部分结论落库 + `【未收口】` 标注 + 标注里给出实际生效的上限；上限由 `stage_configs` 的
  `max_rounds` 给，用例配 3 轮同时钉住「设置项真的生效」）/ 一句话都没说过的触顶**仍旧按失败
  处置**（类别仍是 `model_no_reply`，没有东西可留时报错才是诚实的）/ 一轮死了之后它提的提议
  **随之失效**（`invalidate_pending_foreman_proposals`：状态 `expired`、行留着可追溯）/
  **只作废那一轮自己提的**（`a_failed_turn_only_invalidates_the_proposals_of_its_own_round`：
  上一轮留下的合规待办不受牵连——判据是 `created_at >= 这一轮开始`，同刻按「这一轮的」收）+
  `api_contract::max_rounds_accepts_only_positive_integers`（正整数 / `0` / 负数 / 留空四种）
  + `config::a_stored_zero_max_rounds_fails_startup_validation` 与
  `foreman::a_stored_zero_max_rounds_fails_startup_on_the_real_read_path`（存量里的 `0` 拒绝启动，
  与 `tools_json` 的未知名字同一姿态；后者打在**真读路径**上——绕过写入校验直接改库，走
  `Store::validate_startup`，钉住 `list_stage_configs` → `into_config` 不许把 `0` 吞成「没配过」，
  否则那条守卫永远不可达）+ 前端 `stageConfigs.test.ts` 4 条（正整数入 payload /
  留空省略 / `0` 与负数在按下之前就被拦 / 预填读回来）。
- **待办补两类（决策 234，票 05）**：`scheduler_tick.rs` 4 条（`run_failed` 当场记且点名 run /
  逐 attempt 各一行 / 任务已转 pending 时不重复记 / 取消留一条唤醒待办）+
  `foreman::run_failures_of_the_same_task_are_collapsed_by_the_cooldown`（决策 234 点名的风险：
  同任务 30 分钟冷却必须对 `run_failed` 真的生效，否则一次重试型故障能烧光每小时配额）。
  **同批改写 3 条既有用例**：`a_terminal_run_behind_an_active_cursor_is_noted` /
  `a_fresh_terminal_run_is_not_yet_a_scheduler_no_effect`（原名 `…is_not_noted_yet`）/
  `many_discoveries_note_rows_once_each`——同一个终态失败现在同时是 `scheduler_no_effect` 与
  `run_failed` 两个事实，断言随之改成**按类别**取证（宽限期内不报处置未生效，**而失败本身当场可见**）。

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
| `stage-configs`（决策 22 / 46，工具名准入见决策 154 的后续票） | 未知阶段键 400（须为真实阶段或伪阶段键之一）；引用不存在 / 被禁用的 provider、不可读的 `persona_path`、会破坏启动的删除一律拒绝且**不落库**；**`tools_json` 声明未知工具名 → 400，报文列出未知名字 + v1 已知工具集（8 内置 + `spawn_sub_agent`）；非字符串项同样拒绝**（旧行为是静默忽略）；**存量配置含未知名字 → 启动校验失败并指明阶段与名字**（不静默放行、不自动清理） | 22 / 46 / 129 / 154 |
| analyze | 202 + `GET /projects/{id}/analysis` 轮询 | 130 |
| `/market/*`（决策 194，取代决策 172⑤ 的自定 registry 三组端点） | 仓名单三端点：读到底 / 归一 + 去重 + **保存即生效** / 非法 `owner/repo` 400 且点名 / 空数组是**显式**关闭而非回落 / `DELETE` 回到 `config.toml`；**跨真进程重启仍在**（决策 257，见 L3 的 `settings_saved_market_repos_survive_a_real_restart`——`serve` 启动路径读回 DB 那一级并装成 override，故 `origin` 如实说 `settings`；少了这一步，界面保存的名单重启后静默回落，而「显式清空」与「没保存过」也就再分不开）；列技能：按仓钉住一个 commit、`refresh=1` 重新 `head()`、`q` 是对已 fetch 那一份的本地过滤；安装：**八类 `kind` 可分辨**（`repo_not_found` 与 `commit_not_found` 同为 404，只有 `kind` 分得开）、`detail` 带原始诊断且不混进 `error`、未放行的仓 400、同名未确认 409、**市场失败不阻塞本地导入**（离线 fixture 走票 01 的 smart HTTP） | 194 |
| 跨源防护矩阵 | 带 `X-AgentPipeline` → 过；无 Origin/Referer（非浏览器）→ 过；恶意 Origin → 403；GET / SSE 不受影响；**配置扩权（决策 157）**：`[server] allowed_origins` / CLI `--allowed-origin` 注入的 origin 精确放行，未配置局域网 origin 与前缀伪装仍 403 | 128 / 157 |
| 静态资源（决策 155） | `GET /` 200：内嵌时为构建产物 index.html、未内嵌时为构建提示页（按 `EMBEDDED_ASSETS` 是否为空断言）；`/assets/{*path}` 原样回放 + Content-Type；未知资产 404 | 155 |
| `/foreman/*`（决策 182 / 204 / 247） | 空 home 可读会话（响应含身份回执 `agent_type` / `stage_key` / `wired`；**一个班次都没有时 `session` 为 `null`**，读端点不建行）；空 home 可经 API 对话（`POST /foreman/messages` 落 user 行 + 取回 assistant 行与合计 token）；班次四件事（新建 201 / 列表按最近活动倒序 / 改名 / 归档后从列表消失而按 id 仍读得到）；**两个班次的 messages 与合计互不污染**；往已归档或不存在**班次**说话分别 400 / 404；**未接线时十二个端点一律 503 而不是 500**（+ 决策 247⑤ 的 `GET /foreman/tools`——静态清单也在列，`/foreman/*` 下没有「接线外可用」的特例）；提议面四个端点（提议列表 / 命令台账 / 执行 / 拒绝）：只给该班次未决的那些、一次一按（第二按 409）、过期 409 且标 expired、态势漂移 409 且提议**保持未决**、未知 id 404、执行后文件真的落盘而参数过不了端点校验时提议**不消耗**、`data/` 目标按下也拒、开门类动作（`pairing` 之流）没有工具名且执行报「还没有接线」；`GET /foreman/tools` 出全量 24 条 `{name, label}`（与清单同序、label 均非空、**不按档位滤**——回执标的是历史上的工具调用，昨天的回执今天仍要能翻译；**只出两个字段**，description / parameters 不出，决策 247⑤）；**`ask` 只留给值班长行**（其余阶段 400，报文指向 `deny` 与 `foreman` 行）；空消息 400 且不落库、连班次都不开；`/foreman/stream` 把工头增量送达到订阅者（增量带 `session_id`），而 `/tasks/{id}/stream` **收不到**工头事件（零干扰）；**`GET /foreman/session` 的 `turn_in_flight` 随一轮的寿命翻转**（决策 260：空 home 为假 → 一轮停在模型调用里为真 → 答完翻回假且台账两句——刷新页面之后界面靠它重新接上那一轮，端点这一条钉的就是这个读数）；`/metrics` 不因对话变化；**托管放行的两个动作都执行得通**（`resume(continue)` 与 `unstick` 各一条端到端——后者的牙齿是「放行 ≠ 执行得通」：
执行者只认 resume 时它会红在 `未知 resume 动作：` 上，而 core 那侧注入替身的用例看不见） | 182②③⑥ / 204 / 210 |
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
| 单元 | vitest | `reduce.ts` 归约表逐事件（design §9.1 每行：列归属 / 信号色 / 待办计数 / dossier 开合）；allowed_actions 渲染分组（resume / side_effect、`requires_input`）；NotificationPolicy（cooldown、quiet_hours、cancelled 不弹）；**值班长流式归约 `realtime/foreman.test.ts`（决策 182③，决策 244 扩容，决策 260 再加 4 条：`maxLedgerId` 取最大行 id / 接手那一刻已有的行不算落地 / 落地那一行是哪种 `kind` 都算 / 空台账与更小 id 都不算）**：增量按到达顺序累积且期间保持流式态、非工头 / 非增量事件旁落（返回同一 state）、收尾把非空回话收敛为回话并熄灭方块光标、收尾时空 / 全空白回话**不清掉已到达的文字**、断流时已收到的部分原文保留只多一个说明、开新一轮丢掉上一轮的残留；**思考与工具的实时声道（决策 244，另 10 条）**：`reasoning` 声道进 `thinking` 而**不进回话正文**（分道的全部理由）、缺省声道按回话处理（老后端不发 `channel`）、两条声道各攒各的互不覆盖、`start` 与随后的 `end` **合成一条**、`error` 也是收尾、**连着查两次同一把工具是两条**（合并判据是「最后一条还没收尾」而不是工具名）、流水线节点与缺 `agent_type` 的工具事件旁落、班次守卫在工具事件上同样成立、收尾与断流都不清掉思考与现场；**托管开关的判据 `lib/stewardship.test.ts`（6 条，决策 210① / 票 14）**：未接线与终态任务**不摆**那颗钮（两条理由与端点的 503 / 400 同一份）、未托管时说的是「只能提议，动手要你按键」、托管中要说清还剩几次自动动作（止损线的可见形态）、用满之后托管仍开着而不再自动动手、打开与关掉走同一个端点（方向由 `enabled` 说）、被拒时把后端那句原样说出来；**状态区急停折叠判据 `lib/talkStops.test.ts`（15 条，决策 183 / 192）**：一张急停不折叠、**两张以上一张都不展开**、一张时展开它自己、无急停时无展开项、显式收起不弹回、选中项仍在则保持、选中项被处理掉后回落到默认且不悬空、翻转一次只换一张、**展开判据对单张恒展开**、详情没到不报动作数、**窄屏（`forceFold`）单张也折**、窄屏恒不展开、窄屏下摊开判据只认人点过的那个 id、窄屏显式收起仍不弹回、**同一份集合宽窄两解**；**对讲台判断抽出的四个 module（决策 251 / 259 / 260）**：`lib/talkTurns.test.ts` 18 条（分类透传读 `kind` / `proactive` 不碰正文、提议按时刻合流、在飞三态与 `partial` 边界、同刻 `rank` 兜底排序、配对只看上游布尔的 spoof）、`lib/menuTrap.test.ts` 20 条（绕回含零长不除零、`decideMenuKey` 逐条、点外关闭）、`lib/delegation-scan.test.ts` 17 条（**静态扫描守卫**——工位灯 / 键盘陷阱 / 回合构造 / 配对 kind / 超时 kind / 动作身份：单测只证新 module 对，守卫才钉住组件仍在用它）、`lib/pipeline.station.test.ts` 7 条（`aggregateStationState` 优先序与四盏灯） |
| 组件 | @testing-library/svelte | PendingActions（按所属游标取 cursor_id——决策 91）；DiffReviewPanel（无「拒绝」——决策 23）；StalledBadge（决策 34）；**像素原语 `crate.test.ts`（票 05 / 决策 169）**：量表 16 段与点亮折算、boss 条 20 段与最后一次转红、`retry_exhausted` 权威强制转红、六态映射（灯 / 描边 / 小人节奏）、sprite 非空 SVG + `currentColor` + 工头固定肤色 |
| **契约** | vitest（纯数据 + 解析） | **主题契约模块（票 02 / 决策 169，本 effort 唯一新接缝）**：`theme/contract.test.ts` 几何常量与 theme-6 §2.3 逐项一致、15 枚 sprite 网格合法、六态映射齐备、深浅两套 token 名一致、量表折算边界；`theme/css-parity.test.ts` 读 `app.css` 抽两个 token 块与契约**逐条比对** + 扫描全部组件**禁止 token 块之外出现裸十六进制颜色**（像素纪律的可机器检查形式，白名单两处并注明理由）；**跨语言 mock fixture `lib/e2e-mock-fixture.test.ts`（票 e2e-mock/01）**：Node 侧 SSE 构造函数与提交进仓库的 `tests/fixtures/e2e_mock_sse.json` 逐字段一致（帧 + 字段位置 + usage），与 Rust 侧两处断言指向同一份 fixture |
| E2E | playwright（只 Chromium） | **真 axum 后端 + FakeAgent**（临时 home），**二十九个 spec 文件 135 例**（2026-09-18 全量实测 **108 passed / 27 skipped**——skip 的 27 条是两组 opt-in 套件（截图 / 两轮审计取证），不进默认门；**其中 6 个 spec / 23 例是第二轮 UI/UX 审计本批新加的**：`ux2-failure-paths`（5）失败态与出路、`ux2-reentrancy`（2）对话框重入护栏、`ux2-semantics`（4）地标与页签语义、`ux2-geometry`（3）坞 / 状态行 / 档案盒三处几何、`ux2-flows-and-copy`（4）新建跳转与表单校验与文案、`ux2-resilience`（5）超时 / 断线 / 长值；对讲台一组由 13 条增至 **29 条**（2026-09-18，决策 218 / 220 / 221：折行档 480–899 的版面与 ⋯ 班次菜单、坞与回执的分档、两枚班次标记含跨设备那一组——逐条见下面 218 / 220 / 221 那一行）：两条班次用例（决策 204）+ 两条确认钮用例（提议轮渲染 / 按下执行真的落盘 / 按下拒绝，决策 207））：① happy path（看板 → 详情 → 页签 → diff 审批合入 → **校验合入到 main 的代码符合任务目标**）；② pending → dossier 面板 → resume（琥珀面板、顶栏待办计数）；③ 闸门真跑与失败分流（**真实 Node 工程**，闸门真执行 `npm test`）；④ provider 配错可理解可恢复（中文提示 + 原始诊断 + 「测试连接」）；⑤ UI 三步创建（×5）；⑥ 人工评审分支 + 合并「返回修改」（×3）；⑦ 日志/对话内容 + 刷新恢复（×2）；⑧ 并发第二任务（×3：互不阻塞 / 多游标分支归属 / 基准前移）；**⑨ 像素主题（×6，票 12 / 决策 169）**；**⑩ 对讲台（×29，决策 176 / 182 / 183 / 184 / 192 / 204 / 207 / 208 / 218 / 220）**；**⑪ 手机访问页与配对令牌（×2，决策 186 / 189 / 190 / 191）**；**⑫ 技能市场设置页（决策 187 / 194，本批按新语义重写）**；**⑬ 技能市场安装（决策 177 / 181 / 187 / 194，本批换血：fixture 换成离线 smart HTTP 的 git 仓）**。页面加载**编译期内嵌的真实 bundle**（主流程票 01）|

**像素主题 e2e（票 12 / 决策 169）——`pixel-theme.spec.ts` 六条，全部断言真应用上算出来的样式：**
① 深浅两套 token 计算值 + 切换真的换 token + 圆角 0 / 2px 描边 / `4px 4px 0` 硬投影；
② 缝合像素字体自托管（同源 `/fonts/*` 真被取回、无外部 CDN、无 404 回退）；
③ 看板 = 运转的流水线（12px 信号灯方块、9 刻度迷你轨、16 段量表、列头 sprite + 双帧小人、34px 道具栏槽位、底栏 `▪` 分隔）；
④ 详情 hero 9 站（无 sync-check）+ 站点名非字符字形 + 工位标签盒 active 是 wash 实底；
⑤ 完成横幅（trophy sprite + diff 摘要真数字、点「收下」关闭、刷新不重弹）；
⑥ 移动款（**导航钉屏幕底缘成页签栏、道具栏行只在看板露出**，看板档顶栏 52px / 非看板档 **0px**、
页签栏 58px 且四枚页签 ≥44px、状态条叠页签栏上方、铭牌行不渲染、
6px 纵向链节脊线、`scroll-margin-top` 读 `--topbar-h`、槽位不缩、触控目标；决策 243）。
**对讲台 e2e（决策 176 / 182 / 183 / 192 / 193 / 204 / 207）——`talk.spec.ts` 十七条**：① 路由可达（`#/talk` 与原型写法
`#v-talk` 都落到对讲台、不落 not-found；顶栏入口图标按 chip 节奏 16px）；② 值班板 8 工位（与看板列一一对应、
各一枚 8px 灯，读数与看板同源）；③ 状态区的急停轮是**真数据渲染**的对话框（任务标题、中文理由短标签而**不暴露内部枚举**、
后端下发的恢复动作可下发且下发后该轮消失）；④ 移动款顶栏为 **0px**（对讲台**不是看板路由**，
窄档道具栏行不露出、导航行已钉到屏幕底缘——决策 243；这是 `--topbar-h` 的来源，各钉位按它算，
**0 也是合法值**）、值班板收成对话之上的横向灯条；⑤⑥ **两张急停同挂**（决策 183）：两张都**完整落在状态区可见
范围内**、状态区自己不需要区内滚动（几何断言；桌面取 1280×720 这最紧的一档，手机取 430×900 的 38vh 断点）、
默认**一张都不展开**、点开那张后后端下发的恢复动作仍内联可下发、收起后回到默认形态；⑦ **给值班长发话**：
Enter 发送、回话真的来自脚本、**回话里没有按钮**、两块名牌各按名分渲染（对面「值班长」、人这一侧与输入坞都是「值班经理」，决策 193）；⑧ 长对话**滚到底之后急停仍在第一屏**（状态区不随时间线滚动）；
⑨ **没有项目也没有任务时输入是真的、发送能拿到值班长的回话**（空 home 的验收锚点，用户故事 11）；
⑩ 发送失败：错误轮进时间线、人说过的话仍在台账里、**输入框内容保留**（决策 182②）；**⑪ 输入法回车不发送**（决策 184，见下「决策 → 落点」表的 184 行）；**⑫⑬ 窄屏版面（决策 192，自带装置）**：静置版面两条**几何**断言（单张急停已折成摘要条、状态区不再区内滚、对话区 ≥320px、输入坞底边与底栏顶边严丝合缝、无对话时整页不空滚），以及长对话滚到底之后**摘要条仍钉在顶栏下沿、输入坞仍贴在底栏上沿**（这两条合起来才证伪得了「把状态区放回文档流」那种退化——静止时它也看起来在顶上）。**⑭⑮ 班次（决策 204）**：⑭ 新建 / 切换 / 重命名 / 归档四件事走真界面，且**换会话不动看板**（切班次前后状态区那张急停的文本一字不差）、**chip 行非 sticky 也不在页头里**；⑮ 窄屏下 chip 行横滚不折行、**对话区仍 ≥320px**（chip 行长在会滚的时间线**里面**，故时间线自己的盒子一点没变——这是「不新增钉住物、不动页头」那条硬约束的机器门）。

**这条套件的存在理由**：
对讲台是「**和值班长说话**」——⑨ 钉住「不依赖任务」，⑦ 钉住「回话里没有按钮；时间线的钮两处——提议轮确认钮与提问轮选项钮（决策 265）」（原「只说话、不动手」按决策 188/207/265 改写），
⑧ 钉住「全站唯一该响的信号不随时间线滚走」，⑤⑥ 钉住「多张急停同挂时最老的那张不滚出第一屏」
（把「静默滚出」换成「主动展开」），②③ 正面钉住「内容来自后端真实读数、动作来自 `allowed_actions`（决策 101 纯渲染）」，⑭⑮ 钉住「换会话 ≠ 换看板」。

另：`e2e/screenshots.spec.ts` 在真应用上产出 **7 路由 × 深浅 + 移动 3 视图 × 深浅** 的可重生成截图
（落在 `.scratch/shots/app/*.png`；该目录**不入库**，跑一次即重新生成），**默认 skip**，需
`AGENTPIPELINE_SHOTS=1` 才跑——截图是证据不是门
（像素字体跨机渲染差异会引入 flaky 门，故不做字节级 golden 回放）。

**UX 审计后的界面整备（2026-09-16，`.scratch/ux-audit/`，决策 195–203）：** 27 张票分 A 叠
（纯实现修正）与 B 叠（先决策、后实现），实现见各页文件。本 effort **不新增任何可测试性接缝**，
测试全部落在既有两条上（主题契约模块 + 前端 e2e harness），新增的机器门与既有检查同族：
`theme/contrast.test.ts`（对比度分档门，从契约读值，决策 195）、`theme/css-parity.test.ts`（既有
像素纪律）、`lib/copy-discipline.test.ts`（面向用户的文案里不出现内部编号，决策 199）、
`lib/behavior-map.test.ts`（`frontend-design.md` §12.3 行为映射表的悬空引用检查，决策 199）、
`lib/pipeline.geometry.test.ts`（脊线坐标与列宽同源，决策 196）。新增 e2e：
`board-overflow` / `modal-keyboard` / `settings-landing` / `settings-empty-and-copy` /
`metrics-entry` / `pending-dossier`（都进 `make check-e2e`）；审计取证用的
`e2e/ux-audit.spec.ts` 仍**默认 skip**（`UX_AUDIT=1` 才跑，截图落 `.scratch/ux-audit/*.png`，不入库）。

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
- **票 08 · 真进程重启恢复**：`crates/app/tests/integration/restart_recovery.rs`（Rust spawn 形态，票面降级预案；**不推翻决策 152**，补其未覆盖的进程边界）——并行分支窗口 `SIGKILL` → 同 home 重启 → 归队续跑到 done，join 恰一次、无 worktree / 分支残留。暴露孤儿 `running` 挂起缺陷（决策 162）+ mock `Submit` 后收尾文本修正。
- **票 09 · 并发第二任务**：`concurrent.spec.ts`（E2E-⑧×3）——双任务互不阻塞 + 看板多卡归位 + 双 pending 待办计数（`*2`）/ 并行双分支分组渲染 `['[dev]','[test]']` + resume 带对 `cursor_id`（决策 91）/ 基准前移后 approval 重置、重走阶段 A 再审批（决策 96）。harness 增 `additionalTasks` 与**按任务 id 路由**（决策 165；任务标题只出现在 architect prompt，dev/review/test 段落按 id 才分得清是哪个任务）。**一次暴露 3 个串行测试不可见的缺陷**：看板卡动作按钮被整卡导航链接覆盖（决策 164，用户点按钮只跳详情）、SQLite `BUSY_SNAPSHOT`（决策 163①）、libgit2 建 worktree 的 TOCTOU（决策 163②）。
- **票 04 · 真模型全流程冒烟**：`llm_smoke.rs::real_llm_drives_full_flow_to_merge_approval`（`#[ignore]`，不进任何自动门）——真 key + 真模型驱动完整主流程，断言 12 节点各有 run 行 / `submit_metadata` 在真模型格式下可解析 / token 计量 > 0 / 闸门 `npm test` 真跑且退出码 0 / 无 `retry_exhausted`。**实测一轮通过**：本地 OpenAI 兼容代理 + `deepseek-flash`，570s / 794k tokens / 26 runs，途中自动应答 3 次 `UserDecision`。修了冒烟装置三处缺陷：goto 候选固定取首个导致 `gate_recheck` 死循环（改为按序轮换）、失败命令只打退出码丢掉真实原因（补 stdout/stderr 尾部与阶段元数据）、设计文档断言不认绝对路径（两根兜底 + 列实际文件）。详见票面。
- **票 10 · 纳入闸门 + 产物新鲜度守卫**：`scripts/e2e-artifacts.sh` 守卫两层陈旧（前端源码 vs `dist` → 重建 dist；随后 `cargo build -p app` 增量重编，`frontend/dist` 在 `build.rs` 的 `rerun-if-changed` 里）；`make check` 聚合 `lint + test + frontend + e2e`（决策 168 起 Makefile 是唯一权威，justfile 已删除）。守卫经反向验证：改源码不构建 → 触发重建；注入必败断言 → `make check-e2e` 退出码 2。无 CI 已显式记录。见决策 166 / 168。

> **⑫⑬ 两支技能市场用例（决策 187 / 194，2026-09-16 换血）**：⑬ 的离线装置由 `frontend/e2e/gitRepo.ts` 起（裸仓 + 只服务它的 smart HTTP，`AGENTPIPELINE_MARKET_GIT_BASE` 指回本机回环），旧的手搓 ZIP registry（`e2e/marketRegistry.ts`）随之退场。⑫ 由「白名单是空的 / 不允许远程安装」改写成「仓名单可加可存可退回 + 非法 `owner/repo` 当场被拦 + 冷启动名单零网络」与「**显式清空 ≠ 未保存过**：清空后不回落配置文件，点交还才回落」；⑬ 的三条分别是「添加一个仓 → 按父路径分组列出技能 → 装一个并看到三项预览」「同名重装不静默覆盖、确认后覆盖安装成功」「**列表钉住浏览那一刻的 commit**：远端前进后不刷新仍是那一份、刷新才换」——最后一条正是决策 194 裁决⑤ 的落点。**两条都保留、不合并**：它们是两个独立失效面（界面与配置的两级关系 / 装得下来落得对）。

**playwright 八条 E2E（本批）**：`happy-path`（①）/ `pending-resume`（②）/ `gate`（③）/ `provider-misconfig`（④）/ `create-flow`（⑤×5）/ `review-branch`（⑥×3）/ `logs-reload`（⑦×2）/ `concurrent`（⑧×3），**17 passed**，全过。加上此后的 `pixel-theme.spec.ts`（6 条，决策 169）/ `talk.spec.ts`（11 条，决策 176 / 182 / 183 / 184）/ `lan-bind.spec.ts`（2 条，决策 186 / 189 / 190 / 191）/ `market.spec.ts`（决策 187 / 194，⑫）/ `market-install.spec.ts`（决策 177 / 181 / 187 / 194，⑬——**本批换血**：fixture 从「手搓 ZIP + 普通 HTTP」换成**离线 smart HTTP 的 git 仓**，见 §10 的 177 / 181 / 187 / 194 行与上面的 ⑫⑬ 注），**现行 E2E 共 22 个 spec / 100 例，默认跑 85 例**（2026-09-18 全量实测 **85 passed / 15 skipped**；15 例 skip 是 `ux-audit.spec.ts` 13 例与 `screenshots.spec.ts` 2 例——截图与审计取证是证据不是门，见 §9）。本批（值守与修复）新增两个 spec：`talk.spec.ts` 的「修复提议」那一条（走**真**那条链：`repair` 工具 start → 写补丁 → finish 真跑 `npm test --silent` → 落一条 `kind=repair` 的提议，再按决策 208 的视口量那颗「合入」钮——`expectProposalReachable` 的第三个调用者）与 `stewardship.spec.ts`（托管开关摆得出、拨得动、与库里的那一列一致）。harness 为此多一个 `setForemanRounds`（testkit `set_script` 的等价物）：`repair_id` 只有跑起来才知道，脚本得事后注入——那正是本文件此前记过的那个口子。

**前端测试状态（2026-09-13，票 18 收尾 + 主流程补齐）：** 单元层已落地并全绿（`frontend/`，vitest，**96 passed / 13 files**：`reduce.ts` 归约表逐事件、SSE 连接层主动重连、`allowed_actions` 渲染分组与 cursor_id、NotificationPolicy、provider 掩码保存与测试连接规则、analyze 轮询、metrics 字段映射、stage_configs payload）。组件层以 vitest + DOM 断言覆盖 PendingActions / DiffReviewPanel / StalledBadge（`svelte-check` 0 error / 0 warning）。**playwright 八条 E2E 已执行 → 17 passed**：用例在 `frontend/e2e/`（`happy-path` / `pending-resume` / `gate` / `provider-misconfig` / `create-flow` / `review-branch` / `logs-reload` / `concurrent`），harness `frontend/e2e/harness.ts`（临时 home + 真 `serve --port 0` 就绪行回读 + 同源内嵌产物 + 按任务路由脚本），跑法 `make check-e2e`，只 Chromium（决策 144）。

> **与决策 151 的显式偏差：** 决策 151 要求「复用 E2E harness、**不维护独立 mock server**」，票 18 的实现未复用 testkit 的 FakeAgent，而是在 `frontend/e2e/harness.ts` 里写了一个 Node 侧的 OpenAI 兼容 SSE mock（按 persona 反查 `(stage, node)`、按轮投喂）。**理由**：playwright 进程（Node）无法直接调用 Rust 的 `testkit::MockLlm`，复用需要一个额外的 Rust helper 二进制并纳入 playwright 的构建前置；v1 以「少一个构建步骤、harness 自包含」优先。**代价**：存在第二份 mock 实现，可能与 Rust 侧契约漂移——它仍必须发出真实适配器能解析的 OpenAI SSE，故「SSE 事件格式 ↔ 前端归约」这条契约仍被覆盖。（决策 151 的其余要求——真 axum 后端、`AGENTPIPELINE_HOME` 指临时目录、两条冒烟——均满足。）

> **该漂移风险自 2026-09-17 起由 fixture 机械守住（票 e2e-mock/01，决策 151 的 2026-09-13 修订）：** 原先「由本注记显式承担」的风险，现在落成一份**提交进仓库**的 `tests/fixtures/e2e_mock_sse.json`——三种步骤形态（tool call / submit / text）的**期望 SSE 文本**。三处断言都指向它：Node 侧 `frontend/src/lib/e2e-mock-fixture.test.ts`（import `frontend/e2e/sse.ts` 的 `sseTool` / `sseText` 产出后逐字段比对）、Rust mock 侧 `crates/testkit/src/mock_llm.rs::sse_helpers_match_the_shared_golden_fixture`、消费侧 `crates/core/src/agent/providers/openai.rs::shared_golden_fixture_parses_into_the_expected_chunk_sequence`（逐条喂 `parse_chunk` 断言块序列）。**任一侧漂移即有一侧变红**，不再依赖人去对照两份实现。
>
> 口径（fixture 头注同）：**字节层只钉 SSE 帧**（`data: ` 前缀 / 空行分帧 / `[DONE]` 终止）与字段位置（`choices[0].delta.tool_calls[]` 的 `index`/`id`/`function.name`/`function.arguments`、`usage.prompt_tokens`/`completion_tokens`）；**JSON 键序不是契约**（serde_json 按键序输出、JS 按插入序），两侧都比对解析后的结构。工具调用 `id` 现场生成、`function.arguments` 由产出方字符串化，比对前归一。**更新方式**：fixture 是手工维护的（设计意图——没有任何生成步骤），改 mock 的 SSE 形状时**同时改 fixture 与三处断言**，改错改漏会立刻红。
>
> **已知覆盖缺口（票 e2e-mock/01 补上）：** Rust `from_script` 的伪阶段路由此前只有 `conflict_check` / `validator_cross_check` 两条，`project_analysis` 无法脚本化——现已补第三条（`PSEUDO_MARKERS` 3 条，与 Node 侧 `PERSONA_ROUTES` 对齐），并有 `pseudo_markers_survive_prompt_assembly` 钉住「内嵌 persona 过完 `build_system_prompt` 后 marker 仍命中」。**一条已知限制**：伪阶段配置写了 `persona_path` 时（它覆盖内嵌 persona），该伪阶段的 marker 不在 system prompt 里，脚本化 mock **无法路由**——这类用例须走 `MockLlm::start` 的静态路由（见 `PSEUDO_MARKERS` 的文档）。

## 10. 质量闸门与 traceability（决策 147）

**Makefile（闸门唯一权威，决策 147 / 166 / 168）：** `make check-lint`（`fmt --check` + `clippy -D warnings`）、`make check-test`（`cargo test --workspace`，即 L1 单元 + L2 集成 + L3 API + L4 场景 + 冒烟，**只覆盖 Rust**；另有 `make unit` / `integration` / `api` / `e2e` / `smoke` 分层子集与 `make fmt`）。原先并存的 `justfile` 已删除（决策 168：开发机未装 `just`，两份定义只会漂移），其分层目标已原样搬入 Makefile。分层子集接受三个可选参数（决策 178，**语义由决策 218 修订**）：**`PKG=<crate>`** 把作用域收敛到单个 crate（`unit` 层专用，其余层已自带 `-p`；不传则仍是整 workspace），**`TESTS=<模块名>`** 收窄到某一个集成测试文件的用例。**决策 218 把集成测试按 crate 合成单一二进制**（33 个文件 → 3 个二进制；测试二进制 37 → 7），于是每个测试文件成了一个**模块**、用例全名形如 `<文件模块>::<用例>`，`--test <文件名>` 不复存在——`TESTS` 改走**用例名前缀**过滤，即**编译不再收敛、执行仍然收敛**（改一个文件要重编整个二进制；编好之后单跑一个文件几乎不花时间）。合并的净收益实测：core 的 L2 编译 CPU 从 445 CPU-s 降到 317 CPU-s（**−29%**）。道理是「每个 crate root 都要付一次的固定成本」——往合并二进制里多塞一个文件约 **0.15 CPU-s**，而单独建一个文件要 **1.7–2.6 CPU-s**，**`FILTER=<用例名>`** 走 cargo 的用例名过滤只跑匹配的用例（省**执行**，很小）。牙齿检查「停用防护 → 确认对应用例变红」用 `make integration TESTS=<模块> FILTER=<完整用例名>`。**与 `TESTS` 同用时 `FILTER` 必须写完整用例名**：二者串成 `<模块>::<用例>` 作为**一个子串**交给 harness，写半截（如 `rebase`）会因中间隔着模块前缀而匹配到 **0 条**——不报错，静默通过，是这里唯一的坑。只给 `FILTER` 时仍是全二进制的子串匹配，行为同旧。**作用域收敛一律用 `PKG`，不要写 `make unit -p <crate>`**——make 会把 `-p` 当成自己的 `--print-data-base` 吞掉：cargo 收不到作用域参数（实际跑整 workspace）、近 1900 行 make 数据库被 dump 到 stdout、`<crate>` 被当成不存在的 target 报 `No rule to make target` 并**以退出码 2 结束**（串在 `&&` 之后会静默截断后续步骤）。注意 `PKG` 与 `--workspace` 属两套 unit graph，首次来回切换会整体重建一次，故一次会话内选定一种模式用到底。

**提交前必过 = `make check`（决策 166，扩展 147）：** `lint` + `test` + `frontend` + `e2e` 四项聚合（`make check-frontend` = vitest / svelte-check / vite build，直接在 `frontend/` 下跑 `npm test` / `npm run check` / `npm run build`；`make check-e2e` = playwright，前置 **产物新鲜度守卫** `scripts/e2e-artifacts.sh`）。守卫解决两层陈旧：前端源码比 `frontend/dist` 新则重建 dist；随后 `cargo build -p app` 增量重编（`frontend/dist` 的每个文件都在 `crates/app/build.rs` 的 `rerun-if-changed` 里，故 dist 一变必然重编内嵌资产表）。

**本项目无 CI**（无 `.github/workflows/`，无 git remote）：闸门靠本地执行，这是当前形态而非遗漏。

**构建缓存清理（非闸门，`make sweep` → `scripts/sweep-artifacts.sh`）：** 只删**可再生**的中间产物——`*/target/{debug,release}/incremental/`、`deps/*.rcgu.o`（增量编译**每次会话**产生的目标文件，文件名带会话哈希、cargo 从不回收；2026-09-18 实测 root 一处堆了 **131,924 个 / 实际占用 7.1 GB**）、`target/doc`；第三方 rlib / rmeta 与 `*/build/` 一律保留，故清完不必重编整棵依赖树。**本机已装的 `cargo-sweep` 在这里没用**：它按**访问时间**判「过时」，而堆积按时间算全是**新**的——实测 `--stamp` 报 **0**、`--time 4` 只有 61 MiB，而项目当时实占 **29 GB**、其中 18 GB 正是这一类。桌面壳是独立 workspace（决策 156），两个 target 树都在扫描范围内。`DRY=1 make sweep` 只报将删什么。**逆向验证（2026-09-18）**：sweep 释放 1003.3 MB 后，`cargo build --workspace` 与 `cargo test --workspace --no-run` 都**没有任何 Compiling 行**（0 个 crate 编译、7 个测试二进制原样），`cargo test -p agentpipeline-core --lib` **469 passed / 0 failed**；DRY 预估与实删逐字节一致（267.3 MB）。**体积按 `du`（实际占用）报而不按 `stat`（逻辑字节）**：APFS 会压缩这些目标文件，两者能差一个数量级——实测 `.rcgu.o` 逻辑 735 MB 而删掉它几乎没让 `df` 动，报逻辑字节会高估释放量。核选项 `make clean` 不变（`cargo clean`，连第三方产物一起删，下次冷编本机实测约 20 分钟）。

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
| 170 / 172 / 185 | §5 `skills.rs` 单测（技能根发现 / frontmatter 剥离 / 空正文拒绝 / 三态渲染 / 兄弟展开；**`path_executables_are_not_skills`** 直接钉「`sh` 不在可用技能池里」、`unknown_skill_name_is_config_error_not_silent_name_only` 扩成 `sh` 与虚构名**都一样报错**）、`prompts.rs` 三种渲染 + hash 只对全文态敏感、`config.rs::node_skills_*` 与技能声明形态校验、§6 `executor.rs::node_scoped_skills_inject_different_bodies_per_node` 与 `stage_level_skills_still_apply_and_union_with_node_level`、`skill_tool_injects_body_into_next_round_messages`、§7 `stage_config_accepts_node_scoped_skills_and_rejects_unknown` / `stage_config_validates_skill_declaration_shapes` / `stage_config_rejects_skill_with_missing_sibling` / `stage_config_rejects_empty_knowledge_skill_body` | 已有用例（markdown 技能 + 节点级技能 2026-09-14；决策 172 的运行时补齐同批：`Skill` 工具、三态、兄弟展开、声明形态） |
| 171 | §5 `config.rs::default_server_port_is_8788`（缺省 `port` 与 `host` 钉住；缺省绑定与跨源白名单均由 `port` 派生） | 已有用例（默认端口 8787→8788，2026-09-14） |
| 172③（票 08·只读子代理） | §6 `executor.rs::subagent_tool_set_is_read_only`（**安全断言**：子代理工具集恰为 `read_file` / `list_dir`）、`subagent_does_not_inherit_declared_tools`（阶段声明也扩不了权）、`subagent_run_row_carries_parent_and_agent_type`、`subagent_tokens_are_counted_once_on_its_own_run`、`parent_spawns_readonly_subagent_and_gets_summary_back`、`spawn_sub_agent_absent_unless_declared`；§5 `tools.rs::spawn_sub_agent_*` 四条（未启用 / 缺参 / 缺 run 上下文 / 正常摘要）；§6 `scheduler_tick.rs::subagent_runs_are_not_swept_as_node_timeouts`（子代理 run 不得被超时扫描当节点 run 处置） | 已有用例（只读子代理，2026-09-15） |
| 172⑤（票 09·技能导入） | §5 `skill_import.rs` 单测 30 条——结构校验（含 `SKILL.md` / 正文非空 / frontmatter `name` 一致）、**路径穿越**（`..` / 绝对路径 / 深层穿越 / 反斜杠伪装，外加「穿越包不在技能根外留下任何文件」的断言）、同名冲突默认拒绝 + 报出来源（技能根下那份 `SKILL.md` 的路径）+ 显式覆盖整目录替换、目录扫描（描述 / `exists` / 杂物不进清单）、批量逐项结果（一项坏不中断整批）、卸载（含工具型技能拒绝、路径穿越名拒绝）；§7 `api_contract.rs` 十一条（合法 zip 落盘带兄弟文件 / 缺 `SKILL.md` 400 且不落盘 / 穿越 400 / 同名 409 → 确认后覆盖 / 扫描清单 / 扫描描述 / 扫描目录不存在 400 / 批量逐项 / **卸载后引用 fail fast** / 卸载未知 404 / 无网全链路可用）；testkit `skill_fixture.rs` 提供技能目录与 zip fixture（票 11 / 15 复用） | 已有用例（技能导入，2026-09-15） |
| 172⑤（票 10·自定 `/index.json` registry，**整层由决策 194 退场**） | 这一行承载的用例集（`market.rs` 的索引解析 / 搜索 / origin 白名单 / 字节 sha256 校验 / `origin_of`，`tests/market.rs` 与 `api_contract.rs` 的市场段，testkit 的 `FakeMarket`）**随自定 registry 一起退场**：没有索引可解析、没有 origin 可判定、没有下载字节的 sha256 可校。**它钉过的产品行为不丢**——「未放行来源装不上」「来源方换了字节要拒绝」「市场失败不阻塞本地导入」「同名不静默覆盖」四条在新形态下各有对应（见下面 194 的两行），判定单位由 origin 换成 `owner/repo`、由字节 sha256 换成 git 对象哈希。见决策 194、票 04 的删单 | 已退场（2026-09-16，决策 194） |
| 194（票 01·GitHub 仓访问层） | §5 `repo.rs` 纯函数单测——`RepoId` 与完整 40 位 hex 的校验（带 scheme / 含 `@` / 含 `..` / 缩写 SHA / 空段 / 非 ASCII 全部拒）、`AGENTPIPELINE_MARKET_GIT_BASE` 的取值规则（不设走默认 / 回环 http 放行 / 非回环 http 拒绝 / 带路径·查询·片段拒绝）；§6 离线用例——ls-remote 取 tip（**断言没有下载 pack**）、列技能（2–3 段与 4–5 段两种深度形态）、读一个技能目录（含子树兄弟文件）、三类失败 `repo_not_found` / `commit_not_found` / `skill_not_found` 分得开、**按旧 commit 取到的是那个 commit 而不是 tip**、shallow 路径（离线 smart HTTP：断言 `.git/shallow` 存在且被取 commit 的树可读）、字节上限中断（报错含「已收到 / 上限」，且数字来自我们自己记的那一份）；testkit 的**两层 fixture**见 §3.3。**一处覆盖损失要记账**：离线 fixture 全在本机，**打不到「仓不在白名单」这类判定**，故传输与来源判定必须**纯函数化**单测（照 `egress.rs::check_allow_host` 的姿态） | 194（票 01） |
| 194（票 02·按钉住的 commit 安装） | **§7 契约搬到了 `crates/app/tests/integration/market.rs`**（25 条，注入指向离线 smart HTTP 的真 `Libgit2Repo`；`api_contract.rs` 只留「其余端点在技能来源缺席时不受影响」与一键安装的**落点**）——`head` **只握手不下 pack**（请求日志里一条 POST 都不该有）/ `depth(1)` 留下 `shallow` 且树可读 / 装成功且**装到的是被钉的那个 commit**（不是 tip）/ 同名冲突报文含 `owner/repo@<短 SHA>:<子路径>`、无记录时回落路径形态 / `overwrite` 覆盖成功 / 八类 `kind` 可分辨（`repo_not_found` 与 `commit_not_found` 同为 404 只有 `kind` 分得开）/ `detail` 带原始诊断且不混进 `error` / 未放行的仓**一次请求都不发** / 超限中断且报错含「已收到 / 上限」/ 卸载后来源记录被清 / 一键安装：按清单定位能装、**已装且记录一致**（含**没有记录**）时跳过下载（`note` 说明未重新下载）、**确知来自别的仓时不跳过**而撞同名冲突门、带 `overwrite` 时回来源仓重取。**两处覆盖损失已由票 23 补上（2026-09-23，见下面 250 那一行）**：① `repo_unreadable`——分类路径由 `RemoteBehaviour::AuthRequired`（远端 401 形态）在契约层钉住；**真 GitHub 对无凭据读私有仓回 401 还是 404 仍未实测**（见票 04，`repo.rs::unreadable` 的注继续记着这条缺口）；② `digest_mismatch`——`RemoteBehaviour::CorruptPack` 改坏 pack 尾哈希，在契约层钉住（实测 libgit2 1.9.7 报 `packfile trailer mismatch`）。**真 GitHub 那一半**由 §3.3 的 `repo_live.rs` 手动跑 | 194（票 02） |
| 179（票 12·出口控制） | §5 `egress.rs` 单测 19 条（2026-09-23 计数，含决策 246 批新增的回环伪装一条；旧计数 17 在本批之前已随实现漂移）——放行侧：本地命令形态一条不误判（`git add && git commit` / `cargo test` / `npm run build` / `make check-lint`）、**提交信息里的 URL 不算出口**（只看每段首个命令词，全串扫描被明确否决）、回环恒放行；拒绝侧：未放行主机（报错三段式：拒了什么 / 怎么放行 / 不是安全边界）、`curl -d @.env <URL>` 的 exfiltrate 形态、精确主机与端口不敏感、**子域通配落在点边界上**（`*.example.com` 不匹配 `notexample.com`）、git 网络子命令 vs 本地子命令、包管理器 install vs test、解释器 + URL、ssh 家族目标抽取（`user@host` / `host:path` / 单目标）、多段命令任一段命中即拒、`sudo` / `VAR=x` 包装不是隐身衣、**选项的取值不顶掉子命令**（`git -C /tmp/repo push` 仍是出口，`npm --prefix x install` 同理；同形本地子命令仍放行）、`allow_all` 显式开关、`check_allow_host` 解析期校验、`NetworkPolicy::from_settings` 的保守默认；§6 `tests/egress.rs` 六条（**被拒的命令真的没跑**——副作用文件不存在、且落 `kanban_node_commands` 同表并带拒绝原因 / 放行路径照常执行 / `allow_all` 是唯一的全放行入口 / **决策 246 的三条 `127.` 前缀伪装回归**——`curl` / `ssh` / `nc` 各走一条抽取路径，逐条断言默认配置被拒 + 未执行 + 落审计行） | 已有用例（出口控制，2026-09-15；三条伪装回归 2026-09-23，见 246 行） |
| 180（票 13·会话续接）→ **205 修订开关那一层** | §6 `executor.rs::a_cause_that_says_yes_carries_the_previous_attempt_messages`（**原因说 true** 时重入带上上一轮的工具往来；且 `messages` 里不混入 system）、`a_cause_that_says_no_starts_from_an_empty_conversation`（**原因说 false**（`context_overflow`）时干净起跑——同一节点重入、上一轮会话行就在库里，故这条断言有牙齿；同时是票 04「按下去 pending 真的解除」的验收）、`clean_retry_after_a_tool_failure_stays_empty_whatever_the_cause_says`（决策 33 不变）、`continued_run_links_back_so_tokens_are_not_double_counted`（必要条件二）、`context_overflow_path_writes_a_conversation_row`（必要条件一）；§5 `types.rs::resume_cause_table_is_the_spec`（判定表逐行抄期望值）/ `resume_cause_classification_covers_every_pending_kind` / `resume_cause_strings_round_trip`、`cursor_lifecycle.rs` 的两条（记的是被清掉的原因 / 人为决策也记原因）；§5 `context.rs::l3_anchor_ignores_the_loaded_history_and_takes_the_current_round` | 已有用例（会话续接，2026-09-15；**2026-09-17 由决策 205 改写成原因驱动**——阶段 / 节点开关退场，`resume_continues` 是穷尽 `match`，新增原因不写进表就编译不过） |
| 181（票 11 / 15 / 16·预览与推荐） | §7 `api_contract.rs` 十六条——预览三项返回 / **特征命中列出具体行号**（对着源文件可定位）/ 未信任 + 全文 400 且报文可操作且不落库 / 信任转换生效（阶段级 + 节点级两处都转、之后全文可存）/ 撤销信任撞全文 400 且配置一字未动 / 无引用时 `changed: 0` 如实回报 / **装前预览不落盘** / 未安装 404；推荐清单按阶段下发并标注装没装 / 一键安装落盘 + 写配置（name + 未信任）/ 正文有特征也进得来但**只能名字态** / 既有声明逐字保留且重复安装不重复追加（已在技能根里则**不重新下载**、只补启用那一步，`note` 如实说明）、**已安装技能可直接启用**（未配市场来源也能落进配置） / 技能不存在 404 且不写配置 / 未配置来源 400 可操作 / 伪阶段 400 / 停用后配置行移除；§5 `config.rs` 七条（信任转换就地改写 / 裸字符串物化 / **降信任撞全文拒绝而非静默降级** / 节点级覆盖 / 无关技能不动配置 / 转换后仍过写入门）；§5 `skill_preview.rs` 十条（三类特征分行命中 / 大小写不敏感 / 一行两类 / 宽松匹配的对照样本 / 推荐映射与手动触发型排除）；前端 vitest `stageConfigs.test.ts`（混合数组读写 / 旧格式零迁移往返 / 未信任不可切全文 / 撤销信任撞全文拒绝 / 节点级技能写回保留其余键 / 空列表删键） | 已有用例（装前预览 + 信任转换 + 推荐与一键安装，2026-09-15） |
| 176 / 182 | §6 `tests/foreman.rs`（22 条：快照字段与原因原文 / 历史字符预算 / 会话循环与收口 / **工具白名单在执行点生效** / 指标不动 / 保留期清理 / **名分与界面同词**）+ `tests/pairing.rs`（5 条：生成即持久化 / 重置换枚）；§7 `/foreman/*` 六条（会话、空 home 对话、503 未接线、空消息、流式送达且任务流零干扰）+ `/pairing/*` 七条（缺令牌 403、带令牌通过、回环豁免、只读 GET 不护、读取口仅回环、重置使旧令牌失效、缺省回环绑定不要求令牌）；§5 `peer.rs` / `stream.rs` / `server_info.rs` 的对端地址与配对比较；§9 `realtime/foreman.test.ts`（归约 6 条）+ e2e `talk.spec.ts`（8 条，含空 home 可对话、回话里没有按钮、急停滚动后仍在第一屏） | 已有用例（对讲台与配对令牌，2026-09-16） |
| 183 | §9 `lib/talkStops.test.ts`（10 条：一张不折叠 / **两张以上一张都不展开** / 一张时展开它自己 / 无急停无展开项 / 显式收起不弹回 / 选中项仍在则保持 / 选中项被处理掉后回落到默认且不悬空 / 翻转一次只换一张 / **展开判据对单张恒展开** / 详情没到不报动作数）+ §9 e2e `talk.spec.ts` 两条（**几何断言**：两张急停都完整落在状态区可见范围内、状态区不需要区内滚动——桌面 1280×720 与手机 430×900 的 38vh 两档；默认一张都不展开；点开后后端下发的「补充信息并继续」与「取消任务」两钮都在；**在已有一张展开时点另一张会换过去（同时只展开一张）**；收起回到默认形态） | 已有用例（状态区急停折叠，2026-09-15） |
| 184（输入法护栏） | §9 `lib/enterToSend.test.ts`（10 条：普通回车提交 / Shift+Enter 不提交 / 非回车不提交 / Chromium 路线 `isComposing` / `keyCode 229` 也挡 / **WebKit 同一任务**（compositionend → keydown）不提交 / **跨任务窗口内**也不提交 / **窗口过后照常提交** / 先选字再发送不被吞 / 未组合过不设窗口 / 再次组合重置窗口）+ §9 e2e `talk.spec.ts` 一条（按 WebKit 次序在**同一任务**里合成 `compositionend` + `keydown`，断言时间线没多出「我」那一轮且输入框内容仍在；紧接的独立回车照常发送）。判据三重（`isComposing` / `keyCode 229` / **50ms 时间窗**），第三重是桌面壳（WKWebView）上必需的那一重 | 184 |
| 185（PATH 工具型技能退场） | §5 `skills.rs` 两条直接钉（见上）；`config.rs::validate_startup` 的不存在技能报错文案点明「二进制不算技能」；`skill_import.rs` 的同名冲突只认技能根（两条只对工具型成立的用例已删） | 185 |
| 186（绑定开关） | §5 `crates/core/tests/integration/server_bind.rs`（5 条：未设置过 → `None` / 换句柄仍读到 / 二次写是替换不是追加 / 清除报出是否真清 / 没设置过也能清）+ §6 `crates/app/tests/integration/lan_bind.rs`（2 条**真二进制**：开启 → 重读确认 `0.0.0.0` 且 `bind_source=settings` → **重启后仍记得** → 清除 → 回到 `config` → 再重启不反弹；启动期覆盖压过界面设置）+ §7 契约三条（局域网来源 403 / 无监听器 503 / `bind_source` 上报）+ §6 `serve.rs` 纯函数四条（优先级三级 + 契约串）+ §9 `lib/lanToggle.test.ts`（10 条：正常 / **传输失败但状态已变算成功并说明** / 空窗重试 / 服务端明确拒绝回显原因 / 传输失败且没变**不编造原因** / 读不回来如实说 / 关掉走 `clear` / 启动参数钉住时如实报「关不掉」/ 开启被启动参数钉在回环时指明去掉哪个参数）+ §9 e2e `lan-bind.spec.ts`（真按那颗钮：缺省是钮 → 按下进二维码区 → 关回来回到指引区） | 186 |
| 187 / 194（技能市场设置页与来源仓名单） | §7 契约（读到底 / 归一 + 去重 + **保存即生效** / 非法 `owner/repo` 400 且点名 / 空数组是**显式**关闭而非回落 / `DELETE` 回到配置文件）+ §9 `market.spec.ts`（⑫，本批重写：仓名单可加可存可退回、非法仓名当场被拦、冷启动名单零网络，以及**显式清空 ≠ 未保存过**——清空后不回落配置文件、点交还才回落）。**旧断言（「白名单是空的 / 不允许远程安装」）按新语义重写**：不变的是**两级关系**（界面那份保存即生效、清掉回 `config.toml`），变的是判定单位从 origin 换成 `owner/repo`（决策 194 裁决④；归一与校验与后端同口径，界面上不另写正则）。原 `lib/marketSources.ts` / `marketSources.test.ts` 随之退场，由其 `/market/repos` 同口径的替代物（`lib/marketRepos.ts` + 单测）改写 | 194 修订 187 |
| 177 / 181 / 187 / 194（市场**安装**通路，本批换血） | §9 e2e `market-install.spec.ts`（真 bundle + 真后端；fixture 从「手搓 ZIP + 普通 HTTP」换成**离线 smart HTTP 的 git 仓**，并把 `AGENTPIPELINE_MARKET_GIT_BASE` 指回本机回环——回环明文 http 由决策 177③ 放行，**顺带把 shallow 与传输策略也打了**，那是旧 fixture 打不到的）：① 「添加一个仓 → 按父路径分组列出技能 → 装一个并看到三项预览」——**没点添加之前零网络请求**（fixture 的 `requests()` 可断言，对应票 03 的冷启动约束：内置推荐名单 ≠ 放行）→ 加仓后列表出技能、顶部显示「基于 `<短 SHA>`（时间）」→ 右列三项预览 → 装 → **读后端 `GET /skills` 确认包真的落到技能根**（不信界面自述）；② 「同名重装不静默覆盖」——再装同名给 409 与一次显式「覆盖安装」的机会；③ 「**列表钉住浏览那一刻的 commit**」——远端前进后不刷新仍是那一份、刷新才换，装载用的始终是列表上那一份。**⑫⑬ 两张都保留、不合并**：⑫ 钉「界面与配置的两级关系」，⑬ 钉「装得下来、落得对」，是两个**独立失效面** | 194 修订 177 / 181 / 187 |
| 189（取不到令牌就不画码） | §9 `lib/sharePairing.test.ts`（8 条：有令牌 → 带令牌的码且形状与 `pairing_url` 同约定 / 选中项优先 / 保留字符被编码 / **没令牌 → 指引块不画码** / **空串也当没取到** / 只绑回环优先 / 枚举不出地址优先于选中项 / 服务读数未到不当作回环）+ §9 `routes/Share.test.ts`（5 条**组件层**，换掉客户端出口让 `GET /pairing/token` 各回一次：**403 → 页面上没有二维码**且给出 `127.0.0.1:{port}/#/share` 与「没有令牌的二维码扫了也配不上」 / 有令牌 → 无指引块且 `src` 带 `pair=` / 500 → 无码且照实报因 / 令牌悬着 → 先给「正在读取配对令牌」而不是没令牌的码 / 只绑回环仍是改绑钮）+ §9 e2e `lan-bind.spec.ts`（决策 186 那条补两句：二维码区的地址**带 `?pair=`**、配对说明是「二维码已带上配对令牌」那一条——本机这一页给出的码必须是带令牌的那张）。**非回环那一半不由 playwright 钉**：它造不出一个非回环来源，故 403 那几条落在组件层 | 189 |
| 190（顶栏入口的来源判据） | §9 `lib/localPage.test.ts`（8 条：回环的几种写法都算，含 `[::1]` 与 `127.255.0.9` / 大小写与空白 / **脱尾点与八位组范围**（`localhost.` 放、`127.999.999.999` 拒——决策 246 对齐的两处）/ 局域网与域名不算 / **前缀伪装不算**（`127.evil.com`、`localhost.evil.com`）/ 同源形态看当前地址 / 注入 base 时以 base 为准（桌面壳）/ 判不出主机名时**不藏**）+ §9 `components/layout/TopBar.test.ts`（2 条组件层：本机打开 → 六个入口齐全含「手机访问」；非本机 → 没有「手机访问」且**其余五个一个不少**）+ §9 e2e `lan-bind.spec.ts` 补一句（本机打开的页面上顶栏**有**这个入口）。**非本机那一半不由 playwright 钉**：它造不出非回环来源。附：`lib/lanToggle.test.ts` 10 条继续钉回环口径（它已改为复用 `localPage.ts` 的那一份） | 190 |
| 191（令牌留在地址栏） | §9 `api/pairing.test.ts` 改写三条并新增一条（**地址栏那份不再被抹掉**且令牌仍进本地 / 地址栏还有别的参数不影响读取 / **每次装载都从 URL 重存一次**（主屏图标与书签靠这条）/ 没有配对参数时不动已存的那份）+ §9 e2e `lan-bind.spec.ts` 新增一条（真应用里打开 `/?pair=e2e-token`：**地址栏里仍是 `pair=e2e-token`**、localStorage 也存下了；**再装载一次仍在**——等价于从主屏图标再进来）+ §5 `assets.rs::static_responses_forbid_referer`（所有静态响应带 `Referrer-Policy: no-referrer`，它是当初「抹地址栏」三条理由里 `Referer` 那条的替代品） | 191 |
| 192（对讲台窄屏版面） | §9 `lib/talkStops.test.ts` 由 10 条增至 **15 条**（新增五条：**窄屏一张也折**、窄屏恒不展开、窄屏下摊开判据只认人点过的那个 id、窄屏显式收起仍不弹回、**同一份集合宽窄两解**）+ §9 e2e `talk.spec.ts` 由 11 条增至 **13 条**，新增一组**自带装置**的窄屏用例（不复用上面那组：那组最后一条会把唯一的急停按掉，之后量到的是空状态区）：① 静置版面——单张急停已折成摘要条、状态区不再区内滚、**对话区 ≥320px**（改版前同一装置实测 26px）、输入坞底边与底栏（`.statusline`）顶边严丝合缝、无对话时**整页不空滚**；② 长对话滚到底——整页滚（时间线不再是滚动容器）、**摘要条仍钉在顶栏下沿**（`top:138px`）、输入坞仍贴在底栏上沿；③ 断言口径是**几何**而不是「CSS 里有没有 sticky」（后者在版面塌掉时照样为真）。**牙齿检查**：把 `forceFold` 退回常假 → ①在「单张已折」这条就变红，且此时同一装置上对话区实测只剩 48px（②的前提同时失效）| 192 |
| 193（名分：值班员 → 值班经理） | §6 `tests/foreman.rs` 由 21 条增至 **22 条**，新增 `the_persona_calls_the_human_what_the_ui_does`（人格里出现「值班经理」、旧词「值班员」**不回潮**——同一个名分只有一个答案，而这个名分漂过一次：决策 174 / 176）；§9 e2e `talk.spec.ts` ⑦「给值班长发话」补两条**名牌**断言（人这一侧 `turn.mine .dname` 与输入坞 `.typer .dname` 都读作「值班经理」，对面那块原本已断言「值班长」）——**用例数不变**，补在既有用例里 | 193 |
| 213（桌面壳端口跨重启稳定） | §5 `serve.rs` 六条（绑定策略：首选空闲用它 / 占用退让并标 `fallback` / 未开退让明确报错 / **只认 `AddrInUse`** / 退让缺省关 / 三个串即契约）+ §6 `crates/app/tests/integration/port_stability.rs` 2 条（**不传 `--port` 起两次真二进制，端口不变且 `port_source=config`**；占住配置端口时用桌面壳那套参数起服务 → **退让且 `port_source=fallback`**）+ §7 `api_contract.rs` 两条（缺省 `port_source=config`；`with_port_source(Fallback)` 上报 `fallback`）+ §9 `lib/sharePairing.test.ts` 5 条（退让时的话含当前端口与「重新扫」/ `config`、`startup` 不说 / **只绑回环不说** / 读数未到不说）+ §9 `routes/Share.test.ts` 2 条（退让 → 页面上出现「临时端口 53311」与「重新扫一次」；常态 → 不出现）。**旧行为被替换的那一条**：桌面壳此前的 `port_override = Some(0)`（决策 153⑤ / 156）不再有消费者，命令行与测试的 fail fast 姿态**原样保留**（`smoke.rs` 的「端口占用应启动失败」继续绿） | 213 |
| 214（推荐面板三态与来源定位；票 16 第 14 行那条验收「已安装的可直接启用」的界面落点） | §7 `api_contract.rs::recommendations_report_whether_each_skill_is_declared_in_that_stage`（**同一个技能在两行上的答案可以不同**——按阶段算而不是按技能算；阶段级与节点级声明**都算**本阶段声明；只在别的阶段被引用时 `declared_here=false`；来源 `repo` / `dir` 是 `owner/repo` 与仓内路径的真值，且**不下发 `commit`**——清单是指针不是名录）+ §5 `config.rs::skill_declared_in_stage_is_per_stage_and_counts_node_level`（按阶段而非按技能名 / 节点级算数 / 一条来源非法只废它自己 / **没有配置行的阶段**恒假）+ §9 `frontend/src/components/settings/StageRecommendations.test.ts` 8 条（三态各画什么、两颗钮的文案与点击回参、忙时两颗都禁用、来源行渲染 `owner/repo · 目录`、两段都取不到则整行不画而行仍在）。**旧行为被替换的那一条**：推荐行此前只在 `!installed` 时画钮，故本机十行推荐（全部 `installed: true`）**一枚钮都没有**——三态化之后「已装而本阶段未声明」的那一行才有控件 | 214（票 01 / 02） |
| 215（中间档 480–1240px 的版面） | §9 e2e `ux2-geometry.spec.ts` ②（状态行在 1440 / 1024 / 900 / 820 / 768 / 700 / 600 / 560 / 520 / 500 / 480 逐档：`scrollWidth <= clientWidth`、**每档时钟仍可见**、主题钮在视口内）；§9 `lib/pendingDossier` 侧的档案盒让位见 `ux2-geometry.spec.ts` ③ | **状态行那一格已有用例**（票 08）；**详情页 / 对讲台中间档折行与 `hero` 轨道容器内横滚待票 18 / 19（open）**——那两格的 e2e（宽度扫描 + `document` 不横滚）在票里写明，落地后填这里 |
| 218 / 220 / 221（对讲台的空间预算重排 + 回话中换班次与两枚标记 + 页头那一行的真实高度） | §9 e2e `talk.spec.ts` 增至 **29 条**：新增**折行档（430×900 / 375×667）**一组（静置版面几何 / 整页滚到底三条钉住物还在 / ⋯ 班次菜单三条出口与 ≥44px 命中区与 `aria-current` / 输入坞 88px 无提示语行 / 掐断 SSE 是唯一断线告知且空闲不占高 / 展开一张急停的名牌不被钉住带子压住）、**折行档宽度扫描**一组（480 / 600 / 768 / 899 单列不横向滚、899 与 900 两侧的单张急停「折」与「不折」、900 / 1099 / 1100 右栏 280 → 340）、**班次**三条新增（页头里没有 chip 行且菜单条数与接口一致 / 回话中换班次：切走 → 回话落地 → 原班次带「有新动静」且点回去那一轮完整 / **跨设备那一组**：SSE 里带别的 `session_id` 的增量让那一班带「正在回话」，标记不拦点击）、**工位回执分档**（折行档默认收起；`page.route` 把 POST 拖住 2.5s 验「手动展开后不被流式增量打回」）、桌面那条「长对话滚到底」补上落点断言（`.talk-head .runrow` 与 `+ 新班次` 在视口内、页头高度 ≤ `<h1>` 行盒 + 1px）；**顶栏信号灯**一条（在 `#/talk` 上点灯 → 去 `#/` 且 `#s-init` 进视野，并用预先种下的一格 `window` 标记证明**没有整页重载**——它当初红过：`router.navigate` 只写地址栏、镜像等 `hashchange`，紧随其后那次 `scrollIntoView` 找不到靶子）、§9 单测新增 **`lib/talkLayout.test.ts`**（node 环境静态扫 `Talk.svelte`：断点字面量只有 479 / 899 / 1099 三个，与 `lib/talkLayout.ts` 的常量逐个相等）与 **`lib/talkSessions.test.ts`**（19 条：标记的三态判定与「回话中优先」、当前班次永不算「有新动静」、**立基线只在本机一条记录都没有时**、看过的时刻取服务端自己的 `last_active_at`、`agentpipeline.talk_seen` / `talk_session` 的读写与**坏值当空 / 非法值删键**），`realtime/foreman.test.ts` 扩 `noteForeignDelta` / `forgetForeignActive` / `foreignIsReplying` / `pruneForeignActive`（同一事件里「当前班次丢弃」与「别的班次记录」两条路不互相污染；**静默超时**与**落地即熄灭**两条收口各钉一遍，且落地判据只认那一班；无该清的项时返回**同一个对象**，组件那个 5s `$effect` 因此不自激） | 两支关键用例都**真的红过再绿**：a) 决策 218 ⑦c 的「展开一张急停时名牌整块落在状态区里」——实测 `scrollTop=36, zone.y=158, dname top=164, band bottom=184`，`scroll-margin-top` 原先只写在 `.zone-status.stops:not(.stop-open)` 上，挪到折行档基类才修好；b) 决策 218 ④ 的「那一行 19.2px」——实测 **29.44px**，根因是本页 `.crumb` 误继承台账页基面的下边距，见决策 **221** |
| 216（不可逆动作的确认步与三档量级） | 待票 21：单测落 `lib/actions.ts` 的认档判据（四档 + `continue` 两支分叉），e2e 断言「第一次点不发请求、出确认句，第二次才提交」与「跳过闸门那颗不再是实心」 | **待实现**（票 21，open）。现状锚点：`components/board/PendingActions.test.ts` 与 e2e `ux2-resilience.spec.ts` ⑤ 钉的是**动作身份**（票 20 的修复），不是确认步 |
| 223（对讲台一轮的寿命与痕迹） | §6 `tests/foreman.rs` 新增 2 条：`a_hung_model_is_bounded_and_recorded`（挂住的模型流以「没有结束」失败**并落一条 system 账**；牙齿：摘掉 `respond` 的 `tokio::time::timeout` 即挂死）、`an_interrupted_turn_is_recorded_in_the_latest_session`（panic 那一支的留痕落到最近活动的班次）；§7 `api_contract.rs` 新增 1 条 `a_dropped_request_does_not_kill_the_turn`（回话途中掐掉这一次请求 ≈ hyper 丢掉 handler，回话仍落库、且**不**多出一条失败留痕；牙齿：把 `send` 里的 `tokio::spawn` 拿掉即变红——实测精确复现当晚「回话永不落库」）；§9 `realtime/foreman.test.ts` 新增 1 条（`failureNotice` / `isTimeoutMessage`：本地超时那一类补「它在服务端仍在继续」，其余失败原样）；§5 `git.rs` 内联 `worktree_lock_wait_is_bounded_and_says_so`（等锁有上限且报文说清是锁被占着；牙齿：把 `try_lock` 换回 `lock()` 即挂死） | **起因是实测报障**：2026-09-18 班次 `01M2QZCNN4CC65SSBQS1FJG402` 两条 `user` 行、零回话、零留痕（决策 211⑤ 修过一次的同一形状）；另附桌面进程 `sample` 拿到的三条 `open()` 挂死栈（对讲台那条路一行 git 调用都没有） |
| 217（中流状态的地址与本地留存） | **对讲台班次那一格已落地**（决策 218 / 220 的实现票顺带）：§9 `router.test.ts` 的 `readQuery` / `writeQuery` 用例（缺省不写 / 合并而非覆盖 / `replace` 不新增历史条目 / 实参非法时回落）、§9 `lib/talkSessions.test.ts` 的 `agentpipeline.talk_session` 兜底与非法值删键、§9 e2e `talk.spec.ts` 的班次用例（换班次写地址、装载时按地址落点）；**其余三格（页签 / 过滤 / 草稿）待票 22**：单测覆盖 query 读写与草稿的恢复·清零·7 天过期，e2e 三类断言（刷新恢复 / 后退恢复 / URL 可分享） | 对讲台那一格**已实现**（`.scratch/talk-mobile-space/issues/10`）；页签 / 过滤 / 草稿仍 **open**（票 22） |
| 224（值班长一轮的轮数上限 8 → 30） | §6 `tests/foreman.rs` 新增 1 条 `a_foreman_that_never_wraps_up_is_capped_and_named`：**从不收口的那一支怎么报、怎么留痕**——脚本按常量声明满轮工具调用（每轮一个 `read_task`，模型一直在查台账），断言三样：`llm_classified` 的类别是 `model_no_reply`、模型**整整数轮**被调用（`calls_for(Stage::Init, Node::Execute) == FOREMAN_MAX_ROUNDS`，钉的是「模型被叫几次」而不是「工具被调几次」）、库里落一条带 `model_no_reply` 的 `system` 账。**用例不写死 30**：改上限时它跟着走，它钉的是耗尽这一支的行为，不是那个数。**牙齿**：把 `for _ in 0..FOREMAN_MAX_ROUNDS` 换成「脚本耗尽即收口」就会红在类别那一条（脚本耗尽返回的是收尾文本，会走成功路径） | **起因是实测报障**：2026-09-18 班次 `01M2QZCNN4CC65SSBQS1FJG402` 的 14:54 与 15:47 两轮以 `model_no_reply` 失败，而**同一个班次** 14:59 那条成功的值守播报在痕迹里有 16 条工具调用——上限比一次正常轮还短。另见决策 224 里记的现场缺口（失败轮不留 `traces_json` 与 token，故本用例只能断言「落了一条账」，断言不了「它那几轮查了什么」） |
| 226（判超时要真的把 run 停下来 / 记账不再用 0 冒充读数 / `read_file` 有界读） | **新增 5 条**：§6 `executor.rs` 两条——`a_timed_out_run_is_stopped_and_reports_its_usage`（**这一批的承重用例**：造一个停在模型调用上、`process_group_id` 为 NULL 的执行体，判超时那一方标终态 + 记时长 + `request_cancel` 之后断言五样——执行体在 5 秒内收口、run 行的 `status` 仍是 `Timeout`、`duration_ms` 仍是判超时那个数、`prompt_tokens`/`completion_tokens` 补记成 7/3、游标**不** pending，最后 `try_run` 拿得到执行权。**牙齿**：把 `select!` 那一支换成直连 `await` 即挂死在这条上）、`a_failed_round_records_the_tokens_it_burned`（一次成功的工具调用之后让模型调用当场失败，断言 run 行记 10/5 而不是 0；**牙齿**：把那处 `&tokens` 换回 `&RunTokens::default()` 即红）；§6 `scheduler_tick.rs` 一条 `a_timed_out_run_records_how_long_it_ran`（起跑回拨 311 秒 → `duration_ms == 311000`；**牙齿**：把 `duration_ms` 那行拿掉即红）；§5 `tools.rs` 一条 `a_big_file_is_read_without_being_loaded_whole`（造一个越 4 MiB 的文件：头部读法给开头且**如实标注省略**、`tail` 给到最后一行且不给开头；**牙齿**：把有界读换回 `read_to_string` 后 `tail` 那条断言先红）；§5 `file_policy.rs` 一条 `the_foreman_root_denies_the_key_store_but_not_logs`（`data/` 目录与其下文件读写都拒、`logs/` 都放、别的 `data/` 同名目录不受影响）。**按新语义改写 3 条**（不是放宽断言）：`executor::unsticking_releases_the_in_process_dedup_and_allows_a_rerun`（中止之后「重跑」不再由被判死的旧执行体顺手跑出来——改为「unstick 后先取得执行权、且游标 pending 时不得执行节点、恢复之后才真的跑」，这一条比原来更严）、`env_mode::the_foreman_domain_covers_the_home_but_not_the_key_store` 与 `foreman::the_foreman_reads_the_home_but_not_the_key_store`（端到端：日志读得到且内容真的进对话、密钥仍然一个字节都出不来）。**另有 1 条既有用例由红转绿、断言一字未动**：`executor::continued_run_links_back_so_tokens_are_not_double_counted`——它顶出了「失败轮记真之后任务投影与 run 行汇总不一致」，修的是实现（两条路补 `refresh_task_totals`） | **起因是实测事故复盘**：2026-09-19 任务 `01M2QH0DHKGSGNVHC0WT2Q4CG0` 的 run 23 被判超时后，执行体又活了 8 小时以上（`~/.agentpipeline/logs/agentpipeline.log` 里那行 `resume 触发被在跑的 executor 持续挡下，放弃本次触发`），而它与前一条 run 的 `prompt_tokens` 都是 0、`duration_ms` 也是 0——值班长据此连报四轮（1,091,249 prompt token）却把根因推错 |
| 246（回环判定收窄 + 收敛，票 host-policy 01–03） | §5 `host_policy.rs` 两条——**共享表测试**（读 `tests/fixtures/host_policy_loopback.json` 逐行断言，覆盖决策 246 全输入表，含必需行按名钉住 + 放行 / 拒绝两侧都非空的形状守卫）与归一幂等；§5 `egress.rs::loopback_prefix_disguises_are_denied_on_every_extraction_path`（三条抽取路径各一条：`url_host` / `ssh_style_host` 的 `user@host` / 单目标二进制）；§6 `tests/egress.rs` 三条同名回归（被拒 + 命令未执行 + 落 `kanban_node_commands` 带拒绝原因）；§5 `repo.rs` 的回环表测试随谓词迁走（其七条断言由共享表承接）；§5 `peer.rs::is_loopback_bind_covers_forms` 补 `LOCALHOST` / `localhost.`（大小写与尾点四处一个答案）；§9 `lib/hostPolicyFixture.test.ts` 2 条（同一张表、同一断言方向——Rust 改了规范前端没跟就变红）+ `lib/localPage.test.ts` 的脱尾点 / 八位组范围条 | 已有用例（2026-09-23） |
| 250（票 23·决策 250 Q2 的两件同源修缮：RepoId 子集不变量 + 两个未钉住的 kind） | §5 `repo.rs` 表测试 `shared_repo_id_fixture_pins_frontend_output_inside_backend_accepts`（读 `tests/fixtures/repo_id.json` 的 34 行逐行断言：**后端 `RepoId::parse` 认识前端归一输出 ⟺ `valid`**——「前端输出 ⊆ 后端接受集」从文件头注释升成会变红的测试，含必需行按名钉住 + 合法/非法/归一三侧非空的形状守卫）+ `is_digest_shaped` 措辞清单补**实测原话** `packfile trailer mismatch`（class=Indexer 而非 Sha1；不补则真坏包被误判成 `commit_not_found`、把「别装报警」说成「换一个 commit」）+ 对应 core 措辞单测行；§6 `market.rs` 两条——`auth_required_remote_is_reported_as_repo_unreadable`（`RemoteBehaviour::AuthRequired` 演私有仓 401 → 404，kind 与 `repo_not_found` 分得开）与 `corrupted_pack_is_reported_as_digest_mismatch`（`RemoteBehaviour::CorruptPack` 改坏 pack 尾哈希 → 400，报文劝住「别装」且 `detail` 单列）——**八类 kind 的 API 契约层覆盖由 6/8 补到 8/8**（逐类钉在哪层见票 23 票面）；testkit `repo_fixture::RemoteBehaviour`（Normal / AuthRequired / CorruptPack 三种远端形态，`SmartHttp::serve_behaviour`）；§9 `lib/marketReposFixture.test.ts` 2 条（同一张表、同一断言方向——前端改了规范 Rust 没跟、或反过来，落后的那侧变红） | 已有用例（2026-09-23） |
| 247（值班长工具清单三处同源 + 回执标签后端供给，票 foreman-tool-manifest 01–03） | §6 `tests/foreman.rs`：冻结断言**由两条层清单并成一条 21 名顺序冻结**（`ForemanToolLayer` 删了，分组不再手标）+ 新增 3 条——`the_discipline_groups_are_derived_from_the_tier_predicates`（纪律段两组的并 == 冻结清单、交为空、分组与 `is_env_write_tool` ∨ `is_service_write_tool` 一致，取证取**模型真正收到的那份 system prompt**）、`the_watch_round_prompt_does_not_advertise_what_it_stripped`（值守轮纪律段不含 `read_conversation` / `run_command`——修的是 `system_prompt` 早于 `deny` 计算的顺序 bug，从前值守轮广告着这一轮已被摘掉的工具）、`the_deny_tier_prompt_does_not_advertise_the_environment_layer`（`deny` 档纪律段不含任何 `ENV_TOOLS` 名；取证先把 `FOREMAN_PERSONA` 摘掉——人格点名 `repair` 是静态文案、本票明文不动）；`every_listed_tool_has_a_parseable_parameter_schema` 补**每条 label 非空**（编译器管有没有，测试管是不是空串）；§7 `api_contract.rs` 新增 `the_tool_label_endpoint_lists_the_whole_manifest`（21 条 / 与清单同序 / label 非空 / **只出两个字段**）+ `foreman_endpoints_report_503_when_unwired` 补 `/foreman/tools`（静态清单也在列——`/foreman/*` 下没有「接线外可用」的特例）；§9 新增 `lib/toolLabels.test.ts` 3 条（fetch-once 只发一跳 / 失败不缓存、下一跳重试 / 认不出原样兜底）+ `lib/proposals.test.ts` 改写（基词查 `labels`、skills 徽章 `技能 · install` → `技能动作 · install`、`repair` 固定句传冲突标签也不变、`service` 无后缀、`pairing` 那条**不改仍绿**）+ e2e `talk.spec.ts` 回执由三条改四条（新增「读诊断包」上回执，用**另一个**任务 id——两个读数共用一个 id 会互相认错） | 起因是一次架构评审（候选 7：三个事实源已经不一致），两轮拷问定稿；行为规则同步进 design/frontend-design.md §12.3（决策 199） |
| 249（executor 五片拆分，票 executor-split 01–05） | §5 新增 23 条窄测试、**既有测试一条不删**：`model_request.rs` 13 条——组装侧 golden 序过 interface 直测（基线→工作目录→AGENTS→persona→技能→格式规则的相对位置）、全文态技能正文变 hash 变 / 名字态正文变 hash 不变、重入段首轮为空·打回才渲染·architect 之外不渲染、节点级技能只进声明节点、persona_path + append 与不可读报 Config、deny 档广告摘除、组装期 Overflow 臂**带 plan 回来**；预算侧软限下 Ok、软硬之间就地压缩 Ok{compacted}、压缩后仍超硬 Overflow（软/硬限钉在「静态 + margin」、静态读数取 `reserved_*`）、`capacity=None` 恒 Ok、carried_len 锚不吞载入历史；`run_ledger.rs` 5 条——**假时钟确定性驱动 duration**（Clock 接缝落地的见证）、finish 不抢已有终态只补用量、record_usage 不改状态、续接只链 round 0、take 读清恰一次；`model_invoke.rs` 3 条——post_process 两臂**不建 Executor** 直测、伪阶段 run 行形状不跑全循环（桩 LlmClient）+ `module_overlap_detection` 随迁；`merge.rs` 2 条——基准失配→审批失效**不跑全节点循环**直测、`parse_diff_stats` 随迁单测。集成 / e2e / 契约侧**零删除**（executor 集成与契约的 diff 只见 import 改道），6 条在片内随迁的名字原样（model_request 5 + `module_overlap_detection` 随 03 到 model_invoke） | 起因是一次架构评审（候选 2「拆开 executor god module」），四轮拷问定稿（决策 249）；实现 2026-09-23 |
| 252 / 253 / 254（前端手抄镜像的三条裁决，票 mirror-contract 01–03） | **票 01（决策 252，删镜像）**：§7 `api_contract.rs::the_session_wire_says_what_each_row_is`——**三段独立班次**（基表 / 值守轮失败 / 助理轮带前缀）里摆齐七个场景（人的话 / 操作台 / 没跑起来的一轮 / 值班长的话 / 值守播报 / 值守轮失败 / 助理轮带失败前缀），基表断言 `kind` 序列为 `["mine","console","failed","fm","fm"]`、`proactive` 只在播报轮为 `true`，并断言 `content` 里**仍带**那两个哨兵前缀（前缀不删，只是不再由前端解释）+ 值守轮失败那一格（`fm`+`proactive` 之后紧跟 `failed`+`false`）；另钉一条防伪造：「助理轮不因正文前缀被判成失败轮」（失败账一律由后端以 `system` 写，而助理轮正文来自模型）。§9 `realtime/foreman.test.ts` 删掉那条**自比自**的用例（`expect('【归因】').toBe('【归因】')`——前端字面量与前端字面量比，Rust 改了照样绿，而标题写着「与后端同源」），`LedgerRow` 从 `{id, role, content}` 收成 `{id, kind}`（**刻意不取 `content`**：判据只该看字段），失败轮用例改成比 `kind`，并补「`mine` / `console` 都不作数」。**票 02（成员表）**：§5 `types.rs::shared_enum_members_fixture_matches_the_enums` 读 `tests/fixtures/enum_members.json`，断言两份成员表与**枚举导出的变体表**逐项相等（顺序也 pin）；导出走 `schemars::schema_for!`——**宏从枚举定义取变体表，不经过任何手写清单**（比票面要求的「遍历 `ALL_STAGES` / `as_str`」更强：手写数组本身也可能漏一个）；失败报文带可直接贴回的 JSON。§9 `lib/enumMembersFixture.test.ts` 断言 `STAGE_MEMBERS` / `PENDING_KIND_MEMBERS` 与同一份 fixture **逐项相等**。**票 03（规格表）**：§5 `types.rs::shared_spec_tables_match_the_backend_spec` 读 `tests/fixtures/frontend_spec_tables.json`——`stage_keys` 的真实阶段那一半 == `Stage` 枚举成员集（经 `schemars` 导出、不手写数组）、且按 `ALL_STAGES` 的**全序**排列、后 4 项 == `pseudo_keys`；`pseudo_keys` 与真阶段互斥且含 `foreman`；`terminal_statuses` 遍历 `schema_for!` 导出的**全部** `TaskStatus` 变体问 `is_terminal()`（集合相等，不手写数组）；`user_decision_context_kinds` **双向**断（表里每个都被 `ResumeCause::classify` 认出来不落通用行；反方向拿 `actions::kinds` 的**全部**常量逐个问 `classify`，配一条源码扫描守卫 `every_actions_kind_is_covered_here` 保证那份常量清单不会漏）+ app 侧 §5 `stage_configs.rs::pseudo_stage_keys_match_the_shared_spec_table`（`PSEUDO_STAGE_KEYS` 与同一份表逐项一致）。§9 `lib/specTablesFixture.test.ts` 6 条——`STAGE_KEYS` **逐项含顺序**等于表、伪键**集合相等**（经 `isPseudoStage` 加一条读 `stageConfigs.ts` 源文本的静态扫描——`isPseudoStage` 只答「这一个是不是」、枚举不出集合，故挡不住「集合里多出一个前端不展示的键」）、终态集经 `stewardshipFace` 逐状态断言、`pendingLabel` 覆盖成员表里每个种类（未覆盖即红）+ 覆盖每个 `user_decision` 子类。两侧的读表样板收进 `lib/fixtures.ts`（票 03 的 `Blocked by: 02` 要的那个 helper） | 起因是一次架构评审（候选 8「前端类型面是手抄的镜像」），三轮拷问定稿。**实现 2026-09-23，两处按事实订正票面**（均记在票面「实现者记事」）：① 票 01 的测试要点（「assistant 且带失败前缀 → `failed`」）与它的落地形状栏（`assistant → fm` 无条件）自相矛盾，**按落地形状实现**——另一条是实现不出来的行为变化，且会让模型的措辞能把自己那一行染成失败轮（伪造面）；② 票 02 原定「Rust 侧加一条遍历 `ALL_STAGES` / `as_str` 的测试」，落地改用 `schemars` 宏导出，理由是遍历手写数组仍可能漏（**判据要遍历枚举本身**）——这条同样回灌到票 03 的 `stage_keys` 与 `terminal_statuses` 两处。**两轴 code-review 的收口（2026-09-23）**：修掉实现里三处**注释声称与代码不符**的地方（`TaskStatus` 与 `Stage` 两处号称「遍历变体」实则手写数组、契约测试号称「一个班次一次读取」实为三段）、一处**重复的 `kind` 字面量**（`LedgerRow` 改 `Pick<ForemanMessage,'id'|'kind'>`）、一处**悬空引用的守卫测试名**（改为真实存在的源码扫描守卫）、以及 glossary 词条里两处**过期行号**。此外**不为测试导出内部常量**：前端的 `PSEUDO_KEYS` / `TERMINAL_STATUSES` 仍是模块私有，经 `isPseudoStage` / `stewardshipFace` 这两个**唯一消费者**断言。**票 254 无测试**：`types.ts` 的「形」的镜像是 DTO 字段，本批**押后**（后端无响应 DTO、TS 结构上钉不住真实 JSON、成本是潜在的），重开条件写在决策 254 ③ |
| 255（直接动作面搬进 core，票 foreman-actions 01–04） | §5 `foreman_actions.rs` 2 条——`owner_stuck_window_reads_the_setting_as_minutes`（宽限换算的唯一性：设置项按**分钟**读，钉住搬迁时发现的 60 倍差——提议那条原按 `Duration::seconds` 读分钟设置，调度器与托管按 `minutes`，同一判据两个答案）+ `missing_arg_is_a_validation_error`；§5 `routes/foreman.rs` 2 条**静态守卫**（读自身源文本、只扫 `#[cfg(test)]` 之前——守卫自带禁止字面量，全文扫会扫到自己）：`every_proposal_capable_tool_has_a_dispatcher_arm`（清单 × 写工具集的交集 **8 名**冻结，每名在分派器里有一条臂）+ `the_moved_families_are_not_implemented_here_anymore`（git 链 / `unstick` / 恢复序列不在路由层，四个 `foreman_actions::run_*` 真被调到）；§7 `api_contract.rs` **3 条新增**（搬迁前这两族在 app 与 core 两侧零测试）——`the_service_family_runs_the_three_step_recovery_sequence`（owner 清 / running 归队 / 项目级 run 标终态三步各一断 + 动作名不对 400 且提议不消耗 + 报文含「本进程没有自重启能力」）、`a_repair_proposal_merges_the_branch_and_recycles_the_worktree`（真 git：合入落默认分支 / worktree 回收 / 分支删除三样齐全）、`a_repair_proposal_whose_base_moved_conflicts_and_is_refused`（基准前进冲突 → 409 列出 `src/lib.rs`、main 侧未被改写、worktree 与分支保留、提议仍 pending）。`RepairSession::from_outcome` 收编三处重建 | 起因是一次架构评审（候选 6「提议执行从路由层挪到 core」），三轮拷问 Q1–Q8 定稿（决策 255）；实现 2026-09-23。**订正入档**：决策 255④ 的恢复序列 3+1（`orphan_inflight_model_requests` 是启动特有，运行中由 `recording::Settle::drop` 兜底）、⑤ 的「假传输边界」不成立 |
| 251（值班板工位灯 + 对讲台的判断抽出，票 talk-judgments 01 / 02 / 03） | **票 01**：§9 `lib/pipeline.station.test.ts` 7 条——`aggregateStationState` 的**优先序**（`running` 压过 `failed`，修的就是这一档）、四盏灯各一条、`cancelled` 也点红灯、全 `done` 才算 `done`、空列与 `queued` / `waiting` 落 `idle`；§9 `lib/delegation-scan.test.ts` **静态守卫**（决策 251⑥：`@vitest-environment node` + 读源文本 + 正则）——列头与值班板都调 `aggregateStationState`、两处都不再就地推那串优先序、`.blamp.x` 接上且取色只走 `--stop`（**无裸 hex**，决策 169 的 token 纪律）、**`.brow.hot` 必须有生产者**（词表从自造的 `run` 换回 `go` 时差点整档漏掉，那时看板与值班板对「在跑」有两种画法）；§8 e2e `talk.spec.ts::全失败的工位点红灯`——先 `archBlockerRounds` 把任务钉在 architect 的 `info_insufficient`、**再** `POST /tasks/{id}/cancel`（不等这一下任务还在 `init`，红灯会落错工位），断言那一盏带 `x` 变体、行归属 `architect-design`、计算色**逐字等于页内 `--stop` 探针**（换主题不用改断言）且非 `transparent`。**票 03**：§9 `lib/menuTrap.test.ts` 20 条——绕回（含长度 0 不除零）、`decideMenuKey` 逐条（**ArrowUp 从第一项回触发钮**、焦点没进过面板 Escape 也关得掉且**不**还焦点、关着按 Escape 不动作、`current = -1` 按「第一项之前」处置、空列表 no-op、不认识的键不 `preventDefault`）、点外关闭四条；**`TopBar.test.ts` 11 条一条不改、全绿**（黑盒经 `window` 派发、断 `document.activeElement`——决策 251⑤ 把它定为硬约束）；§8 e2e `talk.spec.ts` 的 ⋯ 班次菜单用例**补四条**：ArrowUp 回触发钮（不绕到末项）、`End` 落末项、末项再 `ArrowDown` 绕回首项、`Home` 落首项——Talk 那份此前**零单测**、e2e 也只盖 Escape / ArrowDown / 点外三条；**票 02**：§9 `lib/talkTurns.test.ts` 16 条——分类透传（四种 `kind`、**failed / console 的边界不认正文前缀**、`proactive` 与 `kind` 正交、`thinking` 空 / 纯空白→`null` 且非空**不 trim 正文**、`attribution` 只透传后端的 `attribution_label`）、提议按时刻**合流**（夹在两行消息之间、`session` 为 `null` / 空表不炸）、在飞三态（`pending`、`live` 占位句与 tools / thinking 跟流、`send-error` 经**回调**判配对）与 **partial 三边界**、**排序两条**（同刻 rank 兜底——换三种输入次序跨档结论不变、异刻按时间且与 `kind` 无关）；`lib/delegation-scan.test.ts` 再加 4 条守卫（Talk 从 lib 取 `buildTurns`、不再内联哨兵 `startsWith`、`stamped` / rank 三元式归 module、**配对谓词仍留 Talk**），**变异验证** 4 条能红（摘回调 + rank 0→3），回滚复绿 | 起因是一次架构评审（候选 10「从对讲台抽出判断，接线与版面留下」），两轮拷问 Q1–Q8 定稿（决策 251）；实现 2026-09-23（票 01 / 03 先落，票 02 同日续做）：最终单元 **63 文件 / 762 条全绿**、`npm run build` 绿、`svelte-check` 0 错误；e2e 曾因并发会话在飞的 Rust 卡编译，恢复后**已全部补跑**——talk + pixel **37/37**、全量 **117 通过 / 3 失败**（2 条复跑即过，属加载抖动；1 条 `market-install.spec.ts:218` 归并发会话在飞的市场改动，本批未碰任何市场文件），票 02 落地后再跑 **talk e2e 30/30** |
| 259（配对 403 的机器可读 kind，票 talk-judgments 04；同批并入票 05 动作身份收口） | **票 04**：`crates/app/src/stream.rs` 3 条单测——pairing 拒绝**带 kind 且报文逐字原样**（决策 189 的话一个字不动）、跨源拒绝**不带 kind**（两个 403 分得开是界面敢按 kind 分支的前提）、`forbidden` 默认无 kind；为此把两处拒绝抽成 `pairing_rejected()` / `origin_rejected()` 两个构造函数——中间件本体要真请求才跑得到（L3 的地盘）；`lib/sharePairing.test.ts` +4 条钉 `isPairingRequired`：配对 kind→true、跨源 403 无 kind→false、**报文写着「还没配对」但 kind 不是→false**（字样 spoof）、非 ApiError / status 0→false；`lib/talkTurns.test.ts` 的 send-error 重写 1 条（配对与否只看上游布尔，报文 spoof 不再影响）；`lib/delegation-scan` 守卫 1 条（Talk 无 `.includes('还没配对')` 谓词、`isPairingRequired` 的 import 指 lib、lib 判 `kind === 'pairing_required'` 且剥注释后无 includes、`buildTurns` 收布尔）。**票 05**：`stores/board.test.ts` +2 条（把提交拖在半路，断言 `actionBusy === 'continue:c-main::'` 四段式；**同名不同游标不是同一个忙**——§12.3 那一行的语义）；`lib/delegation-scan` +2 条（board / TaskCard / Talk 三方 import 都指 `lib/actions`；三处旧两段拼法固定串全仓清零）。纯函数侧由既有 `lib/actions.test.ts` 覆盖，未重复。 | 实现 2026-09-23（两票同日）：前端单测 **63 文件 / 770 条**全绿、`svelte-check` 0 错、`npm run build` 绿；Rust `cargo fmt --check` 绿、`clippy --workspace --all-targets -D warnings` 绿（修掉本批一处 `needless_borrow`）、`cargo test --workspace` **1155 通过 / 0 失败**；e2e `talk + create-flow + happy-path` **36/36**。**已知缺口**：L3 真请求级的配对 403 断言未加——`tests/integration/api_contract.rs` 在并发会话手里，本批不碰（生产者形状由单测钉、消费端由前端测试钉，中间「真请求过中间件」一段暂无自动化，见决策 259）。 | 
| 260（回话中刷新页面仍看得见那一轮——修用户报的毛病） | §6 `tests/foreman.rs` 新增 1 条 `a_running_turn_is_readable_and_drops_the_moment_it_ends`：一个停在模型调用里的替身（`Notify` 放行）把「正在跑」变成可观测窗口——**在跑时为真**、**跑完即假**、**失败那一轮也摘**（提前 `?` 退出那条路）、**说的是那一班**（别的班次不被连带说成在跑）。§7 `api_contract.rs` 新增 1 条 `the_session_payload_says_whether_a_turn_is_running`：空 home 为假（字段在场、加性改动）→ 一轮停在模型调用里为真（且台账里只有用户那一句——**界面手里那段流式文字不在台账里，「在跑」正是它唯一的依据**）→ 放行答完后翻回假且台账两句（**假读数比没有更坏**：留着的话界面永远以为它在说话）。§9 `realtime/foreman.test.ts` +4 条（`maxLedgerId` 取最大行 id 且与顺序无关 / 接手那一刻已有的行不算落地 / 落地那一行是哪种 `kind` 都算 / 空台账与更小 id 都不算）；`lib/talkTurns.test.ts` +2 条（`following` 为真时出那一轮——没增量摆「对面在动」的实情、有增量摆正文；`following` 为假时单靠 `stream.text` 也出轮，两条来源各管各的）；`realtime/foreman.test.ts` 另有 2 条钉**超时接力**（决策 260④：本地放弃之后那一轮的 `partial` 翻假——它仍在流，而增量照旧接得上）；再加 5 条钉**收场三支**（决策 260 裁决③：仍在跑 → 继续跟；台账尾部多一行 → 落地；两者皆否 → 死轮按 `failForemanStream` 姿态收——其中一条**承重**断言「半截字在死轮那一支必须原样留着」，把「已经出现的文字任何一支都不许清掉」这条模块级纪律钉在实现上）；`lib/delegation-scan.test.ts` 2 条静态守卫（落地哨按 `resolveFollowOutcome` 分三支、不再就地一把梭清字；死轮那一支取 `failForemanStream` 而非 `emptyForemanStream`——**牙齿检查**：把死轮那一支改回清字即变红）。§8 `talk.spec.ts` 新增 1 条 **`回话中刷新页面`**（用户报的那条路径：发送 → 等第一截可见 → **刷新** → 断言**中段**接得住 → 落地收口成台账那一行）。装置是 mock 新增的 `drip(parts, gapMs)` 步——回话分三截滴、两段空档，于是有一个**决定性的中间态**；中段那条断言当场读一次服务端取证「台账里仍只有用户那一句、这一轮仍在跑」，故它只可能来自 `/foreman/stream`（**牙齿检查**：把闸门退回 `if (!sending) return` 并重建产物 → 该断言变红；第一版用「一个字都不写、只拖时间」的 `delayMs` 写法时摘掉闸门照样绿，那种写法里增量那半条路径根本没被走到——换掉它正是这条用例的返工记录）。**顺带修掉一条被票 04 改坏的 e2e**：`ux2-flows-and-copy.spec.ts` 的配对 mock 缺 `kind`，而决策 259 之后判据按字段走，那条指引不再出现（mock 补成生产 `pairing_rejected()` 真下的形状）。 | 实现 2026-09-24：Rust `cargo test --workspace` 全绿、`clippy -D warnings` 绿、`fmt` 绿；前端 `svelte-check` 0 错、单测全绿、e2e 全量 **121 passed / 27 skipped**（skip 的是两组 opt-in 取证套件；本批新加的 1 条在列，且经牙齿检查确认有牙）。 |
| 261（出厂技能：白名单种入 + 点名指针，票 foreman-operate-pipeline 01–04） | §6 新文件 `tests/integration/factory.rs` **9 条**——播种四语义（空 home 首启种入且 `discover` 认得出 / 改过一字不动 / 删过补回 / **mtime 不变**钉住连启幂等）+ 卸载拒白名单（**遍历 `FACTORY_SKILLS` 断言**，不手抄名字；报文含「出厂技能」「不可删除」且文件原地还在）+ 点名四语义（无行建行**只带点名** / `NULL` 补值保留其余字段 / 用户值与清空 `Some("")` 都不动 / 二播返回 false）；§6 `tests/foreman.rs` **4 条主缝**——人对话轮与值守轮的 system prompt 均含点名**且位置在人格段后、工具纪律段前**（取证取模型真正收到的那份）、`Skill(name=operate-pipeline)` 真拉正文（回灌 messages 含「永不绕过按键」「逐票一卡」标志句、frontmatter 不回灌）、拉完手册产出的 `task` 提议 args 与按钮直发参数 **canonical 字节全等**（`to_string` 比对，不只比结构）；§7 `api_contract.rs` **2 条**——出厂技能列表 + 预览 `body_available` + 磁盘逐字等于内嵌 + `DELETE` 400 报文 + **对照组普通技能照删**、点名经阶段配置端点播种→用户清空不覆盖→删行（升级路径）重播补回；§5 `serve.rs::startup_seeds_factory_defaults` 源码扫描守卫（启动接线本体，判据读源码文本照 `types.rs` 先例） | 已有用例（2026-09-24） |
| 265（结构化选项提问 `.turn.ask`，票 foreman-capability-gaps 01） | §6 `tests/foreman.rs` 新增 3 条：`an_ask_lands_on_its_row_with_options_and_never_becomes_a_proposal`（载荷落那一轮 assistant 行、永不进提议通道）、`only_the_first_ask_of_a_round_lands`（第二个 ask 执行点被拒、痕迹 `ok=false`）、`a_malformed_ask_is_refused_to_the_model_and_lands_nothing`（坏载荷不落半成品）；冻结契约 21→23（`the_foreman_tool_set_matches_the_frozen_contract`）；`the_automatic_turn_cannot_reach_the_expensive_tools` 扩为 4 禁 + 4 得（值守轮不问人）；§7 `api_contract.rs` 新增 1 条 `an_ask_row_carries_its_options_on_the_wire`（`kind="ask"` + `ask` 载荷由后端给、普通行仍 `fm`）+ label 清单 21→23；§9 `talkTurns.test.ts` 提问轮 describe **4 条**（载荷透传 / 开场 mine 不算已答 / 后到 mine 即已答 / 其余轮 null）；e2e `talk.spec.ts` 独立 describe「对讲台 · 提问轮」（渲染 + 刷新存活 + 点选回发 + `.turn.fm button` 计 0——**装置照修复提议组：独立 app 空脚本**，共享 app 那版整文件偶发红，根因是值守轮偷吃注入轮次） | 迁移 0026 加列 `ask_json`；「已答」是 `buildTurns` 纯派生、无过期机制（265③） |
| 266（受治理网口 `web_fetch`，票 foreman-capability-gaps 02） | §6 新文件 `tests/integration/web_fetch.rs` **6 条**：回环取回正文且按会话维度落命令台账（GET 形态 + 预览 + 退出 0）、未放行远端在**发请求之前** `PolicyDenied` + `EGRESS_DENIED_EXIT_CODE` 留行、非回环裸 `http` 判拒（退出码留空，照 `refuse_readonly`）、显式 `timeout_sec` 打得到点上、非文本 content-type 拒收、**档位不管它而值守轮 deny 摘掉**（与 265 的 `ask` 合钉在同一条） | 白名单复用决策 179 同一张 `NetworkPolicy::allows`，**零新配置面**；不跟随重定向（放行域 302 到未放行域是开口子） |
| 267（内容搜索 `search_content`，票 foreman-within-boundary 01） | §6 新文件 `tests/integration/search.rs` **8 条**：域内正则命中且回执带 `路径:行号:行` + 台账 exit 0、`data/` 前缀与域外绝对路径 `PolicyDenied`（留行、退出码空）、坏正则 `Validation` **不落行**、命中 200 行 cap 带截断标注、二进制（首块含 NUL）跳过、**不跟符号链接**（指向域外的链接一步都走不进）、三档都广告而值守 deny **不摘**；冻结契约 23→24（`search_content` 插 `run_readonly` 之后）；§7 label 清单 23→24 | 纯 Rust（`regex` crate + `std::fs`），零系统二进制；三上限（200 命中行 / 1MiB 单文件 / 1 万文件）+ 单行 300 字；档位豁免与值守可用照 237 / `run_readonly` 先例（267④） |
| 268（离线通知 `[notify]` webhook，票 foreman-within-boundary 02） | 跨语言 fixture `tests/fixtures/notification_policy.json`（22 行）**双端同方向**钉：Rust `notify.rs::shared_fixture_covers_the_notification_policy_table`（形状守卫 + 6 条按名必需行）+ §9 `notificationPolicyFixture.test.ts` 读同一份；`notify.rs` 纯函数边界钉子（`start==end` 恒不静音 / 含头不含尾 / 跨零点）；§6 新文件 `tests/integration/notify.rs` **6 条**：`wakes()` 出站一条通用 JSON（字段按 268④ + POST 方法行）、同类 300s cooldown 挡第二条而 advance 301 后放行、免打扰本地 23 点 `done` 静音而 `pending`/`failed` 照发、`SlowRun` 永不出站（金丝雀反证）、无出口时记账照常、kind→class 映射 12 项全表；`config.rs` 3 条（缺省关死 / 未知键拒 / 显式解析） | 触发点 = `note_attention` 单一漏斗且 `wakes()`（与值守轮同一信号面，LLM 挂了也发得出）；礼貌语义镜像 `notificationPolicy.ts` **同一张表**（fixture 先例照 246）；投递 best-effort + 日志 `without_url()`（URL 是秘密）；130③ 前端 toast 不动；`cancelled`/`notifyOn` 差异显式记在 268 与 fixture `$comment` |
| 269（上下文压缩，票 foreman-within-boundary 03） | §6 `tests/foreman.rs` 新增 **4 条**：超预算压成头部锚点（标记 + 摘要正文同在、**锚点与窗口同一本预算** ≤24k、摘要器无工具带专用指令且输入含掉出预算的最老轮）、预算内零额外调用零锚点、两轮**增量再压**（旧摘要进第二份输入、最老原文不整段重发、锚点跟上新摘要）、摘要器失败**回退现状**（轮次照常成功 + 回退路径无锚点）；两条既有钉子（预算裁剪留最新 / 最新一条永保）**原样不动** | 触发在 `respond_inner` 裁剪处（269② 跨线时刻）；缓存每会话 16 槽 FIFO、`covered_until` 单调（269③），锚点不落库、重启重算；摘要 = 同 `llm.complete` 同 provider 的无工具小补全（60s 上限、4k 字截顶）；失败不写缓存下轮再试（269④）；值守轮共用 `respond` 零分支（269⑤） |
| 270（飞书报文分支 `[notify].format`，票 foreman-within-boundary 04） | `notify.rs` 单测 **2 条**：`generic_payload_is_the_six_field_contract`（六字段 + 归因白名单在 + 原文不出网）、`feishu_payload_is_a_text_message_with_keyword_prefix`（`msg_type=text`、关键词前缀 `[AgentPipeline]`、原文不出网、不带通用字段）；`config.rs` **3 条**：缺省 `generic` / 显式 `feishu` 解析 / 非法值解析期拒；§6 `tests/integration/notify.rs` 新增第 **7** 条 `feishu_format_posts_a_text_message_instead_of_generic_json`（format 不改触发面：`wakes()` 照发、文本消息形状、前缀 + kind + task_id、`boom` 不出网、POST 方法行） | 政策面（cooldown / 免打扰 / kind→class / 触发面 / fixture）一行不动（270③）；归因白名单在 `payload_for` **分流前共用**（270②，纪律不因格式松动）；缺省 `generic` = 268 契约零行为差异 |
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
| L1 单元 | `crates/core/src/**`（in-crate） | 480 | routes 全 `EdgeKind`（含 merge 闸门耗尽收口、code_gate 通过即放行、review 不通过→user_decision；sync-check 回溯**不在** `EdgeKind` 里——它不经 `route()`，改由 L2 的反向不变量钉住）、落点表逐行、metadata 三级降级、FileToolPolicy（realpath / deny / symlink）、脱敏、L1 裁剪 / L2 唯一阈值 / L3 压缩规则表 / L4 兜底、prompt 组装 golden（§10.3 十二节点内嵌模板 + AGENTS.md + stage_configs 消费 + `prompt_template_hash`，票 12）、backtrack 反馈注入范围（决策 126：仅 architect validate_input / execute、首轮不渲染）、allowed_actions 权威表 + 端点按行配对静态检查、焦点投影、指标口径、SSE 事件体（`stage_changed` / `task_done` / `task_cancelled`）、工具真实执行（test-report.md 落任务目录 + **`run_command` 运行期周期心跳**，票 13）、**生产适配器协议解析**（票 13：OpenAI 兼容 / Anthropic 的请求体映射、流 chunk 分片聚合、usage 与 cache token 解析、`[DONE]` / `message_stop` 终止、坏载荷干净报错、base_url 回落） |
| L2 集成 | `crates/core/tests/` | 322（另 3 条 `#[ignore]` 真 LLM 冒烟） | 游标生命周期（创建 / 分裂 / 合并 / 回退 / 重试 / partial UNIQUE / 永不物理删除 / run 外键不悬空 / **损坏行 fail fast** / **cancel 只挂未启动依赖方** / **backtrack 标过期同事务 + upsert 清除**）、git 链路（init / rebase / 冲突 abort / ff 与非 ff 合入 + `update-ref` 写回 / reset --hard + clean / 清理幂等 / unborn HEAD 明确报错 / 非 origin remote 的基准回落）、scheduler tick 六项职责（超时链 + 进程组终止器 + 节点/阶段/全局超时层级、冲突恢复含复检、依赖三态与恢复、准入、stalled 谓词 `has_runnable_cursor`、**纯 name 重合降级 warning**）、executor 循环（FakeAgent 驱动完整 happy path + sync-check system run 恰一次、单分支 pending 不阻断另一分支、单执行者双保险、元数据失败干净对话重试、会话截断、prompt 组装消费、**`tool_event` start/end 成对发射**、**压缩后仍超硬限收口为 `pending(context_overflow)` 且该退出路径补写会话行**（决策 105 / 154 / 180 必要条件一））、**生产适配器对 mock server 全链路**（票 13：`testkit::mock_llm` 手写 HTTP server；OpenAI 兼容流聚合 + cache token + conversation_delta + 心跳刷新、Anthropic 双头鉴权 + system 顶层 + tool_result 合并 + 计量归一、deepseek 分发、provider 解析优先级（决策 129 四级：node_overrides > 任务覆盖 > 阶段配置 > 系统默认）、HTTP 401 / 坏流 / 未知 vendor / 无 provider / 禁用 provider 的干净报错） |
| L3 API | `crates/app/tests/integration/api_contract.rs` | 170 | POST/GET /tasks 与过滤、循环依赖与 provider fail fast、`GET /tasks/{id}` 的 allowed_actions 与 blocks、resume 的 409 / 动作集 / 冷却防连点 / **dependency continue 不 spawn** / **goto 入口节点校验**、merge/decision（approve 与 return）、人工评审（**comments 进流转原因**）、retry（**worktree 硬重置 + system 命令入账**）/ cancel / archive / split / model-override、项目 CRUD 与 202 异步分析、provider `***` 回显、**跨源防护矩阵全覆盖**（自定义头 / 无 Origin / 本机 Origin 严格相等 / 恶意 Origin 与**前缀伪装** 403 / 同源 Referer 带路径放行 / GET 不受影响 / **`allowed_origins` 扩权仅精确放行**，决策 157）、SSE 通道、会话与命令 API（**按 task 隔离**，含卸载输出）、任务产出文件与目录逃逸防护、**前端静态资源同源托管**（决策 155：`/` 内嵌 index 或构建提示页、`/assets/{*path}` 原样回放 + Content-Type、未知资产 404） |
| 冒烟 | `crates/app/tests/integration/smoke.rs` | 3 | E2E-00：spawn 真二进制 → 就绪 → **`/` 同源托管 200（决策 155）** → 0700 目录权限 → 无 provider 创建任务明确报错 → SIGINT 优雅退出（退出码 0）；端口占用明确报错 |
| L4 E2E | `tests/e2e/tests/` | 40 | `happy_path.rs`：E2E-01 happy path（游标分裂 → join → 合并 → 归档序列、真 git worktree / 提交 / ff 合入、`default_branch` 前进、worktree 与分支清理、system run 落库、token 与调用次数汇总、命令日志、流转时间线）、E2E-09 基准前移使 approval 失效、决策 108 的 `gate_failures` 不被 upsert 清零、E2E-08 的 retry 段（worktree 硬重置 + 重新准入）。`join_and_skip.rs`：E2E-02 sync-check backtrack（双游标归档 → main 指 architect.validate_input、设计文档标过期、`backtrack-feedback.md` 落盘、重入 prompt 含反馈段、attempts 归零）、E2E-11 skip 矩阵（architect skip → 分裂；develop-design / test-design skip → `waiting_join`+`skipped_to_join`、sync-check 视 readiness=true、不伪造产出元数据、下游 prompt 决策 115 降级）、E2E-12 尾段（pending 分支 resume 后 join 恰一次） |
| testkit | `crates/testkit/src/**` | 21 | 临时 home、假时钟、记录型终止器、git fixture（干净 / unborn / remote / 脏 / 可自动合并 / 不可自动合并 / 多语言 / symlink 陷阱）、FakeAgent 脚本能力、断言助手、**mock LLM HTTP server**（票 13：路由前缀匹配 + 请求记录）、**跨语言 golden fixture 的生产侧断言**（票 e2e-mock/01：`sse_tool` / `sse_text` 与 `tests/fixtures/e2e_mock_sse.json` 逐字段一致；伪阶段 marker 三条含 `pseudo:project_analysis`） |

**票 15–22 交付（2026-09-12）：** L1 新增伪阶段（`pipeline/pseudo.rs`：conflict_check 语义层 / validator_cross_check 异族复判 / project_analysis 摘要）与 decision 135 的 continue/goto 特判；L2 新增 merge 闸门复检环（`gate_recheck` 注入 → test 复检 → 重跑闸门）、rebase 自动合并（可机械判定的冲突自动解决，硬冲突仍 abort 打回）、崩溃恢复（决策 152 in-process：中断节点续跑 / 双游标独立恢复 / `waiting_join` 跨重启 / `executor_owner` 清理重准入）、单执行者 `try_run` 信号（resume 重试防静默丢弃）、进程组真实 pgid 回填；L3 新增 `stage_configs` CRUD 契约（未知阶段 / 不可用 provider / 不可读 persona / `cross_family_judge` 依赖的拒绝）与 `project_analysis` 接入 `analyze`（LLM 不可用时保留确定事实并记 `summary_error`）；L4 补齐 §8 矩阵（E2E-03/04/05/06a/06b/07/08/10/14/15/16/17/18/19/20/21/22/23/24，E2E-13 归票 18）；testkit 新增 `fail_tool_n` / `long_tool_result` / `backdate_run` / 伪阶段脚本 / mock-LLM responder。前端 `frontend/`：看板与实时流（fetch 流式 SSE + `reduce.ts` 归约表）、任务详情与 resume dossier（`allowed_actions` 纯渲染）、项目 / provider / stage_configs 配置页与轨道分段指标图。

**测试暴露并修复的三个生产缺陷（2026-09-12）：** ① 脏工作区挂起后 `merge_phase_b`（现 `merge.rs::phase_b`，决策 249 随迁改名）仍返回 Route，`route_merge` 按 approval=approved 把游标推进到 `done`（改为 `NodeOutput::Pending`，挂起不推进游标）；② `first_layer_conflicts` 的纯 name 重合 Low 告警被 executor 用 `!is_empty()` 当作硬冲突（改为只对 `duplicate_risk=High` 挂 `conflict_wait`，Low 仅告警，与 scheduler 复检的精确 (module,name) 判定一致）；③ resume 请求落在旧 executor「已读完游标、未释放注册表」窗口内会被静默丢弃、任务永久 pending（新增 `Executor::try_run` 报告是否真正执行，resume 钩子据此有界重试）。三者均有用例钉住。

 **2026-09-12 偏离修复（文档-实现对齐 pass）：** 依据文档权威裁决，修正了已实现代码与文档相悖的行为——路由四处（code_gate 先判通过、merge 闸门耗尽进 `pending(retry_exhausted)`、review 不通过进 user_decision、sync-check 的回溯由 `advance_join` 经 `SyncDecisionKind` 判定并以 `Store::backtrack_cursors` 落库——sync-check 不占游标行故不经 `route()`，`EdgeKind::Backtrack` 是不可达死代码、已删（决策 245 / 票 01））、goto 落点校验、cancel_task 只对未启动依赖挂 dependency_failed、dependency_failed 的 continue 不 spawn、cancel/archive 回收 worktree 与分支、retry 的 `git reset --hard` + `clean -fdx`（记 system 命令）、纯 name 重合降级 warning、test 阶段 `test-report.md` 写任务目录、跨源严格相等（含 `localhost`）、db 文件 0600 + `-wal`/`-shm` 纳管 + 检查前 realpath、git.rs 三处（unborn 判定 / origin 基准 / 兜底删除的 worktree 标记核验）、skills 校验改对真实 PATH 可执行集合、`[server] host/port` 接线、SSE 线协议命名（§12.7）、human_review 动作名 `approve`/`reject`（端点按行配对）、`NodeStatus` 更名、`MergeResult` 必填字段 + `gate` 无 Default（缺行 ≠ 通过）、merge `output_type` 定名 `merge_result`、损坏数据 fail-fast 分类（执行语义字段报错，观测字段 warn + 兜底）。文档同步修订：决策 70 / 128（改旧行）、§11.5 schema 补全、SSE 表补 `stalled`、agents.md 配置示例对齐。

**尚未实现（下一步）：**

1. **票面剩余**：无。票 01–22 与 `.scratch/agentpipeline-v1-closeout/` 的票 01–18 全部实现（见下方「收尾批次」）。
2. **行为级缺口（文档已定义、当前无生产者）**：仅剩 `task_failed` 终态——决策 70 已裁决 `failed` 变体保留但 v1 无生产者，用户主动终止走 cancel → `cancelled`，系统级失败终态留待有真实生产者时启用（非缺口）。其余原缺口均已关闭：
   - `review_diff` 产出（决策 124）→ 票 13，人工评审前生成 `review-diff.diff` + stage output；
   - review 打回后 `required_changes` 注入 develop prompt（决策 133）→ 票 07；
   - 重试摘要回架构设计注入（决策 138）与 info_insufficient 补充输入注入（决策 79）→ 票 08；
   - `context_overflow`（决策 105 / 110 的 L4 兜底）→ 票 04，E2E-22 走真实执行路径触发。**兜底接线已关闭，但 L4 的第二级（分批 / 拆子代理）是「有意保留的能力缺口」**（决策 154）：v1 的 L4 只有两级——压缩 → `pending`，压缩后仍超限一律交用户三动作处置；`L4Plan` / `L4Action` / `plan_l4` 那组死代码已删。**重开条件可核对**：生产库出现真实 `context_overflow` 落库记录，**且**用户以现有三动作处理后任务仍无法推进到终态；
   - `dependency_overridden` 警告（决策 116）→ 票 06；
   - review / test `code_issue` 的 pending 补 `context.kind`（决策 130 ①）→ 票 05；
   - retry 的会话归档（§12.2）→ 票 11（迁移 0003 加 `archived_at`）；
   - `GET /tasks/{id}/conversations/{run_id}/messages` 端点（§12.4.3）→ 票 12；
   - 项目级伪阶段独立观测行（决策 100 / 130②）→ 票 10（迁移 0004 放开归属）。
3. **真 LLM 冒烟**（`#[ignore]`）——已落地（`crates/core/tests/integration/llm_smoke.rs`，`AGENTPIPELINE_SMOKE_*` 环境变量驱动，验收流式 + 计量 + 结构化输出解析）；未用 rig，生产适配器为手写 reqwest 实现（`crates/core/src/agent/providers/`），`client.rs` 的「rig 适配层」注释以本条为准。
4. **文档已定义、实现留空的配置面**：全部接线完毕（票 16 / 17 / 04 / 14 / 15）——`[logging] format / file`、`prompts.dir`、`adaptive_timeout_enabled`（告警侧）、`run_command` 输出流式 SSE、脱敏的环境变量值、上下文 L2 泛化 / L3 按轮 / L4 接线、`model_context_window`（provider 行，实现住 `pipeline/model_request.rs`——决策 249 随迁）。
5. **测试基建注记**：FakeAgent 的 agent loop 会耗尽同节点脚本队列（每 attempt 吃到队列干涸为止），多轮行为测试须按 `set_script` 分轮投喂（`tests/e2e/tests/integration/common/mod.rs` 头注）；executor 注册表以 task_id 为进程全局键，同进程并发测试须用互不相同的 task_id。

**工具名校验收紧（2026-09-17，决策 154 的后续票）：** `tools_json` 里 v1 不认识的名字由「静默丢弃 + 一行 `warn`」改为**拒绝**——写入时 `PUT /stage-configs` 400、启动时 `validate_startup` 失败，报文都列出未知名字与 v1 已知工具集（8 内置 + `spawn_sub_agent`，判据只有 `client::is_known_tool_name` 一处、报文只有 `client::unknown_tools_message` 一处）。形态非法的 `tools_json`（非数组 / 非字符串项）同样拒绝：旧行为会把它们静默忽略，那正是本票要关掉的那条路——**故 UI 的 `tools_json` 占位符同步改成只示例数组形态**（原占位符示例的 `{"execute": [...]}` 对象形态后端从未支持过，是被静默忽略的那一类）。L1 用例：`agent/client.rs::known_tool_names_is_builtins_plus_extended`（判据不许比内置集多认一个名字）、`config.rs::unknown_tool_names_fail_startup_validation`、`pipeline/model_request.rs::tool_defs_rejects_unknown_names_and_accepts_the_known_set`（决策 249 随迁）（执行期兜底与校验**同一种处置、同一句报文**）。L3 用例：`api_contract.rs::stage_config_write_rejects_unknown_tool_names`（400 报文形状 + 不落库 + 9 个已知名字逐个可写）、`…::startup_rejects_an_existing_stage_config_with_an_unknown_tool`（存量数据的升级路径：**确定地报错并指明是哪个阶段的哪个名字**，不静默放行也不自动清理）。前端：`api/client.test.ts`（400 报文原样抛成 `ApiError.message`）+ `components/settings/StageConfigForm.test.ts`（表单原样渲染那句报文；提交不私自改建名）。

**收尾批次（`.scratch/agentpipeline-v1-closeout/`，2026-09-13）：** 票 01–18 全部实现，§11 原「代码评审记录的偏差与遗留」逐条关闭：
- 决策 100 偏差 → 票 10（项目级 run / 会话行，迁移 0004）；
- 决策 109 偏差 → 票 09（闸门完整日志落 `gate-output-{stage}.log`，复检读全文）；
- 决策 153⑤ 未实现 → 票 02（`serve` 沉 lib + `127.0.0.1:0` 回读端口 + 就绪行）；
- playwright 两条 E2E 未执行 → 票 18（`frontend/e2e/`，只 Chromium，`make check-e2e`）；
- 结构性待清理四处 → 票 01（e2e `Flow` 回灌公共模块）/ 票 03（rebase 逻辑回 git 层、action→endpoint 单一事实来源、judge continue 落点复用 `StageLanding`）。

**git 层技术选型（2026-09-12 用户裁决）：** 决策 12（git2 + `spawn_blocking`）与决策 146 原文（生产走系统 git CLI）此前互斥，用户拍板统一 git2——生产 git 层（`crates/core/src/git.rs`）已全部重写为 git2，testkit fixture 保留系统 git CLI 仅作测试脚手架（决策 146 已改旧行）；merge 阶段 B 随之改为内存合入（决策 73 / 97 已补注），git 链路 14 条测试全部在 git2 实现上通过。

