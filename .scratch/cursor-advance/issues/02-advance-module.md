# 02: `pipeline/advance.rs` —— 推进收成一个深 module + resume 补 SSE

**What to build:** 把「落点 → 一串写」收成一个拥有事务的深 module，接入 executor 与 resume 两条
路。同时补上 resume 路径缺失的 SSE（本批唯一的用户可见行为变更），并落决策 **245**。

**Blocked by:** 01（01 的结论决定 `apply_edge` 是 5 臂还是 6 臂；先做会把一处未验证的重复写进
新 module 的既有行为里）

**Status: done（已实现）**

## 一、新 module

新建 `crates/core/src/pipeline/advance.rs`（在 `pipeline/mod.rs` 里照 `resume` / `landing` 的
姿态 re-export）。接口按经拷问定稿的形状：

```rust
/// 推进的落点。**不吃原因**——四种原因（EdgeKind / ResumeAction / 超时耗尽 / 用户裁决）
/// 在门外各自翻译成落点与修饰，门只回答「给定落点和修饰，一次事务写完」。
pub enum Landing {
    /// 串行下一阶段入口（goto / next 跨阶段 / judge-continue）。
    Entry(Stage, Node),
    /// 到 join 边界（决策 107）；`skipped` 置 `skipped_to_join`（决策 93）。
    JoinBoundary { skipped: bool },
    /// 挂起（决策 82）：pending 挂在游标上，不动落点。
    Pause { kind: PendingKind, context: Option<PendingContext>, message: String },
    // Split / Replace **不在这里**：它们已经是原子的 Store 方法
    // （split_cursors / replace_cursors_with_main，都走 begin_write），
    // 门只负责在同一笔事务里补 transition。
}

/// 一次推进的落库结果。**门不发 SSE、不做任务投影**——由调用方完成。
pub struct Advanced {
    /// 事务后受影响游标的新位置（调用方据此发 CursorChanged）。
    pub cursors: Vec<NodeCursor>,
    /// 是否走了「归档旧行 + 插新行」那条路（Split / Replace）。
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

**为什么不吃原因**：四种原因各有自己的校验（judge-continue 不接受 `Terminal`、goto 必须落在
`entry_node`、skip 在 merge 上非法）。门若吃原因就要把这些校验搬进来，门会重新长成第二个
`route()`。

**关键实现约束**：`Store::begin_write()` 是 `pub(crate)`（`storage/mod.rs:133`）而本 module 在
同一 crate，**用得上，不必改可见性**。`Store::pool()`（`:117`）/ `home()`（`:137`）/ `now()`（`:142`）
都是 `pub`，行级写可以在此直接下 SQL。要包事务的是**四条非原子路径**：
`Retry`、`Entry`（StageEntry）、`JoinBoundary`、`Pause`。

**`Split` / Replace 只补 transition**，**不把「游标 + 流转行」扩成原子对**——今天无人依赖它们
原子（`executor.rs:2893-2903` 现状就是两笔），扩了是新约束而非修 bug。

## 二、接入 executor

- `apply_edge` 的 `Retry` / `Next`-StageEntry / `Next`-JoinBoundary / `Pending` 四条臂改调
  `advance::advance`。`Retry` 是本票**唯一的行为变化点**：从 3 笔 autocommit 改成 1 笔。
- 删掉手抄的 pending 三连（`executor.rs:2800-2810` 的 `NodeOutput::Pending` 分支）——改走
  `pend_cursor_with_context`（`:436-472` 已有的深版本），消灭「一个文件里同时有深版本与手抄副本」。
- 顺带消冗余写：`skip_landing` 的 `Skip`、`goto` 的 `Goto`、`advance_after_judge_continue`
  三处在 `set_cursor_stage` 之后重复调 `reset_cursor_attempts`，而 `set_cursor_stage` 的 SQL
  本身已含 `validate_attempts = 0`（`cursors.rs:287`）。**并成门上的一个修饰位**——这是行为
  中性的 cleanup，不是修 bug。
- **先写一条能看见 `Retry` 中间态的用例**（改前跑一次看它红、改后看它绿）：在两个写之间读一次
  游标，断言行不走中间态。这是本次唯一行为变化的见证者——`Retry` 臂今天**零测试覆盖**，
  没有既有用例会因此变红。

## 三、接入 resume，并补 SSE

- `Store::apply_resume`（`storage/decisions.rs:249-403`）与 `advance_after_judge_continue`
  （`:619-639`）**从 storage 移除**——它是唯一住在 storage 却满口领域词汇的函数。
  `ResumeAction → 落点` 的翻译搬进已存在的 `pipeline/resume.rs`（票 08 抽出的 resume「唯一实现」：
  游标解析、允许集合、冷却、调 `store.apply_resume`）。storage 回到纯行级原语。
- **补 SSE**（本批唯一的用户可见行为变更）。今天 `decisions.rs` 里零 `emit`，`Store` 结构体
  （`storage/mod.rs:47-53`）没有 `sse` 字段，所以交互式 resume 完全静默。改法：`resume::apply_resume`
  拿 `&Arc<dyn SseSink>`（`SseSink` 在 `sse.rs:355`，`SseBus` 已 impl 它，`:359`），
  `apply_resume` 结束后照 executor 的既有形状发 `StageChanged` / `Pending` / `CursorChanged`。
- **两个调用方都要传**：`app/src/routes/tasks.rs:277`（界面那颗钮，`AppState` 已有
  `sse: Arc<SseBus>`，`state.rs:102`）与 `app/src/runtime.rs:120`（值班长托管自动动作）。
  补完之后 SSE 从「三处各发各的、其中一处不发」变成「两处照同一份契约发」。

## 四、决策 245

`docs/decisions.md` 追加 **#245**（只追加，编号接 244），记三件有语义的事：

1. 门吃**落点 + 修饰**而非原因（附理由：否则门会变成第二个 `route()`）。
2. **resume 路径补 SSE**——行为变更，必须有出处。
3. `ExecutionState` **不在本次**（补出真类型是独立候选）；文件落盘顺序**保持现状**。

并顺带修**两处早已过期的决策编号**（既存错误，非本批引入）：`docs/README.md:18` 写「#1–#222」、
root `AGENTS.md:5` 写「#1–243」——实际最大 244，本批新增 245。

## 五、术语与断言式文档

- `docs/glossary.md` 在 `### 存储相关` 与 `### 阶段流转` 之间加词条 **「推进（advance）」**。
- 三处**断言式**文档（本票之后立刻变假，必须同批改）：
  `docs/implementation.md:148`（那句签名还错收了一个 `&graph()` 参数）、`implementation.md:177`
  （「没有任何其他代码路径写这个状态」）、`docs/testing.md:136`（「唯一写入路径 = `advance_cursor`」）。
