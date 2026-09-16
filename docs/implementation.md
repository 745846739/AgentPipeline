# 实现架构

> 拆分自 agent-pipeline.md（原 §11）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

## 11. 实现架构

### 11.1 整体架构

```
┌──────────────────────────────────────────────────────┐
│                  petgraph DAG（状态流转）               │
│                                                      │
│  validate_input → execute → validate_output          │
│         ↑                     │                      │
│         └─────────────────────┘  (条件边重试)         │
│                                                      │
│  interrupt_before 每个节点                             │
│  ↓                                                    │
│  检查 DB: task.pending_reason 是否非空                 │
│  非空 → 暂停（等 resume）                              │
│  空   → 继续执行                                      │
└──────────────────────┬───────────────────────────────┘
                       │ checkpoint (SQLite)
                       │
┌──────────────────────┴───────────────────────────────┐
│              KanbanScheduler（独立调度器）              │
│                                                      │
│  每 10s tick:                                         │
│  - 超时检测     → 设置 pending → resume               │
│  - 冲突等待恢复 → resume                              │
│                                                      │
│  通过 resume() 接口驱动图继续                           │
│  - 并发准入（max_concurrent_tasks）                     │
│  - 依赖启动 / pending 提醒 / stalled 标记               │
└──────────────────────┬───────────────────────────────┘
                       │
┌──────────────────────┴───────────────────────────────┐
│                 HTTP API（axum）                       │
│                                                      │
│  POST /tasks                → 建 Task + main 游标，进入 queued     │
│  GET  /tasks                → 任务列表（看板数据源）                │
│  POST /tasks/{id}/resume    → 恢复动作（continue/skip/goto）       │
│  GET  /tasks/{id}           → 当前状态 + allowed_actions           │
│  GET  /tasks/{id}/stream    → 唯一 SSE 事件流                      │
└──────────────────────────────────────────────────────┘
```

> **单执行者保证（决策 36）：** 用户 resume 与 scheduler tick 都可能触发 `run_executor`，因此每个任务同一时刻只允许一个 executor 运行——进程内用 `Mutex<HashSet<task_id>>` 去重，DB 层用乐观锁（`UPDATE ... WHERE executor_owner IS NULL`）兜底跨进程场景。

### 11.2 状态流转 → petgraph DAG + 自定义 executor

使用 petgraph 构建 DAG，自定义 executor 遍历图并执行节点：

```rust
// crates/core/src/pipeline/graph.rs

use petgraph::graph::DiGraph;

pub fn build_pipeline_graph() -> DiGraph<String, &str> {
    let mut graph = DiGraph::new();

    // 添加节点：每个阶段的 3 个节点
    let stages = ["init", "architect-design", "develop-design", "test-design",
                   "sync-check", "develop", "review", "test", "merge", "done"];
    let nodes = ["validate_input", "execute", "validate_output"];

    // ... 添加节点和边 ...

    // 条件边通过边权重标记：
    // "next" → 进入下一阶段
    // "retry" → 重试 execute
    // "pending" → 进入 pending 状态
    // "backtrack" → 回退到 architect-design

    graph
}
```

**executor 核心循环：**

```rust
// crates/core/src/pipeline/executor.rs

pub async fn run_executor(task_id: &str, db: &SqlitePool) -> Result<()> {
    // 单执行者保证（决策 36）：同一任务同时只允许一个 executor
    let _guard = match TaskExecutorRegistry::acquire(task_id) {
        Some(guard) => guard,
        None => return Ok(()),   // 已有 executor 在跑，直接返回
    };
    // DB 乐观锁兜底：executor_owner 已被占用则跳过
    if !db.try_claim_executor(task_id).await? {
        return Ok(());
    }

    let result = run_executor_inner(task_id, db).await;
    db.release_executor(task_id).await?;
    result
}

async fn run_executor_inner(task_id: &str, db: &SqlitePool) -> Result<()> {
    loop {
        // 1. 加载全部活跃游标（决策 80；status != 'archived'，历史行不加载——决策 113）
        //    串行阶段只有 1 个，并行阶段有 2 个
        let cursors = load_cursors(db, task_id).await?;

        // 2. 分别挑出三类游标（决策 82 / 89）
        //    - pending：自己阻塞，**移出可运行集合**，但不影响其他分支
        //    - active：可继续执行
        //    - waiting_join：已到 join 边界，等其余分支
        //    注意：pending 不等于"任务暂停"。只有当没有任何可推进动作时 executor 才退出。
        let pending: Vec<_> = cursors.iter().filter(|c| c.status == CursorStatus::Pending).collect();
        let runnable: Vec<_> = cursors.iter().filter(|c| c.status == CursorStatus::Active).collect();

        // 通知前端哪些分支被阻塞（幂等推送，不改变执行流）
        for p in &pending {
            push_sse(task_id, SseEvent::Pending(p.pending_reason.clone().unwrap())).await;
        }

        // 3. 没有可运行的游标时：
        //    (a) 若还有 pending → 任务整体暂停，等 resume（决策 82）
        //    (b) 若全是 waiting_join → 由 join 屏障统一推进一次（决策 107）
        if runnable.is_empty() {
            if !pending.is_empty() {
                return Ok(()); // 暂停，等 POST /tasks/{id}/resume
            }
            if !advance_join(task_id, &cursors, db).await? {
                return Ok(()); // join 条件未满足（理论上不会走到），防御性退出
            }
            continue;
        }

        // 4. 并发驱动所有可继续游标（决策 81）：单游标退化成一次普通调用，
        //    并行阶段两个分支各跑各的节点，互不阻塞。pending 游标不在集合内，
        //    因此一条分支被阻塞时另一条照常推进——这正是决策 82 的语义。
        let results = futures::stream::iter(runnable.iter())
            .map(|c| async { (c, execute_node(task_id, c, db).await) })
            .buffer_unordered(runnable.len())
            .collect::<Vec<_>>()
            .await;

        // 5. 逐游标更新：validate_attempts / pending 都记在各自游标上（决策 82）。
        //    单条游标的节点失败**不得向上传播**中止整个 executor（决策 89）——
        //    它只把该游标置为 pending，另一分支必须能继续跑完本阶段。
        for (cursor, result) in results {
            match result {
                Ok(node_result) => {
                    // 路由到 next / retry / pending / waiting_join，不改动其他游标
                    advance_cursor(db, cursor, node_result, &graph()).await?;
                }
                Err(node_error) => {
                    // 节点级重试已耗尽（agent_retry_max）→ 只阻塞这一条游标
                    mark_cursor_pending(db, cursor, node_error.into_pending_reason()).await?;
                }
            }
        }

        // 6. 终态判定：所有游标均已抵达 done
        if is_terminal(db, task_id).await? {
            break;
        }
    }
    Ok(())
}
```

**并行执行语义（决策 80–84 / 89 / 107）：**

