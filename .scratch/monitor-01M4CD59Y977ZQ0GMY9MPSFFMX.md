# 监控记录：任务 01M4CD59Y977ZQ0GMY9MPSFFMX

- 标题：审视所有页面文案，有哪些是不适合呈现给用户的都改掉
- 描述：审视所有页面文案，把不适合呈现给用户的文案都改掉。
- 项目：AgentPipeline（`01M3QEG4AR3V42GKR0YG9GH897`），review_mode=agent（无人审档，自动走完）
- 服务器：106（root@106.12.12.6，服务 agent-pipeline，端口 3389，应用自身 TLS）
- 查询方式（ssh 内，回环豁免配对闸门）：
  `curl -sk https://127.0.0.1:3389/tasks/01M4CD59Y977ZQ0GMY9MPSFFMX`（detail / flow / metrics）
- 创建时间：2026-10-08T00:03:41Z（本地 08:03），worktree `/root/.agentpipeline/worktrees/01M4CD59Y977ZQ0GMY9MPSFFMX`
- **时点背景（关键）**：106 上的服务于 **07:55:10 CST 刚重启过**，跑的是 `d4e11b9`
  ——正是「节点内消息级恢复（决策 401–405）」这版新代码。本任务是该功能上线后的
  **第一个生产任务**，监控同时兼作新恢复路径的首次实战观察。

## 基线快照（2026-10-08 08:05 CST / 00:05 UTC，监控开始时）

- 状态 `running`，阶段 `architect-design`，节点 `validate_input`，run=371，attempt=1
- 过渡史 2 条：`start`（并发准入放行）→ init/execute；`normal` → architect-design/validate_input
- 模型请求 request=7718→7723，全部 `status="ok"`，间隔 7–13s，节奏正常
- 指标：init 1 run（1409ms）；architect-design 1 run（尚未收口）；total_calls=1、total_tokens=0
  （与 stored 两处一致）
- 服务：systemd active（07:55:10 起）；journal 今日零 WARN 级及以上条目
- 磁盘：68% 用量 / 剩 13G（/dev/vda1 40G）——高于上轮监控时的基线，但未近告警线
- 服务器 load average 读数 `4294967296.xx`（=2^32）——内核 getloadavg 计数溢出的老问题，
  非应用缺陷，**记为环境噪音**，不作为问题跟踪

## 观察项（开监控时立的）

- **N1（新功能实战）**：d4e11b9 的消息级恢复是首次上线（服务 07:55 重启，本任务 08:03 起跑）。
  关注：① 恢复正常时，节点内消息日志是否按「每条消息一行」落库；② 若任务执行中服务再被重启，
  是否真的从最后一条消息续接、且合成回执（「可能已部分生效/没有执行」）如实落库。
- **N2（历史高发复现观察）**：
  - `submit_metadata` 参数 JSON 截断救援（上两轮各 5 次与 3 连发，根因疑在上游网关 `finish_reason=length`）；
  - 长节点（>90min）超时 → 续接 → 空白重跑的循环（上轮 ux-audit 任务 7.6 小时未落盘的主因）；
  - merge 测试闸门 `cargo test --quiet` 600s 超时踢回（上轮复检期曾引发 gate_failure 重复推送，已按决策 388 修复，本轮是修复后的第一次真实复检）。
- **N3（成本/磁盘）**：上轮 token 到 1.15 亿、磁盘曾 3.5 分钟掉 740MB；本轮逐轮对盘，
  超 6G 告警线前处置（构建产物在 `/root/.agentpipeline/shared-target`）。

## 巡检记录

- **08:05（CST）第一次巡检（基线）**：见上。任务刚起跑 2 分钟，validate_input 正常推理中。
- **08:08–08:11（CST）专项核查 N1a（消息级恢复日志是否真在跑）——已证实**：
  直查 106 库 `kanban_node_messages`，run=371 运行中已有行且在增长：
  08:06 查 41 行（seq 0–40）→ 08:08 查 44 行（assistant 17 / tool 27，最新 created_at
  00:07:51Z）→ 45 秒后再查 **47 行（max seq 46）**。即**逐条/逐批增量落库**成立
  （不是循环退出后一次性写块），此刻若崩溃，续接点就是 seq 46 而非从零。
  全部 `synthetic=0`（合成回执只在真有中断时出现，符合预期）。
  待验的只剩 N1b（真发生进程重启时的续接与合成回执），目前无重启。

> **【巡检规则更新】** 任务处于**人工暂停**（pending / user_paused）期间：无变化就不必逐轮
> 新增记录行（避免刷屏）；恢复跑、被人工动作改变、或出现新事件后再续记。原文的三问见下节。

## 事件：三个子代理连环失败 + 人工暂停未能即时止住子代理（08:16–08:20 CST）

