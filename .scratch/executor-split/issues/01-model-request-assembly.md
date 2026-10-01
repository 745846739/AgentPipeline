# 01: 模型请求组装 —— prompt 组装 + 上下文预算收成一片（决策 249 · 第一片）

**What to build:** 把 executor.rs 里的 prompt 组装与上下文预算调用搬进新 module
`crates/core/src/pipeline/model_request.rs`，接口按决策 249 定稿的杂交形状（丙为基
+ 甲 Overflow 臂带 Plan + 乙两条纪律）。本片是五片的第一片（叶子：调用方只有片③）。

**Blocked by:** 无（本批第一片）。**实现前置**：决策 248（候选 3，工作树上 `graph.rs`
删除等）收口提交后，从干净 HEAD 起实现——决策 249 的 Q6 裁定。

**Status:** done（已实现，决策 249：五片同批落地并过闸门）

## 一、搬什么（行段以 4223 行的 `executor.rs` 计）

| 职责 | 行段 | 去向 |
|---|---|---|
| system prompt 组装（基线/工作目录/AGENTS.md/persona/技能/格式规则 + golden 序） | 2191-2314、3834-4010 | 新 module |
| user prompt 组装 + 五追加段（gate_recheck / backtrack / retry / user-input / review…首轮为空不渲染） | 循环内 1194-1238、1497-1543 | 新 module |
| 上下文预算：`model_context_window`、L0 预估、L3 压缩触发、L4 判定 | 2325-2444 | 新 module |
| `tool_defs` 组装（deny 档摘除、未知工具 fail fast） | 3688-3833 内相关段 | 新 module（它与技能集同源，故随技能走） |

**不在本片**：伪阶段的 prompt 站点（1906/2049 附近，`project_analysis` 那条）——随票 03；
LLM 调用、重试循环、会话与快照**落库动作**——全是⑤的；`Instant::now() → Clock`——票 02。

## 二、interface（决策 249 定稿的杂交形状，以实现为准微调命名）

```rust
// crates/core/src/pipeline/model_request.rs（core 内部，无新 pub 出口要求）
pub struct AttemptCtx<'a> {          // ⑤ 手里本来就有的身份事实 + 仅有的两个环境依赖
    pub store: &'a Store, pub settings: &'a Settings,
    pub task: &'a Task, pub project: &'a Project,
    pub cursor: &'a NodeCursor,      // stage+node：段门与记账
    pub stage_cfg: Option<&'a StageConfig>,  // 结构进 interface（D 测试免造 DB 行）
    pub attempt: u32,
}

/// 甲的形状：两臂都带 plan——超限时先落原文再收口是形状保证，不是纪律（决策 180 退出路径条件）。
pub enum Prepared {
    Ready(RequestPlan),
    Overflow { plan: RequestPlan, facts: OverflowFacts },  // facts 不是 PendingReason
}

pub struct RequestPlan {            // 自持 String、无生命周期，⑤ 跨轮自由持有
    pub system: String, pub user: String,   // 逐字两段（决策 211②：原文是权威）
    pub hash: String,               // 对全文态技能正文敏感（决策 170）
    pub tools: Vec<ToolDef>,
    pub capacity: Option<ContextCapacity>,  // None=无 provider 跳过分档，不臆造窗口（决策 110）
    pub temperature: Option<f64>, pub max_tokens: Option<u32>, pub provider_id: Option<String>,
}

/// 每轮一扇、值不是 Pending——越界翻译照决策 245 留在 executor。
pub enum BudgetCheck {
    Ok { compacted: Option<usize> },        // L3 已就地压缩
    Overflow { estimate: usize, hard_limit: usize },
}

impl RequestPlan {
    pub async fn assemble(ctx: AttemptCtx<'_>) -> Result<Prepared>;   // ← 默认：常见路径这一行
    pub async fn assemble_with(ctx: AttemptCtx<'_>, segments: PromptSegments) -> Result<Prepared>;
    pub fn snapshot(&self) -> PromptSnapshot<'_>;
    pub fn check_budget(&self, messages: &mut Vec<Message>, carried_len: usize) -> BudgetCheck;
    pub fn request(&self, messages: &[Message], run: Option<RunContext>) -> LlmRequest;
}
```