| 环节 | 处理 |
|---|---|
| 执行状态 | `kanban_node_cursors` 是唯一事实来源（决策 80）。`kanban_tasks` 的 `current_stage` / `current_node` / `validate_attempts` 退化为**焦点游标投影**，只供看板展示与筛选 |
| executor 数量 | 仍是**每任务一个** executor（决策 81，沿用决策 36 的进程内 Mutex + DB `executor_owner` 乐观锁），内部并发驱动多个游标。不引入 per-branch executor |
| pending 的处理 | pending 游标被**移出可运行集合**，不是让整个 executor 退出（决策 89）。只有当"没有可运行游标且有 pending"时才暂停等 resume |
| **单游标失败不传播** | 某游标的节点执行失败只把该游标置为 pending，**不得** `return Err` 中止整个 executor——否则另一分支会被连带中断，违反决策 82（决策 89） |
| attempts 计数 | `validate_attempts` **每游标独立**；`validate_retry_max` 按游标判定（决策 82） |
| pending 归属 | pending 挂在**游标**上（`kanban_node_cursors.pending_reason_json`）；任务整体 `status = pending` 是"任一游标被阻塞"的投影（决策 82） |
| 分支失败隔离 | 一个分支 `retry_exhausted` / 超时进 pending 时，另一分支**不被打断**，跑完当前阶段后停在 join 边界（决策 82） |
| **`waiting_join` 的写入** | 由 `advance_cursor` 写入：`next` 路由发现下一阶段是 join 节点时，把该游标置为 `waiting_join` 而非继续（决策 107）。没有任何其他代码路径写这个状态 |
| join 条件 | 所有分支游标都到达 join 边界（`waiting_join`）且均无 pending，汇聚节点才执行一次（决策 83） |
| join 的游标归属 | 汇聚节点 `sync-check` **不占游标行**（决策 107）；它落库为一次 run（`agent_type="system"`，`cursor_id` = 同事务内新建的 main 游标，决策 113），推进时在一个事务内归档分支游标并插入单条 main |
| 焦点游标 | 取 `updated_at` 最新的 pending 游标，无 pending 时取 `updated_at` 最新的活跃游标；串行时即 main（决策 92 / 130） |
| SSE | 事件体带 `branch` 字段用于消歧；时间线记录 `branch`（决策 84） |

> **兼容性：** 串行阶段（init / architect-design / sync-check / develop / review / test / merge / done）只有 `branch = "main"` 的单个游标，`task.current_stage` / `task.validate_attempts` 与游标始终一致，因此 §11.3 的 resume 逻辑、§11.4 的超时检测、§12.4 的各处展示在串行场景下行为不变。理解并行场景时，把下文出现的 `task.validate_attempts` 读作"该游标的 `validate_attempts`"。

**调度器谓词（决策 92 / 117）：** scheduler 的 `remind_pending_tasks` / stalled 标记**不得只看 `task.status`**——一个分支 pending、另一分支仍在跑的任务，其 `status = pending` 但并未卡住。提醒谓词判 `has_runnable_cursor(task)`：存在 `active` 游标即视为在推进，只有"无 active 且无 `waiting_join` 可推进"才算停滞。并发准入的**名额占用**另按决策 117 计数（`status ∈ {running, pending}`）——前者判"是否卡住"，后者判"是否占名额"，两者不冲突。

**路由函数集中定义（决策 39）：** 所有条件边路由放在 `crates/core/src/pipeline/routes.rs`，key 为 `(stage, node)`，输入是节点元数据 + 任务计数，输出 `EdgeKind`。路由条件混合了代码判断、计数判断和 LLM 元数据判断，故用显式 Rust 函数而非声明式表达式语言。

```rust
// crates/core/src/pipeline/routes.rs

// 路由输入是「游标」而不是任务（决策 80）：attempts 与 pending 都是游标级的
pub fn route(cursor: &NodeCursor, ctx: &RouteContext) -> EdgeKind {
    match (cursor.stage.as_str(), cursor.node.as_str()) {
        ("develop", "validate_output") | ("test", "validate_output") => route_test_result(ctx),
        ("merge", "execute") => route_merge(ctx),          // 批准 / 测试闸门失败 / 冲突打回（决策 85/86）
        (_, "validate_input") => route_by_readiness(cursor.stage.as_str(), cursor.node.as_str(), ctx.metadata),
        (_, "validate_output") => route_after_validate_output(cursor, ctx),
        _ => EdgeKind::Next,
    }
}

fn route_after_validate_output(cursor: &NodeCursor, ctx: &RouteContext) -> EdgeKind {
    // 按游标自身的 attempts 判定，不受其他分支影响（决策 82）。
    // 注：`cross_family_judge = true` 时，agent 型 validate_output 首判不合格在节点内先经
    // validator_cross_check 复判（决策 134）——复判合格则根本不进入本路由，而是走
    // pending(user_decision, judge_disagreement)（决策 135）；能进到这里说明两侧一致不合格。
    if cursor.validate_attempts >= ctx.settings.validate_retry_max {
        EdgeKind::Pending
    } else {
        EdgeKind::Retry // 重试 execute
    }
}

fn route_by_readiness(stage: &str, node: &str, metadata: &Metadata) -> EdgeKind {
    if metadata.readiness {
        EdgeKind::Next
    } else {
        // pending 的**原因类型由阶段决定**（决策 94）——路由不返回裸 Pending，
        // 而是带上 type，因为 type 决定前端下发哪些 allowed_actions。
        match stage {
            "architect-design" => EdgeKind::Pending(PendingKind::InfoInsufficient),
            // develop-design / test-design 的输入不足需要用户决定回退还是跳过
            "develop-design" | "test-design" => EdgeKind::Pending(PendingKind::UserDecision),
            _ => EdgeKind::Pending(PendingKind::UserDecision),
        }
    }
}

fn route_merge(ctx: &RouteContext) -> EdgeKind {
    // ① 仍在等审批：不推进、不重入（决策 95）。原先的 `=> Next` 会把
    //    未审批的 merge 直接送进 done，是 bug。
    if ctx.merge.approval == Approval::Pending {
        return EdgeKind::NoOp;
    }
    // ② 闸门未跑 ≠ 通过：`Gate` 无 Default（决策 95），缺省视为 NoOp，绝不当作通过
    let Some(gate) = ctx.merge.gate else { return EdgeKind::NoOp; };
    // ③ 决策 85/108：耗尽收口，否则 merge↔test/develop 无限循环
    if gate == Gate::Fail && ctx.merge.gate_failures >= ctx.validate_retry_max {
        return EdgeKind::Pending(PendingKind::RetryExhausted);
    }
    // ④ 闸门失败与审批状态**正交**（决策 95）：先看 gate，再看 approval
    if gate == Gate::Fail {
        // 闸门失败分流（决策 139）：lint 失败是确定性错误，直接打回 develop；
        // 测试失败需要 agent 分辨 test_issue / code_issue，跳回 test.execute（决策 85）
        return match ctx.merge.gate_failure_kind {
            GateFailureKind::Lint => EdgeKind::KickbackDevelop,
            GateFailureKind::Test => EdgeKind::GotoTest,
        };
    }
    match ctx.merge.approval {
        Approval::Approved => EdgeKind::Next,   // → done（阶段 B 已在 execute 内完成合入）
        // None / Returned 均不经过路由（决策 121）：None 的唯一出口是阶段 A 末尾的
        // pending(merge_approval)，已在 execute 内处理；"返回修改"由 merge/decision
        // 端点直接置游标到 develop.execute（决策 119）。原先的 `=> Kickback` 会把
        // 首次进入 merge（approval=none）的任务直接打回 develop，永远生成不了 proposal。
        Approval::None | Approval::Returned => EdgeKind::NoOp,
        Approval::Pending => unreachable!(),                    // 上面已提前返回
    }
}
```