时间线（CST；UTC = CST − 8h）：

- 08:16:17 `architect-design/validate_input` 成功收口（run=371，745s，prompt 1.13M）→ 进 execute（run=372）
- 08:16:31 run=372 一次 `read_file` 未命中（`frontend/copy-discipline.test.ts`）——正常试错
- 08:17:03 run=372 **一条 assistant 消息里带 3 个 `spawn_sub_agent` 并行调用**
  （`kanban_node_messages` run=372 seq 16，三个 tool_call id：`call_525ce…` / `call_1f787…` / `call_ec695…`）
- 08:17:53 子代理 #1（run=373）**失败**：`校验错误：子代理在 12 轮内未收口，请把子任务拆得更具体`
  （12 轮打满：2×list_dir + 20×read_file）
- 08:17:53 子代理 #2（run=374）起跑
- **08:18:47.696 用户按下暂停**（证据：`kanban_node_cursors.updated_at` = 该时刻，即 pause 的落账时刻）
- 08:19:10 子代理 #2 失败（12 轮打满：12×list_dir + 16×read_file）——**距按停 23 秒**
- 08:19:10 **子代理 #3（run=375）起跑（在按停之后！）** → 08:20:30 失败（12 轮打满：13×list_dir + 17×read_file）
- 08:20:30.220 执行体才在**轮边界**看到「人按停」的中止请求 → run=372 收成 `cancelled`
  （`cancel_origin=hold`）→ 游标挂 `pending(user_paused)`，本轮结论一个字未写台账（决策 276）
- 此后无任何任务侧 run 在跑；截至本次核查**全库无 `status=running` 的 run**

三个子代理的 token：373 = 180,034 prompt / 1,838 completion；374 = 177,778 / 4,037；
375 = 157,357 / 2,963（各含 131k–149k cache_read）。**三次共烧 ~51.5 万 prompt token，
换回三条同一句失败。** 任务当前总量：total_calls=5、total_tokens=1,760,561。

### 三问与根因（附 file:line）

**Q1 任务为什么失败？** 任务本身**没有失败**——它现在是 `pending`（`user_paused`，人工暂停）。
失败的是三个**子代理 run**（373/374/375），错误同一句「子代理在 12 轮内未收口」。两层根因：

- 子代理硬上限 `SUB_AGENT_MAX_ROUNDS = 12`（`crates/core/src/pipeline/subagent.rs:62`），
  打满即 `Err`（`subagent.rs:408-410`）。这上限是**防御性**的（防模型「读一个文件→再读一个」空转）；
- 子代理工具集**硬编码只读两件**：`read_file` + `list_dir`（`subagent.rs:46-49`），
  **没有 grep / search_content**。而父代理派给它的三个子任务**全是检索/扫描型**
  （「在 frontend/src 里扫出所有不适合的文案实例」「搜 decisions 里与文案相关的条目」）——
  子代理只能 list_dir + read_file 一个个翻，12 轮全用在翻文件上，永远走不到
  「模型不再发起 tool_call」的自然收口。三份转录实况：**12 轮 assistant 全带 tool_call**，
  工具用量 read_file 16–20 次、list_dir 2–13 次——**无一例外打满上限**。
- 即：**当前能力面下「让子代理去搜」是一张注定打满的票**。决策 400 刚把 `spawn_sub_agent`
  对三个设计阶段默认开启，这是它的首次生产实战。

**Q2 为什么失败后 subagent 还在执行？** 那不是「一个子代理重试三次」，而是
**一条 assistant 消息里并行的三个 `spawn_sub_agent` 调用**。工具**按声明顺序顺序执行**
（`model_invoke.rs:1227` 的 `for call in &response.tool_calls`），且
**同批里某个调用失败不会取消后面还没跑的同批调用**。#1 失败（08:17:53）时 #2 早已被模型
声明、随即开跑；#2 失败后 #3 同理。屏上就呈现为「失败一个、又起一个」。

**Q3 为什么暂停任务后 subagent 还在执行？** 因为**中止是协作式的、且只在「轮边界」被看一眼**，
而子代理循环**根本不看**这个信号：

- `pause` → `request_hold(task)` 置信号 + 游标挂 pending（`pipeline/pause.rs:97-129`）；
  执行体只在**整轮节点执行收口之后**检查 `held_by_human_signal(cancel)`
  （`executor.rs:748-760`）；模型轮循环里那一眼在**每轮开头**（`model_invoke.rs:1105`）；
  **一轮内部（工具批）没有任何检查点**（`model_invoke.rs:1227-1251` 之间没有 cancel 判断）。
