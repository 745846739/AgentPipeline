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

**Status:** ready-for-agent

- [ ] `Runtime::force_release(task_id)`：从进程内去重集合里摘除，且**摘除后可再次 `try_run`**
- [ ] `unstick` 动作：清 owner + 僵死 run 标终态（原因可读）+ 游标转 `pending`（带原因）
- [ ] `unstick` 的判据：只对「run 已终态而游标仍 active / owner 持有超时」这类任务生效，
      不对正常 `running` 的任务生效（否则它会变成「随便踢一脚」）
- [ ] `unstick` 走 D 层通道，进托管可自动集；`config` 一类的其它动作不变
- [ ] 「重启服务」做成**提议**（复用决策 207 的提议表与确认钮），永不自动，
      且提议文案要说清「会打断 N 个在跑的任务」
- [ ] 新增用例：造一个进程内去重被占用的任务，`unstick` 后**真的能重跑**
      （打在新 run 行出现上，不是打在「owner 列为空」上——后者清个 DB 字段就能满足，
      而那正是这条要防的假绿）
- [ ] 新增用例：正常 `running` 的任务不能被 `unstick`
- [ ] 新增用例：重启提议永远是 `Propose`，无论托管与否

## 备注

票 05 把这类任务**报出来**，票 09 让它**能被解开**。两票缺一，症状就还是「卡住且只能重启」——
区别只是现在有人会告诉你它卡住了。
