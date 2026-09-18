# 04: 两处留痕——系统节点与对讲台失败回合

**What to build:** 两处「出了事但一个字都不留」的地方各补一条痕。

## 4.1 系统节点留痕

`init.execute` 这类系统节点（非 LLM、非进程组）失败后台账只有一句「超时」，
看不到**卡在哪个系统调用、持着什么锁**。

**实证（2026-09-17）**：`支持rtk` 的 init run 卡了 4 小时，栈是
`Git::is_dirty → git2::Repository::open → git_repository_open_ext → ... → open()`。
那张栈是**用外部 `sample` 附进程**才拿到的——服务自己一个字都没说。而 `process_group_id` 为空
正是「按进程组杀不到它」的原因，这一项今天就存在、只是没人看。

要求：系统节点在关键步骤边界留痕（至少「进行到哪一步」）。
**留痕本身不能再挂住关键路径**——`crates/core/src/git.rs` 的 `blocking_within` 已经为此落了
兜底上限，新的留痕机制不得绕过它。

## 4.2 对讲台失败回合留痕

**实证（2026-09-17）**：会话 `监测01M2QH0DHKGSGNVHC0WT2Q4CG0…` 里只有两条 `user` 行，
`prompt_tokens` 与 `completion_tokens` 全 0，`briefing_json` 与 `traces_json` 全空——
**值班长两次没回话，库里没有任何错误痕迹**，连「为什么没回话」都查不到。
形状的成因是 `say()` 先把用户消息落库再调 LLM（`foreman.rs:792-795`），失败就留下孤立用户行。

用户**已经试过**「让值班长盯着这个任务」——这正是本批诉求的第一条在真实环境里的样子。

要求：失败时落一条 `role = system` 的账，写一句人话原因，复用决策 207 已经造好的
「操作台记账」那条路（`role = system` 在模型看到的对话里转写成 `user` + `【操作台】` 标记）。

**Blocked by:** None

**Status:** done

- [x] 系统节点在步骤边界留痕，且留痕开销有界（不得引入新的无界阻塞点）
- [x] 系统节点的 run 行补一项「进行到哪一步」，使「超时」不再是唯一信息
- [x] 对讲台回合失败时落 `role = system` 行，含人话原因（区分网络 / 配置 / 模型 / 内部）
- [x] **失败原因要能归因**，不许只写「失败」——参考既有 `LlmClassified` 的 `kind` 设计
      （`crates/core/src/agent/error.rs`）：宁可退回原始串，不误标类别
- [x] 新增用例（系统节点）：造一个卡住的系统节点，断言台账里有「当时在哪一步」
- [x] 新增用例（对讲台）：注入一个必然失败的 LLM 调用，断言会话里出现一条 `system` 行、
      且它带得出原因；`user` 行不再孤立
- [x] 前端：失败回合渲染成红轮时，把那句原因显示出来（现在只有红轮，无原因）

**实施收尾（2026-09-18）:**

- **4.1 落成一列 `kanban_node_runs.step`**（迁移 0018），值只有 node 自己写：init 三步
  （检查脏工作区 / 创建 worktree / 写回任务行）、done 两步、merge 阶段 A 三步（base_ref /
  rebase / 闸门）、阶段 B 三步、`run_code_gate` 的 lint 与测试各一步。它是**最后到达的那一步**，
  成功也留着——「它做到了哪一步」在成功路径上同样是证据。
- **超时那句话里带上它**（`scheduler::timeout_detail`）：run 行的 `error` 与 pending 的
  `message` 都从「init.execute 超时（attempt 3）」变成「…，当时在「检查项目工作区是否脏」」。
  这才是那次四小时挂死的信息缺口——当时要知道卡在哪个系统调用只能拿外部 `sample` 附进程。
- **留痕是 best-effort**（`Executor::mark_step` 只 warn 不 `?`）：`Git::is_dirty` 那个只值
  一条警告的检查把任务挂死四小时（决策 209）是这个设计的前车之鉴，留痕本身更不能挂住关键路径。
- **4.2 的外框与票 01 同形**：`say` 拆成外框 + `respond`，失败时落一条 `system` 账
  （复用 `NewForemanMessage::system`，即决策 207 的「操作台记账」那条路）。留痕失败只记 error 日志。
- **两条「没有回话」改用 `LlmClassified` 而不是新造错误**：`model_empty_reply`（模型返回空内容）
  与 `model_no_reply`（轮数耗尽）。它们**是模型行为**，而类别正是排查的入口；`turn_failure_reason`
  只做「确证才标类别」的映射（Llm → `llm_network`、Config → `config`、Db → `db`、其余 → `internal`），
  **不做文案嗅探**——按 message 猜类别正是这套 `kind` 设计要挡的东西。
- **既有断言按票面要求先改**：`say_persists_the_user_message_even_when_the_model_fails` 原来钉的是
  「只有用户那一行」（messages.len() == 1），正是本票要改掉的口径；改成 2 行，并断言失败那行的
  标记与归因类别。
- **前端**：`role = system` 的消息若以 `【没跑起来】` 开头，渲染成 `failed` 轮（红、名牌「发送失败」）
  而不是中性的操作台轮——后端的账一直是有的，页面上此前只有一条红轮、原因无处可看。
  该行为的追溯链落在 `design/frontend-design.md` §12.3 新增一行。
- **§12.3 的表加了行**（票面未点名，但决策 199 约束着界面层行为），并新增常量
  `FOREMAN_FAILED_TURN_MARK`——前端那份是镜像，语义源在 foreman.rs。

## 备注

两处合并成一票，是因为它们是同一个毛病的两个面：**出事了，而账上看不出来**。
它也是票 05 事件清单能成立的前提——「调度器处置未生效」那一类之所以能判，靠的就是
「run 是终态／游标仍 active」这些**本来就该写下来的事实**。

## 闸门跑通后补记（2026-09-18）

**失败那一行此前把人话丢了**：`turn_failure_reason` 对 `LlmClassified` 取的是 `raw`
（「模型返回空内容（无 tool_calls、无文本）」），于是台账上只有技术串，票面要的「人话原因」
被扔掉了。改成取 `message`（人话那一段）并把 `raw` 括在后面——值班经理读得懂，排查时也搜得到
供应商原文。用例 `a_silent_model_is_recorded_with_its_own_kind` 钉的就是这一条。

**前端会多出一轮重复的失败**：后端落账之后，一次失败在时间线上有两轮——本地那条传输报文
（`发送失败：provider ... 401`，带配对入口）与台账那一条（带归因）。判据收在
`realtime/foreman.ts::ledgerOwnsTheFailure`：重取台账成功且**新出现**一条带标记的 system 行时，
撤掉本地的 error（台账那行更全、刷新后还在）。只认「新出现」是必要的——不然一次早先的失败会让
此后每一次真实断网（请求根本没到后端，台账不会多行）都静默，而那正是本地那行存在的理由。
这条是被 `frontend/e2e/talk.spec.ts` 那条「发送失败」用例挡下来的：它断言时间线里**恰好一轮**
失败，两轮时 `strict mode violation`。