> **`Approval::GateFailed` 已删除（决策 95）：** 原路由里出现过一个 `Approval::GateFailed` 变体，它既不在 `merge_result.approval` 的枚举里，语义上也放错了层——闸门失败是**执行结果**，不是**审批状态**。现在闸门用独立的 `merge_result.gate: "pass" | "fail"` 字段承载，`approval` 只表达"用户批没批"。两者正交，可以同时是"已批准但闸门失败"。

> **`Approval::None | Returned` 不打回（决策 121）：** 路由中曾把这两个状态映射为 `Kickback → develop`，与 §6"none / returned 走阶段 A"直接矛盾——首次进入 merge（approval=none）的任务会被打回 develop，永远生成不了 proposal。修正后二者均为 `NoOp`：打回 develop 的两条真实路径（execute 内冲突打回、decision 端点的 return）都不经过 EdgeKind 路由。

**游标是 checkpoint 的载体（决策 80）：** `kanban_node_cursors` 的各行合起来就是 checkpoint——包括每个分支的 `stage` / `node` / `validate_attempts` / `pending_reason`。重启后 executor 加载这些行即可恢复到每个游标的中断节点，不需要额外的手动状态恢复。

### 11.3 Pending → 暂停 + 事件通知

pending 不是一个"阶段"，而是**中断**。某条游标进入 pending 后：
- 该游标被移出可运行集合（决策 89）；
- **另一分支不被中断**，继续跑完本阶段后停在 `waiting_join`（决策 82）；
- 只有当没有可运行游标时，executor 才整体暂停等 resume。

```
pending 触发
  │
  ├─ 1. 写入 pending_reason 到该游标（kanban_node_cursors.pending_reason_json，含 stage、node、message、suggested_actions）
  │     并把 kanban_tasks.pending_reason_json 同步为同一份（任务级投影，决策 82）
  ├─ 2. 通过 SSE 推送前端，展示 pending 卡片（带 branch，决策 84）
  ├─ 3. 该游标移出可运行集合；**其余 branch 继续执行**（决策 89）
  ├─ 4. 无 active 游标可推进时，executor 才返回、等 resume 调用
  └─ 5. 用户操作后 → POST /tasks/{id}/resume → executor 从各游标继续
```

```rust
// crates/app/src/routes/tasks.rs

pub async fn resume_task(
    Path(task_id): Path<String>,
    Json(body): Json<ResumeRequest>,
) -> Result<impl IntoResponse> {
    // 游标解析（决策 91）：并行区间**不存在 main 游标**（它已被改写为 develop-design），
    // 因此不能用"没有 main 就用 main 兜底"的写法。
    // 规则：恰好一条游标 → 用它；否则必须显式传 cursor_id，缺失返回 409。
    let cursor_id = match body.cursor_id.clone() {
        Some(id) => id,
        None => store.resolve_sole_cursor(&task_id).await?  // 0 条或多条 → Err(Conflict)
            .ok_or_else(|| Error::Conflict("该任务有多条活跃游标，必须显式提供 cursor_id"))?,
    };
    let cursor = store.get_cursor(&cursor_id).await?;

    // 校验动作是否在当前 pending type 的允许集合内（决策 49 / 69）
    validate_action_allowed(&cursor.pending_reason, &body.action)?;

    match body.action {
        Action::Continue { .. } => {
            // 用户补充了信息，清除该游标 pending，executor 从当前节点继续
            // 例外：dependency_failed 的"继续执行"（决策 116 / 130）= 忽略失败依赖，
            // 清 pending 后 status 置回 queued 交还 scheduler 准入，不在此 spawn executor
            if let Some(input) = &body.input {
                store.append_user_input(&task_id, input).await?;
            }
            store.clear_cursor_pending(&cursor_id).await?;
        }
        Action::Skip => {
            // 落点**不复用 entry_node**（决策 93）——因为 architect-design 的 next 是
            // 分裂成两个阶段，而并行分支的 skip 不得越过 join。见下方 skip 落点表。
            store.clear_cursor_pending(&cursor_id).await?;
            match cursor.stage.as_str() {
                // architect-design 放行 → 游标分裂，两条都落在各自阶段的入口
                "architect-design" => {
                    store.split_cursors(&task_id).await?;   // 幂等，见 §6 architect-design.next
                }
                // 并行分支放行 → 标记到达边界，交给 sync-check 汇总（不越过 join）
                "develop-design" | "test-design" => {
                    store.mark_cursor_skipped_to_join(&cursor_id).await?;  // status = waiting_join
                }
                // 串行阶段 → 下一阶段入口节点
                other => {
                    let next = next_stage(other);
                    store.set_cursor_stage(&cursor_id, next, entry_node(next)).await?;
                }
            }
            store.reset_cursor_attempts(&cursor_id).await?;
        }
        Action::Goto { target_stage, target_node } => {
            // 回退 / 跳到指定阶段节点，validate_attempts 重置（决策 43）
            store.clear_cursor_pending(&cursor_id).await?;
            store.set_cursor_stage(&cursor_id, &target_stage, &target_node).await?;
            store.reset_cursor_attempts(&cursor_id).await?;
        }
    }
    store.sync_task_projection(&task_id).await?;   // 刷新焦点游标投影（决策 80）

    // pending_resume_cooldown_sec 内不重复启动（防连点）
    tokio::spawn(run_executor(&task_id, db.clone()));
    Ok(Json(json!({ "ok": true })))
}
```

**ResumeRequest 模型（决策 35）：**

```typescript
interface ResumeRequest {
  action: "continue" | "skip" | "goto";
  cursor_id?: string;      // 作用于哪条游标。**恰好一条游标时可省略；多条时必须显式提供**，
                           // 否则返回 409（决策 91）——并行区间没有 main 游标，无法兜底
  target_stage?: string;   // action = "goto" 时必填
  target_node?: string;    // action = "goto" 时必填
  input?: string;          // action = "continue" 时的用户补充信息
}
```

**`goto` 的阶段入口节点（决策 69）：** `goto` 的落点用统一的 `entry_node(stage)` 查表，避免各阶段特例散落。