- 子代理侧：`SubAgentRunnerConfig`（`subagent.rs:76-108`）**没有任何中止/取消句柄**，
  `run_rounds`（`subagent.rs:345-411`）除了 12 轮上限与 `max_duration` 外无打断点；
  它的模型调用是裸的 `self.cfg.llm.complete(req)`（`subagent.rs:370`），
  连决策 226 那个「模型调用可被中止请求打断」的 `select!` 观察点都没有（那个在
  `ModelInvoke::complete_once` 里，子代理不走这条路）。
- 实测后果（本任务）：08:18:47 按停 → 在飞的子代理 #2 照跑完（08:19:10 失败）；
  **按停之后又整整跑完一个子代理 #3（08:19:10→08:20:30，80 秒）**；
  执行体直到 08:20:30.220 才在轮边界收口让出执行权——**按停延迟 1 分 43 秒**。
  延迟的上界 = 当前工具批跑完（本任务受 12 轮约束；一般情形受子代理 `max_duration`
  ＝节点级配置约束，缺省 `node_max_duration_sec = 1800s`，`config.rs:135`）。
- 顺带排掉一个误认：按停后仍在跑的不可能是别的子代理——**全库当时没有其它 running run**；
  同期还在发模型请求的 `agent_type=foreman`（session `01M4CDY9EC9M9FSSE236GJXYZC`，
  标题「从专业ux角度分析本服务各界面的bug和优化点」）是**值守/talk 会话**，与任务无关。

### 改进候选（B 组，待用户裁决后落票实现）

- **B1（bug，本轮主问题）子代理不可被按停**：给 `SubAgentRunnerConfig` 加中止句柄
  （与 `cancel` 同源），`run_rounds` 每轮开头看一眼（同 `model_invoke.rs:1105` 的姿势）。
  更彻底的一层：在**工具批**循环里加检查点（批内每两个 tool 之间看一眼）——不然凡「一轮里
  工具批很长」的节点（不只子代理）都要等整批跑完才认按停。子代理是只读工具集，
  批内打断安全；父节点含写工具时要照决策 226 的姿态另行裁决。
- **B2（设计缺口）子代理没有检索工具**：只读集 `SUB_AGENT_TOOLS` 缺 grep / search_content，
  而父代理被鼓励把「搜一批」外包给子代理（决策 400 默认开启，子代理 persona 就写着
  「检索并阅读文件」）。两条出路：① 给只读集加一个受治理的检索工具（只读、走同一
  `file_policy` 口子）；② 暂不动能力面的话，至少把失败文案从「请把子任务拆得更具体」
  改成点明「子代理只有 read_file/list_dir，检索型子任务应由父代理自己用 search_content 做」。
- **B3（行为）同批 spawn 失败不刹车**：一条消息里的 N 个 spawn 全量跑完（本次 3 个 ×
  ~50/77/80s、51.5 万 prompt token）。方向：同批内先失败即取消剩余未跑的同类调用
  （fail fast），或给「一条消息里的 spawn 数 × 轮数」设共享预算。
- **B4（可观测性）按停无可检索日志**：`request_hold` / `pause` 不落任何日志行——本次靠
  `kanban_node_cursors.updated_at` 反推按停时刻。建议 `request_hold` 落一条 INFO
  （task / 来路 / `notified`），排障时不用翻库。

## 巡检记录（续）

- **08:31（CST）第二次巡检：任务仍停在人工暂停，无变化**。状态 `pending`/`user_paused`
  （architect-design.execute，位置保留，allowed_actions = continue/goto/cancel）；
  过渡史仍 3 条（最后一条 00:16:17Z）；total_calls=5、total_tokens=1,760,561 与 stored 一致；
  服务 active 且**自 07:55:10 未再重启**（N1b 仍无机会验，无重启即无合成回执）；
  journal 近 25 分钟**零 WARN**；磁盘 68%（剩 13G）稳定；全库 0 条 running run。
  消息日志两 run 齐整：run=371 共 77 行（0–76）、run=372 共 20 行（0–19），`synthetic` 全 0。
  按上文巡检规则，暂停期间不再逐轮回写；恢复跑或有新事件后再续记。


## 收尾（2026-10-08 11:20–11:55 CST）：五票修复上线、部署踩雷、一次误触 rerun

### 一、任务其实早就醒了（本记录此前的「暂停中」已过期）

用户按停之后，任务被**恢复**过，并已跑过整条设计段（时间均为 UTC）：

| run | 节点 | 结果 | 时刻 |
|---|---|---|---|
| 375 / 374 / 373 | architect-design.execute·subagent | 全部 failed（12 轮未收口） | 00:17–00:19 |
| 376 | architect-design.execute **attempt 2** | **success** | 03:09:21 |
| 377 | architect-design.validate_output | success | 03:19:19 |
| 378 / 379 | test-design / develop-design validate_input | success | 03:20:00 |
| 380 / 381 | test-design / develop-design execute | success | 03:21–03:21 |
| 382 / 383 | develop-design / test-design validate_output | success | 03:23:26 |
| 384 | sync-check.execute（system） | success | 03:24:10 |
| 385 | develop.execute attempt 1 | cancelled（**进程重启**：03:47:21 那次部署重启收的尾） | 03:24:10→03:47:21 |
| 386 | develop.execute attempt 2 | cancelled（**人工重跑本阶段**的中止请求） | 03:47:31→03:49:37 |
| 387 | develop.execute attempt 3 | running | 03:49:37 起 |

