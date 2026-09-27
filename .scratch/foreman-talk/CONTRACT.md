# 冻结接口（implement 阶段的事实源）

并行开工前定下，任何一方**不得擅自改名 / 改签名**。改这里 = 改全部下游。

## 0. 仓库约定（所有人必读）

- 每个 Bash 命令前置 `rtk`（RTK 技能）。
- **写完所有代码与用例再跑测试**（用户硬约束：编译成本太高）。最后统一 `make check-lint` →
  `make check-test` → `make check-frontend` → `make check-e2e`。
- 注释写**约束**（代码本身看不出来的东西），不写「这一行干什么」。
- 改动落在既有文件时，先读该文件顶部模块注释，保持同一姿态。
- 决策日志只追加；新决策号 **176**（语义裁决）与 **182**（实现设计）。

## 1. 身份常量（`crates/core/src/pipeline/foreman.rs`）

```rust
pub const FOREMAN_STAGE_KEY: &str = "foreman";   // stage_configs.stage 的第 4 个伪键
pub const FOREMAN_AGENT_TYPE: &str = "foreman";  // RunContext.agent_type / SSE 载荷
pub const FOREMAN_TOOLS: [&str; 2] = ["read_task", "read_conversation"];
pub const FOREMAN_MAX_ROUNDS: usize = 8;
pub const FOREMAN_HISTORY_BUDGET_CHARS: usize = 24_000;
pub const FOREMAN_PERSONA: &str = "…";
```

同一份常量在**后端**（`app/src/routes/stage_configs.rs::PSEUDO_STAGE_KEYS` → 追加
`"foreman"`）与**前端**（`frontend/src/lib/stageConfigs.ts` 的 `STAGE_KEYS` / `PSEUDO_KEYS`）
各有一份镜像，两处都要改（票 01 验收项）。

## 2. 存储（`crates/core/src/storage/foreman.rs`）

迁移 `0005_foreman_messages.sql`（**定稿后立即 `git add`**，当前未跟踪）：

```sql
CREATE TABLE IF NOT EXISTS kanban_foreman_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role TEXT NOT NULL,                -- user | assistant
    content TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    briefing_json TEXT,                -- 该轮注入的夜班态势快照（审计）
    traces_json TEXT,                  -- 该轮的工具痕迹（票 05）
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_foreman_messages_id ON kanban_foreman_messages(id);
```

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ForemanMessage {
    pub id: i64,
    pub session_id: String,         // 会话隔离的必填入口参数（决策 204）
    pub role: String,               // "user" | "assistant"
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub briefing_json: Option<serde_json::Value>,
    pub traces_json: Option<serde_json::Value>,
    pub status: Option<String>,     // null=收口行 | "in_flight"=半截行 | "interrupted"（迁移 0036，票 01/03）
    pub seq: i64,                   // 行内位置序号：只对在途行有意义，存量行与收口行恒 0（票 02）
    pub interrupted_at: Option<chrono::DateTime<Utc>>, // 仅 status="interrupted" 行（票 03）
    pub created_at: chrono::DateTime<Utc>,
}

/// 在途行的节流刷写载荷（票 01/02）：整体写、不增量拼；`seq` 只记**已进现场**的位置
/// （广播了但还没落库的坐标不计，否则快照会声称覆盖了它其实没有的字）。
pub struct InFlightPatch {
    pub content: String,
    pub thinking: Option<String>,
    pub segments_json: Option<serde_json::Value>,
    pub traces_json: Option<serde_json::Value>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub seq: u64,
}


#[derive(Debug, Clone)]
pub struct NewForemanMessage {
    pub role: String,
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub briefing_json: Option<serde_json::Value>,
    pub traces_json: Option<serde_json::Value>,
}