**`skip` 的落点表（决策 93，与 `goto` 分开）：** `skip` 不能简单套用 `entry_node`——`architect-design` 的 next 是分裂成两个阶段，而并行分支的 `skip` 若直接跳到 `sync-check.execute` 就等于绕过 G5 的 join 条件。规则如下：

| 当前阶段 | `skip` 落点 | 说明 |
|---|---|---|
| architect-design | 游标分裂 → develop-design / test-design 的 `validate_input` | 等价于"强制通过 architect-design" |
| develop-design / test-design | 本分支置 `waiting_join` + `skipped_to_join = true` | **不越过 join**；sync-check 读该标志视其 readiness=true |
| develop | `review.execute` | 串行，正常下一阶段入口 |
| review | `test.execute` | 强制通过评审 |
| test | `merge.execute` | 强制通过测试 |
| merge | — | **无 skip**（决策 86） |
| init / sync-check / done | — | 无 pending，故无 skip |

> **`skipped_to_join` 不改产出元数据（决策 93）：** 标记到达边界时**不得**去改写 `kanban_stage_outputs.metadata_json` 里的 `readiness`——那是 agent 的判断结论，伪造它会让审计失去意义。正确做法是在游标上记 `skipped_to_join = true` 并写一条 `user_resume` 类型的 `kanban_transitions`，由 `sync-check.execute` 读取该标志把对应分支视作 readiness=true。

> **skip 的空产出（决策 115）：** 并行分支在 `validate_input` 被 skip 时，本分支的产出文件（`dev-plan.md` / `test-scenarios.md`）**不存在**；sync-check 仍按 `skipped_to_join` 视其为 readiness=true。此时下游语义固定为"跳过该设计阶段 = 直接基于 `design.md` 工作"：`develop.execute` / `test.execute` 的 prompt 模板显式写明该降级，不视为错误。

| 阶段 | 入口节点 |
|---|---|
| init / sync-check / develop / review / test / merge / done | `execute` |
| architect-design / develop-design / test-design | `validate_input` |

**允许动作下发（决策 49 / 69）：** `GET /tasks/{id}` 的响应里带 `allowed_actions`，由后端按 `(pending_reason.type, context.kind)` 生成，前端纯渲染。每项形如：

```typescript
interface AllowedAction {
  action: string;                        // continue | skip | goto | cancel | split_task | ...
  kind: "resume" | "side_effect";        // resume → POST /resume；side_effect → 专用 API（决策 69）
  label: string;
  cursor_id?: string;                    // 该动作作用于哪条游标（并行时区分两个分支，决策 82）
  requires_input?: boolean;
  target?: { stage: string; node: string };
}
```

例如 `merge_approval` 返回"合入 / 返回修改"（二者均为 `side_effect`，走 `POST /tasks/{id}/merge/decision`，决策 119）；`dependency_failed` 返回"继续执行 / 取消任务 / 等待依赖重试"（其中"取消任务"是 `side_effect`，走 `POST /tasks/{id}/cancel`）；`retry_exhausted` 返回"重试 execute / 强制进入下一阶段 / 终止任务"（决策 70）。

**前端 pending 卡片交互：**

```
┌─────────────────────────────────────────┐
│ ⏸ 设计文档信息不足                        │
│                                         │
│ 缺少以下信息：                            │
│ - 技术约束（语言、框架）                   │
│ - 预期用户量                             │
│                                         │
│ [输入框: 请补充信息...]                    │
│                                         │
│ [补充并继续]  [跳过检查]  [回退到设计]     │
└─────────────────────────────────────────┘
```

### 11.4 轮询 → KanbanScheduler（独立调度器）

超时检测、冲突等待恢复**不适合放进 executor 循环里**。executor 是 DAG 执行引擎，不是定时任务引擎；轮询是"等外部事件"，不是"图流转"。

KanbanScheduler 承担图外的定时/轮询职责（决策 55）。**10s tick（轻量）** 与 **小时级维护** 分开：

```rust
// crates/core/src/scheduler/tick.rs

pub struct KanbanScheduler {
    db: SqlitePool,
}

impl KanbanScheduler {
    pub async fn tick(&self) -> Result<()> {
        // 10s 轻量周期：超时 / 冲突恢复 / 依赖启动 / 并发准入 / pending 提醒 / stalled
        self.check_timeouts().await?;
        self.resume_conflict_waits().await?;
        self.check_waiting_tasks().await?;     // 依赖启动（决策 57）
        self.recover_dependency_failed().await?; // 依赖被重试后清 pending 退回 waiting（决策 57）
        self.admit_pending_tasks().await?;     // 并发准入（决策 36）
        self.remind_pending_tasks().await?;    // pending 提醒 + stalled 标记（决策 55）
        Ok(())
    }

    /// 小时级维护：与图流转无关的重维护
    pub async fn maintenance(&self) -> Result<()> {
        self.purge_expired_conversations().await?;   // 过期会话清理
        self.aggregate_node_metrics().await?;        // node_runs 聚合
        Ok(())
    }

    async fn check_timeouts(&self) -> Result<()> {
        // 超时时钟取自 kanban_node_runs.started_at（决策 64），不用 updated_at
        // 空闲超时（node_idle_timeout_sec）与绝对超时（node_max_duration_sec）分别判定
        // 超时只作用于该分支的节点 run，另一分支不受影响（决策 82）
        for run in self.db.get_active_node_runs().await? {
            let idle = Utc::now() - run.last_activity_at;
            let total = Utc::now() - run.started_at;
            if idle > effective_idle_timeout(&run) || total > effective_max_duration(&run) {
                // 杀死进程组，节点失败 → 按 agent_retry_max 重试；耗尽才 pending(timeout)
                self.handle_timeout(&run).await?;
            }
        }
        Ok(())
    }

    async fn handle_timeout(&self, run: &NodeRun) -> Result<()> {
        kill_process_group(run.process_group_id).await?;
        let task = self.db.get_task(&run.task_id).await?;
        if run.attempt < self.settings.agent_retry_max {
            // 未耗尽 → 干净对话重试当前节点（决策 33）
            tokio::spawn(run_executor(&task.id, self.db.clone()));
        } else {
            // pending 挂到该 run 所属的游标上（决策 82），任务级 pending 随之投影
            self.db.set_cursor_pending(run.cursor_id, PendingReason {
                type_: PendingReasonType::Timeout,
                stage: run.stage.clone(),
                node: run.node.clone(),
                message: format!("{}.{} 执行超时（attempt {}）", run.stage, run.node, run.attempt),
                ..Default::default()
            }).await?;
            self.db.sync_task_projection(&task.id).await?;
        }
        Ok(())
    }

    async fn resume_conflict_waits(&self) -> Result<()> {
        for cursor in self.db.get_cursors_with_pending_type(PendingReasonType::ConflictWait).await? {
            let pending = cursor.pending_reason.as_ref().unwrap();
            // ① conflict_wait 记的是**全部**冲突任务 id（决策 102），不是单个
            let conflict_ids = pending.context.get_str_list("conflict_task_ids")?;

            // ② 必须**全部**终态才考虑恢复
            let tasks = self.db.get_tasks(&conflict_ids).await?;
            let all_terminal = tasks.iter().all(|t| matches!(
                t.status, TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled
            ));
            if !all_terminal {
                continue;
            }

            // ③ 恢复前**重跑第一层比对**（决策 102）：等待期间活跃任务集合可能已变化，
            //    既可能新冒出第三个冲突任务，也可能原对手终态后不再冲突。
            //    - 仍有交集 → 保持 pending，用新的 id 列表更新 context（不重跑节点）
            //    - 无交集   → 清 pending，拉 executor 继续
            //    这里用任务已有产出（affected_files / new_symbols）重新求交集。
            let still = self.db.recheck_first_layer_overlap(&cursor).await?;
            if still.is_empty() {
                self.resume_cursor(&cursor).await?;
            } else {
                self.db.update_cursor_pending_context(
                    &cursor.cursor_id, "conflict_task_ids", &still
                ).await?;
                // 推送 pending_updated 让看板刷新冲突对象
                push_sse(&cursor.task_id, SseEvent::PendingUpdated {
                    cursor_id: cursor.cursor_id.clone(),
                    context: json!({ "conflict_task_ids": still }),
                }).await;
            }
        }
        Ok(())
    }

    async fn resume_cursor(&self, cursor: &NodeCursor) -> Result<()> {
        self.db.clear_cursor_pending(&cursor.cursor_id).await?;
        self.db.sync_task_projection(&cursor.task_id).await?;
        tokio::spawn(run_executor(&cursor.task_id, self.db.clone()));
        Ok(())
    }
}
```

