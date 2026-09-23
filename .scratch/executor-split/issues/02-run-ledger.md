# 02: run 台账 —— 记账收成一片 + 计时接 Clock（决策 249 · 第二片）

**What to build:** 把 run 台账的读写（`begin_run` / `finish_run` / `mark_step` /
`next_attempt` / 用量与续接记账）从 executor.rs 搬进新 module，并把**喂台账读数**的
`Instant::now()` 全部改读既有的 `Clock` 接缝（决策 249 Q2：搬移 + 依赖显式化随批做）。

**Blocked by:** 01（⑤ 的编排要经 01 的 interface 拼请求、经本片的 interface 记账——
先立两翼再合中路；且 01 是叶子，顺序已定 ①→③）。

**Status: ready-for-agent**

## 一、搬什么

| 职责 | 行段 | 说明 |
|---|---|---|
| run 行开立/收口/步标记/下次尝试 | 3243-3344 | `begin_run` / `finish_run` / `mark_step` / `next_attempt` |
| 用量与续接记账 | 同批散布 | `record_run_usage`、`link_run_continuation`（round==0 才链，违反过会静默少算 token）、`take_continuation` 一次性读清语义的调用点归位 |

**Clock 接缝落点**（喂 `duration_ms` 台账与 `NodeFinished` SSE 的取时点，
**本票一并改**，即使其所在行段此时尚未搬走——改的是取时点不是搬代码）：
`executor.rs:545, 604, 738, 884, 965, 1107, 1940, 2666, 3101` 与 `git.rs:159`
（worktree 锁等待）。`Clock` 已存在（`clock.rs`，Store 与 scheduler 都在用）——
Executor 加一个 `clock` 字段（`Executor::new` 是外部 6 触点之一，**签名不动**：
从既有 `Settings`/`Store` 侧取，或加 builder 方法，实现期定，语义冻结优先）。

**行为红线**（评审与决策 245 都点过名的承重顺序，搬运时逐条保住）：

1. `request_cancel` 必须在 `finish_run` **之后**（scheduler 侧注释「顺序是承重的」，
   否则 cancel 处理器的 `record_run_usage` 被 `RunOutcome::default()` 的 0 覆盖）；
2. `error.is_cancelled()` 分支：补用量、**不碰** status/error（executor 侧对偶）；
3. `link_run_continuation` 只在 `round == 0` 调一次；
4. `take_continuation` 读清语义：恰好一次、重试环之前——搬完这三条注释升级为
   module 的不变量（locality：顺序从散文变成结构）。

## 二、interface

```rust
// crates/core/src/pipeline/run_ledger.rs（票用名，落表随票 05）
pub struct RunLedger<'a> { store: &'a Store, clock: &'a dyn Clock }
impl RunLedger<'_> {
    pub async fn begin(...) -> Result<RunId>;
    pub async fn finish(outcome: RunOutcome) -> Result<()>;      // 含用量收口
    pub async fn mark_step(...);
    pub async fn record_usage(...);   // cancel 路径专用，不改终态
}
```

形状以实现为准；判据是⑤与留守核**不再各自拼 SQL 与顺序**，且计时读数只有一个来源。

## 三、验收

- **F 桶 8 条一条不删、断言不放宽**；其中 4 条续接测试（cause 是/否、干净重试、
  `continued_from_run_id` 不双算）今天就要跑，搬运后必须原样绿。
- **新增窄测试（只做加法）**：台账 module 直接测——`finish` 不抢已有终态、
  cancel 补用量不改状态、续接只链恰一条；**假时钟确定性测 duration**（Clock 接缝落地的
  见证——今天这些读数无法确定性驱动，这是本片的核心杠杆）。
- `make check` 绿；超时那条全栈用例（`a_timed_out_run_is_stopped_and_reports_its_usage`）
  原样绿。

**明确不做**：不动 `Store::finish_run` 等既有 SQL 的语义（搬调用不改储存层）；
不把 SSE 发射搬进来（观测面留守，照 245 先例）；不顺手改 token 口径（决策 61 系只读引用）。