impl Store {
    pub async fn append_foreman_message(&self, msg: NewForemanMessage) -> Result<i64>;
    /// 在途半截行的四个动作（票 01）：建（`status='in_flight'`）/ 节流刷（谓词只认在途行，
    /// 迟到的刷写改不动终态）/ 收口（同一行写成完整行、`status→NULL`）/ 丢弃（失败轮不落行）。
    pub async fn begin_foreman_inflight(&self, session_id: &str,
        briefing_json: Option<serde_json::Value>) -> Result<i64>;
    pub async fn update_foreman_inflight(&self, row_id: i64, patch: &InFlightPatch) -> Result<()>;
    /// 收口：同一行写成完整行、`status→NULL`，并把这一轮**已进现场**的位置写进 `seq`
    /// （前端拼接的 `seq0`）。失败 / 静默的轮走 `discard`，不落行。
    pub async fn close_foreman_inflight(&self, row_id: i64, msg: NewForemanMessage,
        seq: u64) -> Result<i64>;
    pub async fn discard_foreman_inflight(&self, row_id: i64) -> Result<()>;
    /// 启动恢复（票 03）：把没有活跃轮对应的悬挂在途行标成 `interrupted` + 中断时刻
    /// ——与 `orphan_inflight_model_requests` 同姿势，排在 READY 之前。返回标了几行。
    pub async fn mark_orphan_foreman_inflights(&self) -> Result<u64>;
    /// 最近 `limit` 条，按 id **升序**返回（时间线顺序）。`limit = 0` → 空表。
    /// `before_id`（票 05）：给了就只取**更早的一段**（`id < before_id` 里最新 `limit` 条），
    /// 到头返回空表——500 条缺省语义一个字不变（显式修订读接口「不分页」立场）。
    pub async fn list_foreman_messages(&self, session_id: &str, limit: usize,
        before_id: Option<i64>) -> Result<Vec<ForemanMessage>>;
    /// 本会话合计 `(total_tokens, total_calls)`；calls = assistant 行数。
    pub async fn foreman_session_totals(&self) -> Result<(u64, u64)>;
    // 对话消息**没有**保留期清理（票 04，显式修订 182④「同一把保留期尺」与 204⑦
    // 「归档不保护消息」）：`purge_foreman_messages` 已删除，消息表永久保留；
    // 维护作业里提议 / 待办 / 终态会话 / worktree 照旧吃同一个 cutoff。
}
```

## 3. 工头模块（`crates/core/src/pipeline/foreman.rs`）

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForemanBriefing {
    pub pending: Vec<BriefingPending>,
    pub running: Vec<BriefingRunning>,
    pub failed: Vec<BriefingFailed>,
    pub projects: Vec<BriefingProject>,
    pub done_count: usize,
}
pub struct BriefingPending { pub task_id: String, pub title: String, pub stage: String,
                             pub kind: String, pub message: String }   // message = pending 原因**原文**
pub struct BriefingRunning { pub task_id: String, pub title: String, pub stage: String }
pub struct BriefingFailed  { pub task_id: String, pub title: String, pub stage: String,
                             pub message: Option<String> }
pub struct BriefingProject { pub id: String, pub name: String }

impl ForemanBriefing {
    /// 确定性渲染进 prompt 的文本（空 home 给出「空班」句，不报错）。
    pub fn render(&self) -> String;
}
pub async fn build_briefing(store: &Store) -> Result<ForemanBriefing>;

/// 历史窗口按**字符预算**裁剪：从最新往回取，返回选中项（时间升序）。
pub fn trim_history(history: &[ForemanMessage], budget_chars: usize) -> Vec<ForemanMessage>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForemanTrace { pub tool: String, pub args_summary: String, pub ok: bool }

#[derive(Debug, Clone)]
pub struct ForemanTurn {
    pub reply: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub briefing: ForemanBriefing,
    pub traces: Vec<ForemanTrace>,
}

pub struct ForemanRunner { /* store, settings, home, llm */ }
impl ForemanRunner {
    pub fn new(store: Store, settings: Settings, home: Home, llm: Arc<dyn LlmClient>) -> Self;
    /// 一次回话：写 user 行 + **建在途 assistant 行** → 循环（每步**边广播边落库**）→ **收口**。
    pub async fn say(&self, user_text: &str) -> Result<ForemanTurn>;
}
```

**一轮的写入顺序**（`.scratch/talk-replay` 票 01，**显式修订**「回话落库才算数」的旧口径）：

1. `say` 写 user 行（不变）；`respond` 这个唯一漏斗登记「有一轮在跑」（决策 260 不变）；
2. 历史读完、请求组装完之后**建在途 assistant 行**（`status = 'in_flight'`，`content` 空）——
   这一轮的历史因此一个字不变（在途行不进上下文）；