**调度器和 executor 的交互通过 DB + resume：**

```
scheduler 检测到条件满足
  → 清除 pending_reason
  → spawn run_executor(task_id)
  → executor 从 checkpoint 继续执行
```

**调度器谓词必须按游标判定（决策 92 / 98）：**

| 谓词 | 正确判据 | 为什么不能用 `task.status` |
|---|---|---|
| pending 提醒 / stalled 标记 | `!has_runnable_cursor(task)`：无 `active` 游标且无 `waiting_join` 可推进 | 一个分支 pending、另一个仍在跑的任务 `status = pending`，但它并没有卡住 |
| 并发准入 `admit_pending_tasks` | 取 `status = queued` 的任务，按 `max_concurrent_tasks` 放行；名额占用 = `running + pending`（决策 117） | `queued` 是与 `waiting` 正交的独立状态（决策 98） |
| 看板 pending 筛选 | `has_pending_cursor(task)` | 任务级 `status = pending` 只是投影，不能区分哪条分支被阻塞 |
| 看板列归属 | 并行区间用独立槽位 + 两个分支药丸（决策 92） | `current_stage` 只是焦点投影，不驱动列 |

### 11.5 数据库表设计

SQLite 数据库位于 `~/.agentpipeline/data/agentpipeline.db`，WAL 模式，通过 sqlx migrations 管理：

