# 05: 待办补两类（`run_failed` / `task_cancelled`）

**What to build:** `AttentionKind` 补 `run_failed`（一次 run 落 `failed` / `timeout` **且任务没有
因此转 pending**）与 `task_cancelled`（取消路径补写关注项）；**两类都唤醒**（`wakes() = true`），
都吃既有的三重节流（60s 去抖 + 同任务 30min 冷却 + 全局 12 次/小时）（决策 234）。

**两类都是实测撞出来的「监控对象出事了、值守不知情」**：一条任务的三次 `architect-design.execute`
全失败、合计烧掉 1,050 万 prompt token，**值守轮一次都没醒**（`scheduler_no_effect` 与
`task_pending` 两条既有判据都不成立）；三条任务被标 cancelled 之后，值守班次从 01:25 起再没被叫醒过。

**Blocked by:** None（最便宜的一条，且立刻止住「烧掉一千万 token 而值守不知情」）

**Status:** pending

- [ ] 两个枚举值 + `as_str` / `parse` 两条臂（`wakes()` 缺省为真，两条都自动唤醒）
- [ ] `run_failed` 的生产点：`note_discoveries` 里扫「终态失败 / 超时的 run」并带
      **「任务没转 pending」**那一半判据（否则重试型故障会连着出几条）
- [ ] `task_cancelled` 的生产点：取消路径（`cancel_task`）补写
- [ ] 用例：两类各自被记下并被唤醒；**同任务 30 分钟冷却对 `run_failed` 真的生效**
      （风险明文在决策 234：不然一次重试型故障能烧光每小时配额）；任务自己转 pending 时不重复记

**Notes（实现提示）:**
- 表上 `kind` 没有 CHECK 约束，故加类别不需要迁移。
- `occurred_at` 取事件发生时刻（run 的 `finished_at` / 取消那一刻），不取「这一 tick 的时刻」。