3. 循环里每次模型调用的边界（段序 / 痕迹 / 推理 / token）与流式途中（逐字正文、推理
   增量，随 `conversation_delta` 广播、**250ms 一拍**节流）刷进这一行；
4. **收口**：同一行写成完整行（`status → NULL`）——台账终是「一次回话 = user 行 +
   assistant 行」两行，与从前逐字一致；失败 / 静默的轮**丢弃**这行（今天它们库里本来
   就没有回话行，失败账由 `system` 行承载，决策 211④ 不变）。

读侧配套：`GET /foreman/session` 的 `messages` 在飞时含半截行（`status` 字段随之下发）；
喂给模型的历史、跨时间线摘要与归因回看都**滤掉在途行**。`turn_in_flight` 仍是进程内
登记的直接读数（spec 决策 7：它答「此刻有没有轮在跑」，台账答「此前发生了什么」）。

`say()` 的 LLM 请求固定填：`stage = Stage::Init`（占位，让既有解析链跑通）、
`node = Node::Execute`、`attempt = 1`、`provider_id` 由 `FOREMAN_STAGE_KEY` 读到的
`StageConfig` 经 `resolve_provider_id(None, None, stage_cfg, None)` 得出（**不配置也能用**：
阶段配置缺行时 `None` → 适配器回落首个启用 provider）、
`run = Some(RunContext { task_id: String::new(), branch: String::new(), run_id: 0,
agent_type: "foreman".into() })`。

## 4. 工具（`crates/core/src/agent/tools.rs`）

新增两个只读台账工具，**并入既有分发**（不是另起一套）：

```rust
/// 注入台账读句柄（工头的 read_task / read_conversation 用）。未注入 → 两工具报错不可用。
pub fn with_ledger(mut self, store: Store) -> Self;
```

`execute` 的两个新 match 臂：

- `"read_task"` → 参数 `{ "task_id": String }`。返回 JSON：id / title / description /
  status / current_stage / current_node / pending_reason（**含 message 原文**）/
  allowed_actions / cursors[{branch,stage,node,status}]。
  任务不存在 → `Err(Validation(...))`（由调用方转成 tool_result 文本回灌，别上升为失败）。
- `"read_conversation"` → 参数 `{ "task_id": String, "run_id": Option<i64> }`。
  `run_id` 缺省取该任务**最近一次**会话。返回 JSON：stage / node / attempt / agent_type /
  token 数 / messages（截到最后 20 条、总字符 ≤ 12_000）。

**白名单在执行点强制**：工头用 `ToolExecutor::with_allowed_tools(&FOREMAN_TOOLS)`，
越权调用的拒绝来自 `execute` 开头那段既有白名单检查（与只读子代理同一处）。
工具集里**没有** `read_file` / `list_dir` / `run_command`（票 02 的硬约束）。

## 5. 事件（`crates/core/src/sse.rs`）

**不新增变体**。工头增量走既有 `SseEvent::ConversationDelta`，带
`task_id = ""`、`branch = ""`、`run_id = 0`、`agent_type = "foreman"`。

新增一个**只读**判定（供路由过滤，不改既有语义）：

```rust
impl SseEvent {
    /// 面向人的对话事件（工头对讲台用）。既有任务级路由按 task_id 精确匹配 → 零干扰。
    pub fn is_foreman_event(&self) -> bool;
}
```

## 6. HTTP 端点（`crates/app/src/routes/foreman.rs`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/foreman/session` | 查询 `?session=<id>&kind=<talk\|watch>&before_id=<id>`（票 05：`before_id` = 向上游标，只取更早一段；缺省仍是最近 500 条）→ `{ messages: ForemanMessageWire[], total_tokens, total_calls }`；`messages` 在飞时含半截行 |
| GET | `/foreman/sessions` | 查询 `?kind=<talk\|watch>&include_archived=<bool>`（票 06：`include_archived` 打开时归档班次也在列；缺省与从前逐字一致） |
| POST | `/foreman/messages` | 体 `{ text }` → `{ message, total_tokens, total_calls }`；LLM 失败 → 错误状态 + `{error}`，**user 行已落库** |
| GET | `/foreman/stream` | SSE，只转发 `event.is_foreman_event()` 的事件；复用同一 `SseBus`（在途事件带 `ledger_id` / `seq`，票 02） |

