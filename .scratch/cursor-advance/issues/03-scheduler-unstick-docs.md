# 03: scheduler 的 pending 支与 unstick 的 DB 部分接上新门

**What to build:** 把剩下两处调用方接上 `advance`，并收尾文档。**只接真正动 DB 的那一步**——
两处的进程级编排都留在自己家里。

**Blocked by:** 02

**Status: done（已实现）**

## 一、一致规矩（本票的判据）

`handle_timeout` 与 `unstick` 都是「进程内存/重派 + 几步 DB」的混合体。已定的规矩是：
**进程内存 / 重派不进事务模块**，只有动 DB 的那一步进。这不是为省事——`unstick` 的第一步
`force_release` 是**不可回滚的内存操作**，硬塞进纯 DB module 会把它变成要接 `&dyn Fn` 闭包的
module（`unstick` 签名里那个突兀的 `+ Sync` 闭包参数正是这个形状的后果）。

## 二、`handle_timeout`（`scheduler/mod.rs:330-363`）

**重试支不进**（`:331-345`）：它的 `insert_transition` 的 from 与 to **是同一个**
`(run.stage, run.node)`，游标不动、`agent_retry_max` 与 `cursor.validate_attempts` 是两个独立
计数器（注释明写「不计 validate_attempts」），且它带一个进程级重派 `(self.resume)(task_id)`。
**那不是推进**——`advance` 的语义是「把游标搬到新落点」，这条不搬。保留现状。

**pending 支进**（`:346-362`）：耗尽后挂 `Pending(Timeout)` 那一段改调
`advance::advance(..., Landing::Pause{..})`，消掉手抄的 `set_cursor_pending` +
`sync_task_projection` 两连（`emit_pending` 保留在调用方，因为门不发 SSE）。

**注意那条承重顺序**（`scheduler/mod.rs:315-319` 的注释）：`request_cancel` 必须在 `finish_run`
之后——收口是异步的，执行体收口时会 `record_run_usage` 把已烧的 token 补进这一行，而
`finish_run` 是按值整段写入（`RunOutcome::default()` 的 token 是 0）。**本票不动这条顺序**，
但改动落在同一段里，落地时不要顺手挪它。

## 三、`unstick`（`pipeline/unstick.rs:126-201`）

**编排不进、DB 部分进**。三段编排（`force_release` → 标僵死 run 终态 → 清 owner + 转 pending）
保留在 `unstick` 自己那里；其中第三步那两连（`set_cursor_pending` + `sync_task_projection`，
`:190-193`）改调 `advance::advance(..., Landing::Pause{..})`。

**这里今天没有 SSE**（`unstick` 只写库不发事件），与 resume 那条是同一种静默。**本票不扩大
范围**：`unstick` 的调用方是值班长的动作面，补不补 SSE 是**另一件事**（见「明确不做」）。
落地时在 `unstick` 里留一句注释说明「这里不发 SSE，与 resume 那条的处置不同，原因是……」，
免得下一个读者以为是漏了。

## 四、收尾文档

- `docs/implementation.md:177`（「没有任何其他代码路径写这个状态」）与
  `docs/testing.md:136`（「唯一写入路径 = `advance_cursor`」）若票 02 未改，本票改完——
  两处都是**断言式**，重构后立刻变假。
- 复核 `docs/pipeline-spec.md:47/56/219/223`：四处是叙述（「`advance_cursor` 把它置为
  waiting_join」），**不影响正确性**。本票**只做一件事**：在这四处或附近加一句指向
  `pipeline/advance.rs` 的说明，**不改写叙述**。整段梳理留待单独一轮。

## 明确不做（记在案，防止下一轮重提）

- **`unstick` 不补 SSE**——它的调用方是值班长动作面，事件面怎么设计是另一件事。
- **`ExecutionState` 不补**——`glossary.md:16` 把它定义为头等概念而代码里一处都没有
  （散落的 `Vec<NodeCursor>` 局部量）。补出真类型会顺带给 `pipeline/cursor.rs` 那八个单行谓词
  （`live_cursors` / `runnable_cursors` / …）一个家，那是一个**独立候选**。
- **文件落盘顺序保持现状**——`retry-feedback.md` / `backtrack-feedback.md` / `user-input.md`
  今天靠「先写文件再动游标」的注释维持（`decisions.rs:370-382`、`executor.rs:2459-2472`）。
  不把 `fs` 纳进门（同「SSE 推出门」的理由），只如实记下这条依赖。
- **不删 `petgraph` 与 `pipeline/graph.rs`**——那是评审的候选 3（生产零调用方的死镜像），
  另一张票。

## 验收

- [ ] `handle_timeout` 的 pending 支改调 `advance`；**重试支一字未动**
- [ ] `scheduler_tick.rs` 的超时用例全绿（`:309` 那条同时覆盖两支，是本票的主要回归网）
- [ ] `unstick` 的三段编排仍在原处；第三步改调 `advance`；有一条注释说明为何不发 SSE
- [ ] `unstick` 的两条既有用例全绿（`executor.rs:4048` 与 `:4191`）+
      `api_contract.rs:4480` 的托管 unstick 全绿
- [ ] `implementation.md:177` 与 `testing.md:136` 已改；`pipeline-spec.md` 四处加了指向新 module 的说明
- [ ] 「明确不做」四项已写进决策 245 的「明确不做」段（与票 02 合并成同一条）