**不变量（签名之外⑤必须知道的）**：

1. `assemble` 每 attempt 恰一次，`system/user/hash` 该 attempt 内冻结（prompt cache，
   §12.13.5）；重试轮**重新 assemble**（段与台账状态变了）——同一行调用，零段参数。
2. `carried_len` = 续接锚点（决策 180 压缩锚不认载入历史），调用方在进轮循环前取。
3. 任一门 `Overflow` → ⑤先落快照与会话行，再翻译成 `NodeOutput::Pending(context_overflow)`
   （落点由构造者定，决策 245「门吃落点不吃原因」），**永不继续循环**（决策 154）。
4. `capacity = None` → 跳过预算门，FakeAgent 路径恒 Ok。
5. **system 侧不开扩展轴**（乙的纪律：golden 序与 cache 是决策，封顶是有意的）。
6. `plan_custom` **不在本 interface**——触发条件：伪阶段/repair 真成为第二条调用方时
   单开票进（两条 adapter 才是真 seam，乙的纪律）。
7. `assemble_with` 带**可删条款**：实现期若测试全走写文件、无一用到，删。

**错误模式**：`Error::Config` = persona_path 不可读/空、全文态技能正文缺失、未知工具名、
provider 已解析但 `context_window == 0`（决策 110 显式 fail）；`Overflow` 是值不是错；
AGENTS.md 缺失、反馈文件缺失、无 provider、段不适用——都不是错。

**依赖与测面**：in-process（组装/hash/预算谓词纯函数）+ local-substitutable（fs 走
`AGENTPIPELINE_HOME` 临时目录、Store 走临时 SQLite）；**无 git、无 LLM、无 Executor，
不开 port**——internal seam 足够（依赖四分类里这片没到 remote）。

## 三、验收

- **既有 18 条（D13+E5）一条不删、断言不放宽**，继续全绿；两条 L2 pending 见证件
  （`context_overflow_ctx`、`context_overflow_path_writes_a_conversation_row`）原地不动——
  它们钉的是⑤对 Overflow 的**翻译**，刻意在本 interface 之外。
- **新增窄测试（只做加法）**：D 桶——golden 序、全文态换正文 hash 变/名字态不变、
  段首轮为空打回才渲染、节点级技能、persona_path/append、deny 档广告摘除、未知工具 fail fast；
  E 桶——Ok / Compacted / Overflow 边界与字段、`capacity=None` 恒 Ok、`carried_len` 锚。
  全部经 `AttemptCtx` + TestHome + 临时 SQLite，不建 Executor。
- `make check` 绿（决策 168）。

**明确不做**：不调 LLM；不构造 Pending；不开 `UserSegment` trait；不做跨 attempt 缓存；
伪阶段 prompt 站点不动（票 03）；`EdgeKind` 与路由不动。

## Comments

- 2026-09-23 实现落地：`crates/core/src/pipeline/model_request.rs`（interface 按 Q13 杂交形状，
  `assemble_with` 按可删条款**未落地**——窄测试全走写文件/建行取段，注释轴等第二调用方再开；
  `Prepared`/`BudgetCheck`/`OverflowFacts` 与 plan 补了 LlmRequest 身份戳和 `keep_recent_rounds`
  字段，微调条款内）。executor 侧：`agent_attempt_inner` 改走 `RequestPlan::assemble` +
  每轮 `check_budget`，两条越界汇进新私有方法 `context_overflow_exit`（翻译留编排侧，照 245）。
  既有 5 条随搬（名字不变），新增 13 条窄测试（D8+E5），core 全量 491 lib + 328 集成全绿；
  clippy / fmt 干净。未动：伪阶段 prompt 站点（随票 03）、Clock（随票 02）。
