# 游标推进收成一个深 module（cursor-advance）

**状态（2026-09-23）：票 01–03 已全部落地（`Status: done`），1104 个测试全绿。** 本目录来自一次架构评审
（`improve-codebase-architecture`）的候选 1，经四轮拷问定下设计树。评审报告是临时的，不入库；
设计结论以本文件与决策 245 为准。

## 一、问题

`docs/implementation.md:148` 承诺的一个深接口
`advance_cursor(db, cursor, node_result, &graph())` **不存在**。现实是「游标推进」（把游标从
一个落点搬到下一个）散成四份实现，而调用方必须自己记住 3–5 笔存储调用的顺序：

| 实现 | 位置 | 形状 |
|---|---|---|
| `advance_cursor` + `apply_edge` | `pipeline/executor.rs:2764-3070` | 6 个 `EdgeKind` 臂 |
| `Store::apply_resume` | `storage/decisions.rs:249-403` | Continue / Skip / Goto |
| `advance_after_judge_continue` | `storage/decisions.rs:619-639` | 20 行，已与 executor 同源 |
| `handle_timeout` | `scheduler/mod.rs:330-363` | 重试支 + pending 支 |
| `unstick` | `pipeline/unstick.rs:126-201` | 进程级编排 + 三步 DB |

**结构原因**：`Store::begin_write()` 是 `pub(crate)`（`storage/mod.rs:133`），只对 storage 模块内
的多个步骤开放。pipeline 侧**根本没有**组合事务的能力，于是「多笔写」成了唯一选项。

**落点这一层已经没有重复**（票 03 已让 executor 与 `advance_after_judge_continue` 共用
`pipeline::landing::stage_landing`）。重复的是**「落点 → 一串写」**。

## 二、四处真实缺口（都有现场证据）

1. **原子性**。真正非原子的有四条：`Retry`（3 笔）、`Next`-StageEntry（2 笔）、
   `Next`-JoinBoundary（2 笔）、`Pending`（2 笔）。其中 `EdgeKind::Retry`
   （`executor.rs:2831-2848`）里 `increment_cursor_attempts` 与 `move_cursor` 是两笔独立的
   autocommit 事务——中间那一瞬游标是「attempts+1 但还指着 validate_output」。
   `cursors.rs:376-379` 的注释证明团队知道这个形状的危害并写明了绕开它的方法，retry 边恰好没绕。
   **另外三条已经是原子的**（`split_cursors` / `backtrack_cursors` / `replace_cursors_with_main`
   都走 `begin_write`），所以新门要包的是那四条，不是全部。

2. **pending 三连四处抄**。`set_cursor_pending` → `sync_task_projection` → `emit(Pending)` 在
   `executor.rs:461-470`、`executor.rs:2800-2809`、`scheduler/mod.rs:345-362` 各抄一遍。
   `executor.rs` 里甚至同时存在深版本（`pend_cursor_with_context`）与一份手抄副本。

3. **冗余写**。`set_cursor_stage` 的 SQL 本身已含 `validate_attempts = 0`（`cursors.rs:287`），
   而 `skip_landing` 分支（`decisions.rs:337`）、`goto` 分支（`:384-386`）、
   `advance_after_judge_continue`（`:632`）**各自又补一次** `reset_cursor_attempts`。
   三条路语义相同（都归零），故这是**行为中性的 cleanup**，不是修 bug。

4. **SSE 静默缺失**。交互式 resume **今天完全不发 SSE**——`decisions.rs` 里零 `emit`，而
   `Store` 结构体（`storage/mod.rs:47-53`）只有 `pool` / `clock` / `home` / `conversation_max_chars`，
   **没有 `sse` 字段**，storage 侧即使想发也发不出。界面只能靠重新拉取或轮询。这是**用户可见的
   行为变化**，也是本批要立决策 245 的直接原因。

## 三、设计树（四轮拷问定稿）

**主目标**：以 **locality** 为纲、**原子性为硬约束**；可测性是副产品。

**接缝**：新建 `pipeline/advance.rs`，由它拥有翻译与事务边界，接 `&Store`；storage 退回行级原语。
选它而非「`Store::advance`」的理由：落点表本来就在 pipeline（`landing.rs`），而事务只是机制；
让 storage 拥有 `EdgeKind` 这类上层词汇是把一处重复换成一处泄漏。**与决策 80（游标是唯一事实
来源）、决策 163①（多步读写必须走 `BEGIN IMMEDIATE`）同向，不冲突。**

**接口**（门吃**落点 + 修饰**，不吃原因；原因在门外各自翻译）：