- `docs/pipeline-spec.md:47/56/219/223` 四处是**叙述**，不影响正确性，**留待单独一轮**。

## 验收

- [ ] `pipeline/advance.rs` 落地；`Landing` / `Advanced` / `advance` 三个名字就是上面这组
- [ ] `Retry` 中间态的用例先在旧代码上变红、改造后变绿（把两次运行的记录写进本票的交付说明）
- [ ] `Retry` / `Entry` / `JoinBoundary` / `Pause` 四条路径各只有**一笔**事务；
      `Split` / Replace 仍是既有原子 Store 方法 + 一笔 transition
- [ ] `Store::apply_resume` 与 `advance_after_judge_continue` 已从 storage 移除；
      `pipeline/resume.rs` 承担 `ResumeAction → 落点` 的翻译
- [ ] 交互式 resume 与托管 resume **都**发 `StageChanged` / `Pending` / `CursorChanged`；
      有一条测试断言「人按 continue 之后收到 StageChanged」（今天不存在，因为今天不发）
- [ ] 三处冗余 `reset_cursor_attempts` 已并成修饰位；既有 resume 用例（`apply_resume` 那 28 个
      调用点所在的测试）全绿
- [ ] 决策 #245 已追加；`docs/README.md:18` 与 root `AGENTS.md:5` 的编号已订正
- [ ] `glossary.md` 的词条已加；三处断言式文档已改
- [ ] `docs/implementation.md:148` 的 `&graph()` 已一并订正（该参数对应的是候选 3 的死镜像，
      本次不删 `graph.rs`，但文档不能再写它）