```sql
-- 任务主表
-- current_stage / current_node / validate_attempts 是「焦点游标」的投影（决策 80），
-- 供看板展示与筛选；执行状态的唯一事实来源是 kanban_node_cursors。
-- 串行阶段恒有一个游标，投影与游标完全一致。
CREATE TABLE IF NOT EXISTS kanban_tasks (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    project_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',  -- 决策 98：一律 queued/waiting 落库，准入后置 running
    current_stage TEXT NOT NULL,           -- 焦点游标 projection
    current_node TEXT NOT NULL DEFAULT 'execute',
    validate_attempts INTEGER NOT NULL DEFAULT 0,   -- 焦点游标 projection
    pending_reason_json TEXT,              -- JSON 序列化的 PendingReason（任务级，决策 82）
    worktree_path TEXT,                    -- git worktree 路径
    branch_name TEXT,                      -- 隔离分支名
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_calls INTEGER NOT NULL DEFAULT 0,
    review_mode TEXT NOT NULL DEFAULT 'agent',  -- agent | human
    model_override TEXT,                   -- 任务级 provider 覆盖（决策 105）
    archived_at TEXT,                      -- 归档时间（软删除，决策 34）
    stalled INTEGER NOT NULL DEFAULT 0,    -- pending 超时标志（决策 34）
    executor_owner TEXT,                   -- 当前 executor 持有者（乐观锁，决策 36）
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 活跃游标表（决策 80）：执行状态的唯一事实来源
CREATE TABLE IF NOT EXISTS kanban_node_cursors (
    cursor_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    branch TEXT NOT NULL DEFAULT 'main',   -- main | develop-design | test-design
    stage TEXT NOT NULL,
    node TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active', -- active | waiting_join | pending | archived（决策 82 / 113）
    validate_attempts INTEGER NOT NULL DEFAULT 0,
    skipped_to_join INTEGER NOT NULL DEFAULT 0,  -- 用户对本分支 skip 的放行标志（决策 93）
    pending_reason_json TEXT,              -- 游标级 pending（决策 82）
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id)
    -- 游标行永不物理删除（决策 113）：合并 / backtrack / 重试把旧行置 archived 后插入新行，
    -- kanban_node_runs.cursor_id 的 NOT NULL 外键因此永不悬空。
);
-- 唯一约束只作用于活跃行：archived 行让位于同分支的新行（决策 113）
CREATE UNIQUE INDEX IF NOT EXISTS uq_node_cursors_active_branch
    ON kanban_node_cursors(task_id, branch) WHERE status != 'archived';
CREATE INDEX IF NOT EXISTS idx_node_cursors_task ON kanban_node_cursors(task_id, status);

-- 任务依赖表
CREATE TABLE IF NOT EXISTS kanban_task_deps (
    task_id TEXT NOT NULL,
    depends_on_id TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on_id),
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (depends_on_id) REFERENCES kanban_tasks(id)
);

-- 阶段产出记录（文件路径 + 结构化元数据，决策 30）
CREATE TABLE IF NOT EXISTS kanban_stage_outputs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    stage TEXT NOT NULL,
    output_type TEXT NOT NULL,         -- "design_doc" | "dev_doc" | "merge_result" | "review_diff" | ...
    file_path TEXT NOT NULL,
    metadata_json TEXT,                -- 路由依据（readiness/approved/changed_files 等）
    stale INTEGER NOT NULL DEFAULT 0,  -- 决策 83：backtrack 标过期（文件保留，覆盖写入时清除）
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    UNIQUE(task_id, stage, output_type)   -- 供 upsert（G9 幂等）
);

-- 节点执行记录（可观测性，决策 63）：主代理会话与 run 1:1
-- 归属二选一（票 10 / 决策 100）：任务级 run 有 task_id + cursor_id；
-- 项目级伪阶段（project_analysis）无任务无游标，改以 project_id 归属。
CREATE TABLE IF NOT EXISTS kanban_node_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT,                      -- 任务级 run 的所属任务；项目级伪阶段为 NULL
    cursor_id TEXT,                    -- 归属哪条游标（决策 80/82）；游标行只归档不删除，外键永不悬空（决策 113）
    project_id TEXT,                   -- 项目级伪阶段的归属（票 10）；任务级为 NULL
    stage TEXT NOT NULL,
    node TEXT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 1,
    agent_type TEXT NOT NULL DEFAULT 'main',   -- main | code_searcher | test_runner | doc_writer（决策 77）
    parent_run_id INTEGER,             -- 子代理关联父 run；main 时为 NULL（决策 77）
    status TEXT NOT NULL,              -- running | success | failed | timeout
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,    -- prompt cache 命中（决策 46 配套可观测）
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    process_group_id INTEGER,          -- 超时时杀进程组用（决策 66）
    last_activity_at TEXT,             -- 空闲超时心跳（决策 64/66）
    prompt_template_hash TEXT,         -- prompt 版本标注（决策 137）：最终组装 system prompt 的 SHA-256
                                       -- 前 16 位（含用户 prompts/ 覆盖后的内容），指标按版本对比
    started_at TEXT NOT NULL,
    finished_at TEXT,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id),
    FOREIGN KEY (cursor_id) REFERENCES kanban_node_cursors(cursor_id),
    FOREIGN KEY (project_id) REFERENCES kanban_projects(id),
    FOREIGN KEY (parent_run_id) REFERENCES kanban_node_runs(id),
    CHECK ((task_id IS NOT NULL) <> (project_id IS NOT NULL))   -- 归属恰好其一（决策 100 / 票 10）
);

-- 项目管理
CREATE TABLE IF NOT EXISTS kanban_projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    local_path TEXT NOT NULL,             -- 本地仓库路径（唯一事实来源，决策 29）
    default_branch TEXT NOT NULL DEFAULT 'main',
    language TEXT,                        -- 检测到的编程语言
    test_framework TEXT,                  -- 检测到的测试框架
    lint_command TEXT,                    -- 可选静态检查命令（决策 139）；project_analysis 探测候选、
                                          -- 用户确认时预填。未配置则闸门跳过 lint 环节，不阻塞
    agents_md_path TEXT,                  -- AGENTS.md 路径（如果存在）
    created_at TEXT NOT NULL
);

-- checkpoint 由 executor 自动管理（存储在 kanban_node_cursors 表的各行，决策 80）
-- provider 配置（决策 22 / 46）：界面可改，含模型上下文窗口与明文 api_key
-- 决策 111：一行 = 一个 (厂商, 模型, 上下文窗口)。阶段通过 provider_id 引用它，
--           阶段不再单独存 model —— 换模型即换 provider_id，context_window 查找路径唯一。
-- 决策 112（修订决策 10）：api_key **明文存储**，不再单独建 api_keys 表。
--           安全性依赖 ~/.agentpipeline 的目录权限（§12.14），**不防本机 shell**。
CREATE TABLE IF NOT EXISTS providers (
    id TEXT PRIMARY KEY,
    vendor TEXT NOT NULL,                   -- openai | anthropic | deepseek | ...（∈ supported_adapters）
    model TEXT NOT NULL,
    context_window INTEGER NOT NULL,        -- 模型上下文窗口（决策 46）
    base_url TEXT,
    api_key TEXT,                           -- 明文密钥（决策 112）。前端只回显 ***，不回传原值
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 注：api_keys 表已删除（决策 112，修订决策 10）。
--     原 `GET/POST/DELETE /providers/{id}/api-keys` 收敛为 providers 的 CRUD 内联字段。

-- 阶段级 agent 配置（决策 22 / 46 / 66 / 111）：界面可改；系统基线硬编码不可覆盖
CREATE TABLE IF NOT EXISTS stage_configs (
    stage TEXT PRIMARY KEY,
    provider_id TEXT,                       -- 引用 providers.id（决策 111）；不再单列 model
    temperature REAL,
    max_tokens INTEGER,                     -- 替代已移除的 node_max_tokens（决策 56）
    persona_path TEXT,
    persona_append TEXT,
    tools_json TEXT,                        -- 增量工具（与基线取并集）
    skills_json TEXT,                       -- 增量 skill（与基线取并集）
    idle_timeout_sec INTEGER,               -- 阶段级超时覆盖（决策 66）
    max_duration_sec INTEGER,
    node_overrides_json TEXT,               -- 节点级覆盖（决策 66）
    updated_at TEXT NOT NULL,
    FOREIGN KEY (provider_id) REFERENCES providers(id)
);
```

> **其余表的位置：** `kanban_transitions`（§12.4.2）、`kanban_node_conversations`（§12.4.3）、`kanban_node_commands`（§12.4.4）分列在可观测性各节，此处不重复。`kanban_project_analyses`（决策 130⑦）：`analysis_id TEXT PRIMARY KEY`、`project_id TEXT NOT NULL REFERENCES kanban_projects(id)`、`status TEXT NOT NULL`、`result_json`、`error`、`created_at`、`updated_at`——配套 `POST /projects/analyze` 异步 202 + `GET /projects/{id}/analysis` 轮询。全部表由 sqlx migrations 统一管理（决策 13）。

> **技能来源相关的表（决策 194）：** 仓名单住 `kanban_market_repos`（迁移 **`0010_market_repos.sql`**：机器级**单行表 + `CHECK (id = 1)`**，与迁移 0008 / 0009 那两张单行表同族）——「显式清空」与「没保存过」必须分得开；装下来的技能来源住 `skill_sources`（迁移 **`0011_skill_sources.sql`**：`name TEXT PRIMARY KEY` + `owner` / `repo` / `commit_sha` / `subpath` / `installed_at`，**一行一技能**，卸载时一并删）。
> **迁移 `0009_market_sources.sql` 的文件保留、读写它的代码退场**——`sqlx::migrate!` 对每个**已应用过**的迁移文件记校验和，**改动或删除已应用的迁移都会让既有库在启动时报版本不符**（与决策 193 记的是同一条性质）；要连表一起清掉得是一条**新迁移**（`DROP TABLE`）加一次显式的数据处置决定，不是删文件。

### 11.6 进程中断恢复

executor checkpoint 机制天然支持：

```
进程崩溃
  → 重启
  → 清理 executor_owner 残留：UPDATE kanban_tasks SET executor_owner = NULL（决策 127）
    ——kill -9 后乐观锁持有者不会释放，不清理则该任务永远无法被重新 claim
  → 从 SQLite 加载全部活跃游标（kanban_node_cursors 各行的 stage + node，决策 80）
  → executor 逐游标恢复到中断节点（并行任务恢复到两条游标各自的位置）
  → scheduler.tick() 检查是否有 pending 需要处理
  → 一切继续
```

**恢复粒度：节点级。** 如果 execute 节点中途崩溃，恢复后从该节点入口重新执行（幂等保证）。不会丢失已完成的节点结果。并行场景下两个游标独立恢复，互不影响——某条游标停在 `waiting_join` 时也照样保留，等另一条就位后由 join 节点推进。

