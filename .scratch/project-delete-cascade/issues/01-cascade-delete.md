# 01: 删除项目 = 连它的全部历史一起删

**What to build:** 决策 328——① `Store::delete_project` 改成事务内的显式级联
（先子后父；`parent_run_id` 先断链；runs 先于 cursors；返回被删任务 id）；
② app 层 `recycle_task_artifacts` 回收 worktree / 分支 / 任务目录（按
`Home::worktree_path` / `Home::task_dir` 的约定路径，库先落定、磁盘尽力而为）；
③ 决策 101 的活跃任务闸门原样保留为联锁。

**Blocked by:** None

**Status:** done（已实现，决策 328）

- [x] `delete_project`：`begin_write()`（`BEGIN IMMEDIATE`）→ **事务内**查活跃任务（有则 `Conflict`，未写一行故 drop 即回滚）→ 取任务 id → 按序删除
      （commands / conversations / model_requests / foreman_attention /
      stage_outputs / transitions / task_deps → `parent_run_id` 置空 →
      runs → cursors → project_analyses → tasks → project）→ 返回任务 id
- [x] 归谓语用子查询（`task_id IN (SELECT id FROM kanban_tasks WHERE project_id = ?)`）
      而不是把 id 拼进 SQL；`kanban_tasks` 的行留到倒数第二步，子查询全程有效
- [x] 依赖边两个方向都清（`task_id` 与 `depends_on_id`）——跨项目指向本项目的边也是死边
- [x] app 层产物回收：worktree（含 git 登记）+ 分支 + 任务目录，失败只 `warn`
- [x] L2 `project_delete` 4 条：删净（13 张表逐表零行）/ 无孤儿任务 / 边界（别项目
      不动 + 跨项目边清理）/ 活跃任务拒绝且一行不少
- [x] L3 契约扩写：删前留一条分析行（787 的最后一道拦路者）+ 一份磁盘现场（真 worktree + 任务目录），删后 `/projects` 查不到、worktree / 分支 / 任务目录三者皆回收
- [x] 反向验过：换回裸删，四条 L2 全红（三条 787、一条孤儿 1 行）

**注记（留给后来者）**：

- 线上库那次 787 的最后一道拦路者是 `kanban_project_analyses`——逐表缩小后
  「清掉 runs + conversations 仍失败，再清 analyses 才过」。分析行没有保留期，
  故这个洞在正常使用下必然踩到。
- 将来新增一张挂 `project_id` / `task_id` 的表，**必须同时**加进删除路径与
  `project_delete.rs` 的两张表清单（`PROJECT_SCOPED` / `TASK_SCOPED`）——
  「哪张表忘了清」正是这个 bug 的形状。

## 查出来、当时没做的邻接缺陷（**已由票 02 / 决策 329 做掉**）

真机验证（线上库副本走 `Store::delete_project`）时量到：每次 `POST /projects/analyze`
会在 `kanban_model_requests` 留 **4 行无归属**台账——`run_id` / `session_id` / `task_id`
**三个键全空**，`agent_type = pseudo:project_analysis`。删项目时它们既不在 `project_id`
也不在 `task_id` 键上，任何级联都够不着，只能一直躺着。

根因：`model_invoke.rs::project_analysis` 组装 `LlmRequest` 时把 `run_id` 填成 `0`
（`recording.rs` 里那个「没有 run 行」的哨兵，注释写「值班长没有 run 行…**项目分析也没有**」），
而这句话自决策 100 / 迁移 0004 起就**过时了**——项目级 run 现在有真行（`insert_project_run`），
路由在调 `project_analysis` 之前就先落了它。

**为什么没顺手改**：`Executor::project_analysis` 是决策 249 明确冻结的**外部 6 触点之一**
（「`new` 五参、`try_run`、`project_analysis`、`force_release`、`request_cancel`、类型导出——
**要改语义单开票，不顺手改**」）。修法本身很小（把路由已经拿到的 `run_id` 透进 `RunContext`，
那 4 行从此挂在 run 上、既能被本项目级联带走、也才进得了决策 231 的「在飞请求」台账与
`model_requests_for_run` 读数），但改的是冻结签名，故按那条决策另开票——**那张票就是本目录的 02，用户拍板「现在顺手改」后已落地（决策 329）**。下面这段保留原文，记的是当时为什么不能顺手。

**判据（留给那张票）**：`POST /projects/analyze` 之后，`model_requests_for_run(<项目级 run>)`
能读到这次分析的请求行；删项目时那几行随项目一起消失。