## 交付说明（2026-09-23）

**`Retry` 中间态用例的红 / 绿两次运行**（验收第 2 条）：

1. **红**（把 `Landing::Retry` 临时改回三笔 autocommit：先 `clear` + `increment` + `move`
   各自提交，再单独 `insert_transition`）——`cargo test -p agentpipeline-core --test
   integration a_failed_retry_leaves_the_cursor_untouched`：

   ```
   assertion `left == right` failed: attempts 不得在失败后已经 +1（说明两笔写不是一个事务）
     left: 1
    right: 0
   test result: FAILED. 0 passed; 1 failed
   ```

   造红的办法：`DROP TABLE kanban_transitions` 让**流转行**写不进去，游标行的两笔早已提交，
   于是 `validate_attempts` 停在 1、落点停在原处——正是设计里那个「半截」状态。

2. **绿**（恢复一笔 `BEGIN IMMEDIATE`）——同一条用例 `test result: ok. 1 passed; 0 failed`。

**与评审草案的两处出入（以实现为准，已同步进决策 245 与 glossary）**：

- `Landing` 多两个变体：`EntryWithAttempt`（决策 135 的 judge goto：归零后 +1、**恒为 1**，
  逐字复刻 `set_cursor_stage` + `increment` 的既有净效果）与 `Stay`（原地只离开 pending——
  resume 的 `dependency_failed` / `info_insufficient` / 兜底 continue 三条今天就不动落点，
  没有它就得借 `Entry` 落到自己身上并把 `validate_attempts` 归零，那是行为改变）。
- `Advanced.replaced` **不设**：`Split` / Replace 没进 `Landing`，恒为假的判据是死代码。

**`PendingReason` 整条进 `Landing::Pause`**（而非草案的 `kind/context/message` 三件）：
`NodeOutput::Pending` 里有构造者指定的 `(stage, node)`（`conflict_wait` 写死
`architect-design.execute`），由门按游标重算会悄悄改写这条待办的落点。

### 评审后补的三处（2026-09-23，code-review 两轴）

- **验收 5「都发 `StageChanged` / `Pending` / `CursorChanged`」只成立一半**：resume 发
  `StageChanged`（位置真挪了才发）与 `CursorChanged`（每条受影响游标都发），但**不发
  `Pending`**——resume 的五种落点没有一种会把游标挂起。`Pending` 由 executor / 调度器 /
  `unstick` 三处经同一扇门的 `Landing::Pause` 发出。`pressing_continue_emits_stage_changed`
  把这条口径也钉住了（断言 `Pending == 0`）。
- **`Advanced.cursors` 不再是死返回值**：`pipeline/resume.rs` 的 `apply_action` 直接拿门交
  回来的游标发事件，不再 `load_live_cursors` 读全表；只有分裂那种会改变**游标条数**的落点
  才重读（`split_via`）。
- **决策 107 / 113 是就地标注而非删除重写**：`decisions.md` 的「只追加」由文件头那句
  「**被修订的行保留原文不删**，修订点与代价见对应行的正文」细化——两行原文一字未动，
  只追加了「措辞面由决策 245 修订」的标注，与仓库既有惯例（如 10 / 19 / 26 行的
  「已由决策 N 修订」）同形。