```rust
// crates/core/src/pipeline/advance.rs
pub enum Landing {
    Entry(Stage, Node),              // 串行下一阶段入口
    JoinBoundary { skipped: bool },  // 到 join 边界；skipped=true 置 skipped_to_join（决策 93）
    Pause { kind: PendingKind, context: Option<PendingContext>, message: String },
    // Split / Replace 不在这里：它们已是原子的 Store 方法，门只补 transition。
}

pub struct Advanced {
    pub cursors: Vec<NodeCursor>,
    pub replaced: bool,
}

pub async fn advance(
    store: &Store,
    task_id: &str,
    cursor: &NodeCursor,
    landing: Landing,
    trigger: TransitionTrigger,
    reason: Option<&str>,
) -> Result<Advanced>;
```

- **不吃原因**：四种原因各有自己的校验（judge-continue 不接受 `Terminal`、goto 必须落在
  `entry_node`、skip 在 merge 上非法），门吃原因就要把校验搬进去，门会重新长成第二个 `route()`。
- **只返回值**：门不发 SSE、不做任务投影。`Advanced` 返回后由调用方完成「同步投影 + 发事件」。
- **`Split` / `Replace` 只补 transition**，**不把「游标 + 流转行」扩成原子对**——今天无人依赖
  它们原子，扩了是新约束而非修 bug。

**范围纪律（四项「不做」）**：

- `handle_timeout` 的**重试支不进**：它的游标不动、attempts 不动、且带一个进程级重派
  （`(self.resume)(task_id)`）——那不是推进。只有 pending 支进。
- `unstick` 的**编排不进**：第一步 `force_release` 是进程内存操作、失败中段不能回滚。硬塞进来
  会把纯 DB module 变成要接 `&dyn Fn` 闭包的 module。它的 DB 部分复用新门。
  **由此得一条一致规矩：进程内存 / 重派不进事务模块。**
- `ExecutionState` **不在本次**：`glossary.md:16` 把它定义为头等概念，代码里一处都没有
  （散落的 `Vec<NodeCursor>` 局部量）。补出真类型是**独立候选**，不是本票附属。
- **文件落盘顺序保持现状**：`retry-feedback.md` / `backtrack-feedback.md` / `user-input.md` 今天
  靠「先写文件再动游标」的注释维持（`decisions.rs:370-382`、`executor.rs:2459-2472`）。不把
  `fs` 纳进门（同「SSE 推出门」的理由），只**如实记下**这条依赖。

**`ResumeAction → 落点` 的翻译搬进已存在的 `pipeline/resume.rs`**（票 08 抽出的 resume
「唯一实现」）。`Store::apply_resume` 是**唯一**住在 storage 却满口领域词汇的函数，搬走后 storage
回到纯行级原语。

## 四、三张票

| 票 | 内容 | 依赖 |
|---|---|---|
| [01](issues/01-backtrack-dead-code.md) | 取证：`EdgeKind::Backtrack` 是死代码（生产也不可达）+ 反向不变量用例 | — |
| [02](issues/02-advance-module.md) | 新 module + executor/resume 接入 + 补 SSE + 决策 245 | 01 |
| [03](issues/03-scheduler-unstick-docs.md) | scheduler pending 支 + unstick DB 部分 + 三处断言式文档 + 两处过期编号 | 02 |

**为什么 01 先于 02**：01 是**取证**，它的结论会改变 02 的接口（`apply_edge` 从 6 臂变 5 臂）。
02 先做会把一处未验证的重复写进新 module 的既有行为里——那正是本批要消灭的东西。

## 五、先冲掉的成本很低

`Retry` 臂**零测试覆盖**（全仓无一处能区分「两次写」与「一次写」；`TransitionTrigger::NodeRetry`
零断言；`FailureCause::TestIssue` 路径在测试面里不存在）——所以把非原子的那些路径收成一笔事务，
**没有既有测试会因此变红**。这一点同时也是风险：本次唯一的行为变化将无人见证，故票 02 要求先写
一条能看见 `Retry` 中间态的用例（改前后正反两次跑）。

`decisions.rs` / `scheduler/mod.rs` / `unstick.rs` **没有 `#[cfg(test)]` 模块**（三层里最外那层零 L1）。
`cursor_lifecycle.rs:902` 那个 `concurrent_writers_do_not_fail_with_busy_snapshot` 只钉了
`split_cursors`，够用不作废。

## 六、术语与文档

- `docs/glossary.md` 在 `### 存储相关` 与 `### 阶段流转` 之间加词条 **「推进（advance）」**。
- 三处**断言式**文档（重构后立刻变假，必须同批改）：
  `docs/implementation.md:148`（签名，且错收了 `&graph()`）、`implementation.md:177`
  （「没有任何其他代码路径写这个状态」）、`docs/testing.md:136`（「唯一写入路径 = `advance_cursor`」）。
- `docs/pipeline-spec.md:47/56/219/223` 四处是**叙述**，不影响正确性，**留待单独一轮**。
- **两处早已过期的决策编号**（既存错误，与本次改动无关）：`docs/README.md:18` 写「#1–#222」、
  root `AGENTS.md:5` 写「#1–243」——实际最大 244，本批新增 245。