**ctrl+c 优雅关闭（决策 54）：**

```
第一次 SIGINT
  → 置全局 shutdown 标志
  → scheduler 停止派发新任务
  → 正在执行的节点跑完当前节点后，在节点边界退出（checkpoint 停在节点级）
  → 任务状态保持 running，不改动（下次启动由 executor 恢复）
  → updated_at 不刷新，超时检测在重启后自然接管
第二次 SIGINT
  → 立即退出；被中断的节点靠幂等 + checkpoint 重跑
```

### 11.7 关键接口汇总

| 接口 | 方法 | 说明 |
|---|---|---|
| `POST /tasks` | POST | 创建任务（可带 `depends_on`）。**一律以 `queued`（有依赖则 `waiting`）落库**，由 scheduler 按 `max_concurrent_tasks` 准入后才启动（决策 98） |
| `GET /tasks` | GET | **任务列表**（看板数据源，决策 101）：支持 `project_id` / `status` / `include_archived` 过滤；每条任务带分支级摘要（各游标的 `branch` / `stage` / `node` / `status`） |
| `GET /tasks/{id}` | GET | 查询任务当前状态（stage + node + pending + `allowed_actions`） |
| `GET /tasks/{id}/stream` | GET(SSE) | **唯一**事件流：节点 / 流转 / 命令 / pending / 终态事件，按 `type` 区分（决策 76） |
| `POST /tasks/{id}/resume` | POST | 恢复动作（`continue` / `skip` / `goto`，决策 35；`kickback` 已废弃）。多游标任务必须带 `cursor_id`，否则 409（决策 91） |
| `GET /tasks/{id}/files/{path}` | GET | 读取任务产出文件内容 |
| `POST /tasks/{id}/retry` | POST | 失败任务重新进入 init（复用 worktree，重置到分支起点——决策 125）：置回 `queued` 由 start_task 重新准入（决策 117），旧游标归档、新建单条 main（决策 90 / 113） |
| `POST /tasks/{id}/cancel` | POST | 取消任务，清理 worktree |
| `POST /tasks/{id}/split` | POST | **旁路动作**：按用户给定方案创建 N 个新任务（复用 `POST /tasks` 全部校验）并把原任务置 `cancelled`；v1 不做自动拆分规划（决策 105） |
| `POST /tasks/{id}/model-override` | POST | **旁路动作**：设置任务级 `model_override`（仅影响本任务后续节点，不改全局 stage config）（决策 105） |
| `POST /tasks/{id}/archive` | POST | 归档终态任务（软删除，写 `archived_at`） |
| `POST /tasks/{id}/review` | POST | 人工评审结果提交（review_mode = "human"） |
| `POST /tasks/{id}/merge/decision` | POST | merge_approval 的旁路动作端点：`{decision: "approve" \| "return"}`，单事务内写 `merge_result.approval` + 清 pending + 置游标（决策 119） |
| `GET /tasks/{id}/flow` | GET | 流转时间线的历史查询（实时推送统一走 `/tasks/{id}/stream`，决策 76） |
| `GET /tasks/{id}/metrics` | GET | 任务成本与耗时统计 |
| `GET /tasks/{id}/conversations` | GET | 各节点会话摘要列表 |
| `GET /tasks/{id}/conversations/{run_id}` | GET | 单个节点完整会话内容 |
| `GET /tasks/{id}/commands` | GET | 命令执行列表（agent + 系统），支持按 stage/node 过滤 |
| `GET /tasks/{id}/commands/{cmd_id}` | GET | 单条命令详情（含完整输出） |
| `GET /tasks/{id}/commands/{cmd_id}/output` | GET | 完整 stdout（从卸载文件读取） |
| `POST /projects` | POST | 创建项目（本地路径，立即校验是否为 git 仓库） |
| `GET /projects` | GET | 项目列表 |
| `PATCH /projects/{id}` | PATCH | 修改项目配置（名称 / `default_branch` / `test_framework` 等，决策 101） |
| `DELETE /projects/{id}` | DELETE | 删除项目（有活跃任务时拒绝，决策 101） |
| `POST /projects/analyze` | POST | 触发 `project_analysis` 伪阶段静态分析（决策 48）。**异步**（决策 130）：立即返回 `202 {analysis_id}`（分析含 LLM 调用，不阻塞 HTTP），前端轮询 `GET /projects/{id}/analysis` 至完成 |
| `GET /projects/{id}/analysis` | GET | 最近一次项目分析的状态与结果（决策 130） |
| `GET/POST/PATCH/DELETE /providers` | — | provider / model 配置 CRUD（存 DB，决策 22/46）。**`api_key` 为明文内联字段**（决策 112）；读接口只回显 `***`，不返回原值 |
| `GET /metrics` | GET | 全局统计（成功率、平均耗时、token 消耗） |
| `GET /skills` | GET | 已安装技能清单（名字 / 描述 / 来源 / `declared_in`——被哪些阶段与节点引用，供卸载前看后果）（决策 172⑤，票 09） |
| `POST /skills/import` | POST | 上传 **zip 原始字节**（`?name=&overwrite=`）装技能；校验含 `SKILL.md`、frontmatter 可解析、正文非空，落到技能根 `{name}/SKILL.md` + 兄弟文件；同名默认 409 并报出现有来源，`overwrite=true` 才覆盖；`name` 可省略（包为 `{name}/SKILL.md` 布局时自动推断，平铺包须显式给）（票 09） |
| `POST /skills/import-dir` | POST | 从一个或多个本地技能目录导入（`{paths: [], overwrite}`），**逐项返回结果**，一项失败不中断整批（票 09） |
| `GET /skills/scan` | GET | 扫描一个本地技能根（`?root=~/.zcode/skills`）列出可导入技能：名字 + `description` + `exists`（票 09） |
| `DELETE /skills/{name}` | DELETE | 卸载技能（删技能根下 `{name}/`）。**不检查引用**——仍被引用的卸载后由启动校验与 `PUT /stage-configs` fail fast 兜住（票 09；技能只剩技能根下的 markdown 一个来源，决策 185） |
| `GET /market/repos` | GET | 当前生效的**技能来源仓**名单：`{repos, origin: "settings" \| "config", recommended}`——`origin` 标明这份来自界面保存（住 DB）还是回落 `config.toml` 的 `[market] github_repos`（决策 187 的两级结构，信任单元由决策 194 换成 `owner/repo`）（票 03） |
| `PUT /market/repos` | PUT | 保存界面这份仓名单（`{"repos": ["owner/repo", …]}`），**保存即生效**（当场换，不重启）；显式空数组 = 不装任何远程技能，与「没保存过」分得开；校验只有一处实现（与配置解析共用）（票 03） |
| `DELETE /market/repos` | DELETE | 清掉界面这份，回落 `config.toml` 那一级（票 03） |
| `GET /market/skills` | GET | 列出某仓的技能（`?repo=owner/repo&q=&refresh=1`）：用 `head()` **钉住一个 commit**（只 ls-remote、不下载 pack），按技能目录的父路径分组；`q` 是对**已 fetch 那一份**的本地过滤（不引 GitHub search API）；`refresh=1` 重新 `head()`，否则用缓存里那个 commit。响应含 `commit` / `commit_short` / `listed_at`（票 01 / 03，决策 194 裁决⑤：列表钉住浏览时的 commit） |
| `POST /market/install` | POST | 从一个钉住的 commit 安装（`{"owner","repo","commit","subpath","overwrite"}`）：`read_skill`（按 (仓, commit) 缓存，与列表共用一份）→ 内存里重打成 `{name}/SKILL.md` 单根包 → `SkillPackage::from_zip` → `install`（**既有落盘入口零改动**）→ 写一行来源记录（`skill_sources`）。`commit` **一路透传、不得中途「取最新」**（「看到的 = 装到的」唯一落点）。**错误体统一带一个机器可读的 `kind`**（与既有 `error` / `detail` 并列）：`market_network` 502 / `repo_not_found` 404 / `commit_not_found` 404 / `skill_not_found` 404 / `repo_unreadable` 401·404 / `digest_mismatch` 400（语义是「git 对象哈希不符」，比旧的字节 sha256 更强）/ `repo_not_allowed` 400 / `download_too_large` 400；同名未确认 409。**界面按 `kind` 分支**，不按状态码也不按 `error` 里的字样（票 01 / 02 / 03，决策 194 裁决⑦） |