`ForemanMessageWire`：`{ id, role, content, prompt_tokens, completion_tokens,
briefing, traces, status, seq, interrupted_at, created_at }`（`briefing`/`traces`/
`status`/`interrupted_at` 可为 null；`seq` 只对在途行有意义，其余为 0）。

`AppState` 新增：

```rust
pub foreman: Option<Arc<ForemanRunner>>,
pub fn with_foreman(mut self, runner: Arc<ForemanRunner>) -> Self;
```

生产在 `serve.rs` 里按 `ProductionLlm` 注入；契约测试注入 `FakeAgent`。缺省 `None`
时三个端点返回 503「工头未接线」，**不是** 500。

## 7. 对端地址（票 06，`crates/app/src/peer.rs`）

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub std::net::SocketAddr);
impl PeerAddr { pub fn is_loopback(&self) -> bool; }

/// 中间件：把 axum 的 ConnectInfo 归一成 PeerAddr 扩展（缺省不插入）。
pub async fn peer_address(request: Request, next: Next) -> Response;

/// 处理器的唯一读法：**缺省视为回环**——本改动之前一切都隐含是本机，缺省必须保持不变，
/// 否则所有既有契约测试（tower oneshot，无 ConnectInfo）会集体变红。
pub fn peer_is_loopback(extensions: &http::Extensions) -> bool;
```

`serve.rs` 改用 `router.into_make_service_with_connect_info::<SocketAddr>()`。
契约测试通过 `req.extensions_mut().insert(ConnectInfo(addr))` 模拟局域网客户端。

## 8. 配对令牌（票 07）

- 迁移 `0007_pairing_token.sql`：单行表 `kanban_pairing_token(id INTEGER PRIMARY KEY
  CHECK(id = 1), token TEXT NOT NULL, created_at TEXT NOT NULL)`。
- `Store::pairing_token(&self) -> Result<String>`（无行则生成并持久化——**长期有效**，
  不随启动重生成）、`Store::reset_pairing_token(&self) -> Result<String>`。
- `AppState::lan_mode()`：`!is_loopback_bind(&self.bind_host)`（`server_info.rs` 已有该判定，
  提到 `peer.rs` 或复用）。
- **只在 `lan_mode()` 下生效**的中间件 `pairing_guard`：拦**所有非 GET/HEAD/OPTIONS**
  ＋**所有 `/foreman/*`**。请求头 `X-AgentPipeline-Token`。只读 GET（看板 / 会话 / 指标 /
  分享页）不护。
- `GET /pairing/token`：**仅回环可读**（`peer_is_loopback` 为假 → 403）。
- `POST /pairing/reset`：重生成，走配对令牌校验（回环豁免）。
- 二维码放宽：`server_info.rs` 的 `allowed_qr_urls` 精确匹配改为「**origin 落在白名单内，
  允许追加 query**」。
- 前端：`?pair=` → localStorage → 从地址栏抹掉 → 之后作为 `X-AgentPipeline-Token` 头带上；
  403 → 提示「这台设备还没配对」。

## 9. 前端（`frontend/src/routes/Talk.svelte`）

- `frontend/src/api/types.ts`：`ForemanBriefing` / `ForemanTrace` / `ForemanMessage` /
  `ForemanSession`。
- `frontend/src/api/client.ts`：`getForemanSession(signal?)`、`sendForemanMessage(text)`。
- `frontend/src/realtime/connection.ts`：`TaskStream` 增加可选 `path` 覆盖（默认
  `/tasks/{id}/stream`），工头流走 `/foreman/stream`。不新造一套 SSE 解析。
- 版面**三分区**（票 04）：状态区（急停轮 + 值班板，钉第一屏、不随时间线滚动）／
  对话时间线（发言 + 回执）／输入坞（钉底）。
- **值班长的回复里永远没有按钮**；写动作只在状态区的急停轮里渲染。
- 空态的「去看板新建任务」是页面固定的**导航钮**，不进后端动作契约。
- 不新增色彩 / 字体 / sprite / 动画位；不新增 token 块外裸色值
  （`theme/css-parity.test.ts` 会扫）。
