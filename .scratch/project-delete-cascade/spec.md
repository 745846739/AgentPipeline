# 删除项目报 787：级联删除项目历史（project-delete-cascade）

**Status:** done（已实现，决策 328）

> **来源**：2026-09-29 用户报障——「本服务删除项目报错 数据库错误：error returned
> from database: (code: 787) FOREIGN KEY constraint failed」。诊断找出两处根因，
> 三条候选语义（真删 + 级联 / 保持数据但回一条说得清的 409 / 软删除）里用户拍板
> **第一条**。决策落 `docs/decisions.md` 328。

## Problem Statement

删除一个用过的项目，界面弹的是底层 SQL 报错（`code: 787`，并被
`Error::Db` 原样透成「数据库错误：…」）。项目用得越久越必然踩到——分析行是
界面上一个正常动作、行从不清理，而它外键挂在项目行上。

就算绕开这层，删除路径还有第二处洞：删掉父行之后**任务不会报错、只会变成孤儿**
继续留在看板上（任务列表是单表取的，不 join 项目），连同它的整棵子树悬空。
换句话说，「删除项目」这个动作从来没被真正实现过——只删了一行不挂外键的表。

## Solution

`Store::delete_project` 改为**一个事务里的显式级联**：按外键方向先子后父，把项目级
三张子表（`kanban_project_analyses` / `kanban_node_runs` / `kanban_node_conversations`）
与该项目任务及其全部行（cursors / runs / commands / conversations / transitions /
deps / stage_outputs / foreman_attention / model_requests）一次删净，最后删项目行。

两处需要额外一步：`kanban_node_runs.parent_run_id` 是自引用，同一批删除里父行可能
先走，故先 `UPDATE ... SET parent_run_id = NULL` 断链；`runs.cursor_id` 指向
`kanban_node_cursors`，故 runs 必须在 cursors 之前走完。`kanban_tasks` 的行活到
倒数第二步，前面各步的子查询才有对象。

**不采用 `ON DELETE CASCADE`**：改外键要重建三张子表（SQLite 不能 ALTER 外键），而
`kanban_tasks` 那半个「项目 → 任务」根本没有外键可用——显式删除是唯一能一次覆盖
两半的写法，也才有地方写上面那两步顺序修正。

`delete_project` 返回被删任务的 id；磁盘产物（worktree / 分支 / 任务目录）由 app 层
按约定路径回收（任务行此刻已删，读不回来了）。storage 不碰文件系统——与决策 3 /
§12.1 的取消清理同一分工。库先落定、磁盘尽力而为，失败只告警不回滚。

决策 101 的活跃任务闸门原样保留，它是级联删除的**联锁**：有活跃任务时整件事不发生，
而不是「删一半」。

## User Stories

1. As a 用完了一个项目的用户, I want 删除它时不要看到 SQL 报错, so that 我能收拾掉
   不再需要的项目而不是被一句 `code: 787` 挡住。
2. As a 用户, I want 删除项目时它的任务一起消失, so that 看板上不留一堆点进去没有
   项目的孤儿卡片。
3. As a 用户, I want 项目有活跃任务时仍然被拒绝, so that 我不会误删一条正在跑的流水线。

## 验收判据

- 用过的项目（有分析行、项目级 run / 会话、多个「跑过一轮」的任务）能一次删净；
  删完之后逐表扫，13 张挂 `project_id` / `task_id` 的表零行。
- 不留孤儿任务（`kanban_tasks.project_id` 指向已删项目的行数为零）。
- 边界：别的项目一行不动；跨项目指向本项目的依赖边清掉，别项目自己的边留着。
- 活跃任务在时返回 `Conflict` 且**一行都不少**；任务收工后同一份数据一次删净。
- 反向验过：把实现换回裸删，上述 L2 四条全红。

## 落地位置

- `crates/core/src/storage/catalog.rs`——级联删除
- `crates/app/src/routes/projects.rs`——磁盘产物回收
- `crates/core/tests/integration/project_delete.rs`——L2 四条
- `crates/app/tests/integration/api_contract.rs`——L3 契约扩写

## 一处够不着的残留（**已做**：票 02 / 决策 329）

项目分析的模型请求台账行（每次分析 4 行，`agent_type = pseudo:project_analysis`）三个
归属键全空，级联够不着。根因与「为什么没顺手改」写在
`issues/01-cascade-delete.md` 末节——它要动决策 249 冻结的 `Executor::project_analysis`
签名，按那条决策要单开票——即本目录的 `issues/02`，用户拍板后已落地（决策 329）：`run_id` 透进 `RunContext`，那几行从此挂得上项目级 run，删项目时由决策 328 的 `RUN` 谓词一并带走。