> **旧三组市场端点整组退场**（决策 194）：`GET /market/search`、`POST /market/install`（旧请求体 `{name, overwrite}`）、`GET | PUT | DELETE /market/config` 以及 `config.toml` 的 `[market] allowed_sources` 都不再存在——自定 `/index.json` registry 那一层**整层退场**，理由与残留处置见 `docs/agents.md` 的「从 GitHub 仓安装」一节与票 04 的删单。

> **技能市场全程离线（票 09）：** 五个导入 / 扫描 / 卸载端点不依赖任何网络，本机无网时功能完整。
> 上传走**原始字节**而非 multipart / base64——`multipart` 要新引 `multer` 一棵树，base64 要一个
> 编解码依赖并让体积涨 33%，而原始字节零依赖（`fetch(url, {body: file})` 即可）。
> **路径穿越**是本组端点的主要风险：zip 条目名过两道独立判定（自己的 `sanitize_rel_path` +
> `zip` crate 的 `enclosed_name`），落盘前实数校验目标在技能根之内（见
> `crates/core/src/agent/skill_import.rs` 模块头）。从 GitHub 仓安装（决策 194）复用同一落盘入口——
> 来源侧读出的技能目录重打成 `{name}/SKILL.md` 单根包后才交给它，故远程包不比本地上传的包享有更宽的路。

> **仓访问接缝的注入姿态（决策 194，修订决策 143 第五条接缝）：** `crates/core/src/agent/repo.rs`
> 的 `SkillRepo` trait（`head` / `list_skills` / `read_skill`）是本批**唯一新增的接缝**，生产实现是
> 走 libgit2 git 通道的 `Libgit2Repo`；L3 契约测试注入 testkit 的**两层离线 fixture**（本地裸仓 /
> 离线 smart HTTP），因此「commit 取不到」「技能目录不存在」「对象哈希不符」「传输超限与中断」
> 这些**真网络没法稳定复现**的路径都成了确定性、离线的用例。仓名单为空是**合法状态**
> （= 不装远程技能），端点返回一条说明怎么开的 400，而不是 500。上一代的自定 registry 客户端
> （`AppState.market: Option<Arc<dyn MarketClient>>` / `HttpMarketClient` / testkit 的 `FakeMarket`）
> 随那一层退场（决策 194）。

> **`allowed_actions` 与端点的配对（决策 101 / 119）：** 前端对 `allowed_actions` 纯渲染，因此每个 `side_effect` 动作都必须有对应端点——`cancel` → `POST /tasks/{id}/cancel`、`split_task` → `POST /tasks/{id}/split`、`更换长上下文模型` → `POST /tasks/{id}/model-override`、`合入 / 返回修改` → `POST /tasks/{id}/merge/decision`（决策 119）。新增 side_effect 动作时必须同时新增端点，否则前端会出现点不动的按钮。

### 11.8 可测试性接缝（决策 143）

测试设计（[testing.md](testing.md) §3.1）要求实现预留四条接缝，生产代码只依赖 trait、不感知测试形态：

| 接缝 | 生产实现 | 测试实现 |
|---|---|---|
| `Clock` trait | 系统时钟（超时 / 心跳 / tick 的唯一时钟源语义不变，决策 64） | 假时钟：手动推进 / `tokio::time::pause` |
| `AGENTPIPELINE_HOME` 环境变量 | 默认 `~/.agentpipeline/` | 每测试独占临时目录 |
| 进程组终止器 trait | 真杀进程组（决策 66） | 记录调用，不真杀 |
| scheduler `tick()` | 10s 周期驱动 | 测试中手动调用 |

**第五条接缝（决策 194 修订决策 143 / 177，票 01）：** 换形状——从「网络出口加一条 `MarketClient`」变成「**仓访问加一条 `SkillRepo`**」。**条数仍是五条**，自定 registry 退场后 `MarketClient` 那种「索引 → 下载字节」的形状**没有对应物**（没有索引、没有 `sha256`、字节来自 git 对象库），故新接缝按「仓访问」切。
（测试设计侧的同一条接缝见 [testing.md](testing.md) §3.1 的权威表；决策 169 的主题契约随之成为第六条。）

| 接缝 | 生产实现 | 测试实现 |
|---|---|---|
| `SkillRepo` trait（`crates/core/src/agent/repo.rs`：`head` / `list_skills` / `read_skill`） | `Libgit2Repo`：`head` 只 ls-remote、**不下载 pack**；`list_skills` / `read_skill` 走 libgit2 的 git 通道（`RemoteRedirect::None` **显式设**、`depth(1)`、字节上限 64 MiB 在流式回调里守） | testkit 的**两层离线 fixture**：本地裸仓（快单测；**不能带 `depth`**——local transport 直接报 `shallow fetch is not supported by the local transport`）与**离线 smart HTTP**（核心用例：真 HTTP 传输 + `depth(1)` + 重定向策略 + 中断），**不打真网络** |

> 这是 v2 技能 effort **唯一新增**的接缝（决策 143 的「接缝数不随功能数线性增长」）。加它的
> 理由与四条老接缝同构：这条来源下有多条**真网络无法稳定复现**的失败与策略路径（commit 取不到 /
> 技能目录不存在 / 对象哈希不符 / 传输超限与中断 / 不跟随跨站重定向），而票面要求它们互不混淆——
> 只有把仓访问换成 trait，这些路径才能被钉住（决策 194）。

> **实现顺序要求：四个接缝先于业务模块落地**——后补接缝要翻全部模块签名。逐项用例目录见 testing.md。
