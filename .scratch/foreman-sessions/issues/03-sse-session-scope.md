# 03: 事件流带会话身份——两台设备同时说话不串台

**What to build:** `/foreman/stream` 今天把所有工头 delta 广播给所有订阅者，而 delta 里**没有任何能
区分会话的东西**：工头一律 `task_id: ""` / `branch: ""` / `run_id: 0` / `agent_type: "foreman"`
（`agent/providers/mod.rs:393-405`、`pipeline/foreman.rs:467-474`）。前端唯一的守卫是
`if (!sending) return`（`Talk.svelte:234-238`），而 `send()` 的 await 窗口里如果切了会话，
`settleForemanStream(stream, res.reply)` 会把旧会话的回话写进新会话的活动轮。

单机单窗口靠「发送中不许切」能挡住，但**手机和电脑同时连着**时（配对令牌正是为此存在）两台设备在
两个会话里同时说话就会串台。做法（加性、不改既有事件语义）：

- `RunContext` 加 `session_id: String`（流水线运行留空）；
- `SseEvent::ConversationDelta` 加一个 `#[serde(default)] session_id: String`；
- `emit_delta` 透传；`is_foreman_event`（`sse.rs:203-208`）不变；
- 前端 `realtime/foreman.ts::appendForemanDelta` 加会话守卫（事件里的 `session_id` 与当前会话不符
  则丢弃），`send()` 加一个 generation 记号挡住过期回包。

**不做每会话一条独立 SSE 路径**：服务端本来就分不出来，加路径解决不了问题。

**Blocked by:** 01、02

**Status:** done

- [x] delta 带 `session_id`；流水线的 delta 该字段为空串（`serde(default)`，老客户端不炸）
- [x] 前端在会话不符时丢弃 delta；切会话后旧回话不落进新会话的活动轮
      （单测钉 `appendForemanDelta` 的守卫，照 `realtime/foreman.test.ts` 的既有写法）
- [x] `send()` 的 await 窗口里切会话时，回包与 `reload()` 的结果都不落到新会话
- [x] 两个会话同时发话的取证：一条测试或 e2e，证明两边的增量各归各

## 交付

本票已落地（2026-09-17）。

- **增量的会话身份**：`RunContext.session_id`（流水线 / 子代理 / 伪阶段一律 `String::new()`，
  值班长给它的班次 id）、`SseEvent::ConversationDelta` 的 `session_id: String` 带
  `#[serde(default)]`、`emit_delta` 透传；`is_foreman_event` 不变。
  **加性改动有测试**：`conversation_delta_without_session_id_still_parses`（老客户端读到的事件照旧能解析）。
- **前端守卫**：`appendForemanDelta(state, event, sessionId)` 多了第三个参数——
  事件里的 `session_id` 与当前班次不符**或当前没有班次**时丢弃。两条新单测：
  班次守卫（别班 / 空串 / 没有当前班次 / 老事件无字段四种都丢）与「两个班次各说各的」。
- **`send()` 的世代记号**：`generation` 在「屏幕上换了班次」时 +1（切换 / 新建 /
  重取时发现服务端换了班），每个 await 之后比对；不符就**连乐观轮一起撤**（它属于已经不显示的那一班）。
  这条同时覆盖「另一台设备把当前班次归档了」那条被动路径——它不在「发送中禁止切换」那把 UI 锁的覆盖范围内。
- **不做每会话一条独立 SSE 路径**（服务端本来就分不出来）。