- architect 那一轮（376）是在**旧二进制**上成功的（同一 attempt 里不再派子代理），
  失败文案里没有「三件只读工具」那句 → 决策 408 之前的措辞。
- 于是「重跑 architect-design」这件事**已经不需要做**：它早已成功，且下游 design / sync-check
  全部跑完，重跑 architect 只会把后面这些一起作废。**没做。**
- 387（正在跑的 develop）才是第一轮跑在**新二进制**上的节点；截至 11:50 它还没派过子代理
  （子代理 run 表里最新的仍是 373–375），故新代码在本任务上的**实证使用**还没发生。

### 二、部署踩雷：106 的 main 被值班长的修复提交占住（新问题，已落票）

`deploy.sh` 走 `git pull --ff-only`，而 106 的检出 **diverged（ahead 1 / behind 6）**：

- 本地那条 `ba77558`（author `agentpipeline-foreman`，2026-10-08 03:01:32Z）是**值班长修复
  轮**的产物——`[repair] 值班长修复 01M4CEFYE8XBZ3R98TKDFY3ZCV…`（21 文件 / +897 −102，
  前端 hero 横滚、`talkDraft` 中流持久化、`actionTier` 四档那批）。
- 它**没有推到 origin、也没有构建进正在跑的二进制**（服务与二进制都是 07:55:10 CST 的
  那一份，早于这条提交），却足以让每一次部署在 `git pull` 那一步失败。
- 处置（非破坏）：`git branch repair/01M4CEFYE8XBZ3R98TKDFY3ZCV ba77558` 按住名分 →
  `git bundle` 带回本机 → **push 到 origin 的同名分支**（祖先是 d4e11b9）→ 106 的 `main`
  `reset --hard origin/main`（**任务 worktree 与 `kanban/01M4CD59…` 分支一字未动**）→
  重跑 deploy → 成功（03:47:21Z 重启，二进制重建于 11:47 CST）。
- 根因是**产品级的**（不是运维手滑）：值班长的修复轮把提交落在**部署检出本身**上
  （该项目的 `local_path` 就是 `/opt/AgentPipeline`），于是「修复」与「部署」抢同一条分支。
  已落票 `.scratch/deploy-divergence/issues/01-repair-commit-blocks-deploy.md`（needs-triage）。

### 三、一次误触 rerun（如实记）

按用户原有的指令（「部署完成后重跑那个暂停任务的 architect-design 阶段」），我在 03:49:37Z
发了 `POST /tasks/01M4CD59…/rerun`。**前提是错的**——那条指令基于本记录 08:31 的「停在
architect-design」，而任务那时早已跑到 develop。该端点的语义是「重跑**当前阶段**」，于是实际发生的是：

- 在飞的 `develop.execute` attempt 2（run 386，已跑 2m06s）收到按停信号 → `cancelled`；
- 游标仍在 `develop.execute`（位置没挪，`validate_attempts` 归 0），执行体重派 → run 387。

**代价**：那一轮已烧的 token 与 2 分钟（无工作区重置、无提交丢失——`rerun` 不动 worktree，
决策 125 的 `git reset --hard` 只属于「整条任务 retry」）。**教训**：动作类指令要**先读当下状态**
再发，不能拿几小时前的巡检结论当输入。

### 四、部署验证（四项绿 + 两条反向）

- `systemctl is-active` = active；监听 `0.0.0.0:3389`；二进制重建于 11:47 CST，服务 11:47:21 重启；
  HEAD = `a7f4785`（决策 406–410 全在）；二进制里能查到新文案（「子代理被中止」「同批前一个」
  「三件只读工具」各命中）；
- 外网**导航**请求 `https://106.12.12.6:3389/` → **401 + 配对页**；非导航 → **403 +
  `kind: pairing_required`**（`/tasks` 实测）；
- **反向**：明文入口 `http://106.12.12.6:3389/` → 连接不可用（TLS 端口对明文的拒绝）；
- 一处与 skill 记载不符、以代码为准：**回环**请求（ssh 上去 `curl -sk https://127.0.0.1:3389/`）
  实测 **200**，因为闸门对回环来源**豁免**（决策 336 的 `pairing_lan_loopback_peer_is_exempt`）；
  skill 里「回环 TLS 401」那一行是旧口径。
