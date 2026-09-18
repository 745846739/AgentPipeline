# 09: 修补动作面——`unstick` 与「重启服务」提议

**What to build:** 两个新动作，一个能自动、一个只能提议。

## 9.1 `unstick`（进自动动作集）

**`unstick` 与 `resume` 是两回事。** `unstick` = 清 `executor_owner` + 把僵死的 run 标终态 +
游标转 `pending`（带原因）。

它必须配一个 `Runtime::force_release(task_id)`——把 id 从进程内去重
`Mutex<HashSet<task_id>>`（`crates/app/src/runtime.rs:10`）里**摘掉**。否则**清了 DB 也没用**。

**实证（2026-09-17）**：`支持rtk` 的 run 被标 `timeout` 后，超时处理写了「干净对话重试」的
transition，但**重试从未发生**——原始 `try_run` 没返回、进程内去重仍持有该 task_id，
重试被逐次拒掉（那条 25 次退避重试循环存在的理由，`runtime.rs:63-90`）。
**`resume` 在这种情况下是空操作，还会白吃票 08 的 N 次配额。**

`unstick` 进自动动作集：只影响一个任务、可逆、且是 `resume` 能生效的前提。

## 9.2 「重启服务」只提议、永不自动

它是**全局**动作，会打断所有在跑的任务。它落在决策 206 定的「全局改动单独提议、人按」那条规则里，
而且它是**只重启、不改代码**——与「不许热修本仓源码」不是一回事。**这一句必须写进决策，
否则规则有歧义。**

重启本身是安全的：决策 127 的 `clear_executor_owners` + `requeue_running_tasks`
（`crates/app/src/serve.rs:319-329`）会把中断的 running 任务归队。

**Blocked by:** 08

**Status:** done

- [x] `Runtime::force_release(task_id)`：从进程内去重集合里摘除，且**摘除后可再次 `try_run`**
- [x] `unstick` 动作：清 owner + 僵死 run 标终态（原因可读）+ 游标转 `pending`（带原因）
- [x] `unstick` 的判据：只对「run 已终态而游标仍 active / owner 持有超时」这类任务生效，
      不对正常 `running` 的任务生效（否则它会变成「随便踢一脚」）
- [x] `unstick` 走 D 层通道，进托管可自动集；`config` 一类的其它动作不变
- [x] 「重启服务」做成**提议**（复用决策 207 的提议表与确认钮），永不自动，
      且提议文案要说清「会打断 N 个在跑的任务」
- [x] 新增用例：造一个进程内去重被占用的任务，`unstick` 后**真的能重跑**
      （打在新 run 行出现上，不是打在「owner 列为空」上——后者清个 DB 字段就能满足，
      而那正是这条要防的假绿）
- [x] 新增用例：正常 `running` 的任务不能被 `unstick`
- [x] 新增用例：重启提议永远是 `Propose`，无论托管与否

**实施收尾（2026-09-18）:**

- **`force_release` 落在 core**：进程内去重集合（`EXECUTOR_REGISTRY`）住在
  `pipeline::executor`，与 `try_run` 同一处。app 层的 `runtime::force_release` 是一个**有名字的
  入口**（票面点的是 `Runtime::force_release`），实现只有一行转发——两个入口、一处实现。
  **代价写进了注释**：摘掉之后那个仍卡着的旧执行体若哪天活过来，可能与新执行体同时写库；
  这不是新增风险面，而是原本那个僵死状态本来就有的。
- **判据只有一处**：`pipeline::unstick::stuck_evidence`，**票 05 的待办扫描也改用它**。
  两处各写一份的后果是「报出来的卡住」与「解得开的卡住」成为两个集合，而「它说卡了、
  我却解不开」正是最让人不信任这套东西的一类现象。
- **`unstick` 挂在 `task` 工具上**（`action=unstick`，决策 207④ 的一族一个工具），
  pending 用 `UserDecision` + context kind `unstick`（不是新造 PendingKind：它要动前端与
  动作集两份公开契约，而这里的语义确实是「解开了，等你决定下一步」）。
- **「重启服务」做成第四个 D 层工具 `service`**（`action=restart`）。提议文案由系统生成，
  明写「**会打断所有在跑的任务**」。用例正反面都打：托管开着也仍是提议。
- **一处如实偏离票面**：按下 `restart` 之后做的是**恢复序列**（决策 127 的两步：清
  `executor_owner` + 中断的 running 任务归队 + 顺手收中断的项目级 run），然后**明说本进程
  没有自重启能力、请在你启动它的地方重启一次**。票面写的是「只重启、不改代码」，但本仓
  没有进程外的监督者契约——擅自 `exit` 会让服务就此消失，而按下那颗钮的人未必在能把它
  拉起来的地方。**真重启需要一条 supervisor，那是另一票**；这一处不能算「做完了」。

## 备注

票 05 把这类任务**报出来**，票 09 让它**能被解开**。两票缺一，症状就还是「卡住且只能重启」——
区别只是现在有人会告诉你它卡住了。
