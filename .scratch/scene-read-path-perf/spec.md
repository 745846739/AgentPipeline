# Spec: 现场 / 时间线的读路径瘦身（首屏载荷移出关键路径 + 响应压缩）

2026-10-01 grilling 决议（用户逐轮「同意」）。缘起是用户报障「看板任务阶段的现场、
时间线信息查询很慢」，进一步确认为**106 上打开任务详情后约 3~4 秒才出数据**
（手机 Safari，公网访问）。落点：决策 361（拟），票 01–07（分两批）。

## 排查结论（诊断底稿）

### 症状与实测

用户的原始描述是「打开不慢，但是等大概 3 到 4 秒才会出数据」——**骨架立刻出来、
数据要等**。在 106（`https://106.12.12.6:3389/`，应用自终止 TLS，配对令牌
`X-AgentPipeline-Token`）上带令牌实测（2026-10-01）：

| 端点 | 字节 | 服务器 TTFB | 客户端 total |
| --- | --- | --- | --- |
| `GET /tasks` | 1,391 | — | 0.14 s |
| `GET /tasks/{id}` | 1,820 | — | 0.14 s |
| `GET /tasks/{id}/flow` | 7,004 | — | 0.12 s |
| `GET /tasks/{id}/conversations`（摘要） | 829 | — | 0.13 s |
| **`GET /tasks/{id}/commands`** | **1,330,415** | **0.19 s** | **8.5–10.9 s** |

- 响应头只有 `content-type` + `content-length`，**无 `content-encoding`** —— 未压缩，
  在真实端点上确认（不只是读代码推断）。服务器**序列化只花 0.19 s**，其余全在搬字节。
- 双连接并行下载各 68 KB/s、合计 ~135 KB/s，是**整条管道的带宽上限**（带宽受限），
  不是单连接限速。测速对照：同一台机器到 `speed.cloudflare.com` 为 300–500 KB/s，
  故 ~129 KB/s 是这条链路的上限。
- 106 现场只有 **1 个任务** `01M3QW8CKS07R3MWG9XM4FNYER`（335 命令 / 23 会话）——
  上面这 1.33 MB 就是**中等任务的常态负担**，不是极端案例。

### 首屏为什么被它扣住

`frontend/src/stores/taskDetail.svelte.ts` 的 `load()`：

1. `await getTask(taskId)` → 头部 / 游标 / 动作先赋值（`:113-124`）——**用户看到的
   「打开不慢」就是这一步**；
2. `await Promise.all([getFlow, getConversations, getCommands])`（`:126-130`）；
3. **三个都回来之后**才一次性赋值 `transitions / conversations / commands`（`:132-135`）。

时间线的数据是 `transitions`（来自只有 7 KB 的 `/flow`），却被同一个 `Promise.all`
和 1.33 MB 的 commands 一起扣住 —— 默认页签（`timeline`，`frontend/src/routes/TaskDetail.svelte:40`）
因此一直空着，直到那 1.33 MB 搬完。这与「**任何时刻都慢、哪怕系统空闲**」的现象吻合：
它是纯传输，与服务端负载无关。

### 已排除（各有实测支撑）

- **查询形状**：`list_conversations` / `list_commands` / `list_runs` / `list_transitions`
  / `list_cursors` 全部走 `task_id` 前缀索引，本机副本上 `.timer` 实测 1.3–10 ms。
  与 `.scratch/storage-io-budget/spec.md` 的「查询形状无罪」一致。
- **DB 连接池排队**：106 上 2026-09-29（决策 321 落地）之后的日志里**慢语句 0 条、
  慢 acquire 0 条**（`journalctl -u agent-pipeline` 匹配 slow/elapsed/acquire/pool/timeout
  亦为 0）；机器自 2026-09-30 起无写者。用户也确认「系统空闲时同样慢」——空闲读只有
  1–10 ms，排队说不成立。（本机日志里确有 3.86 s 中位的慢语句，但止于 9-29 01:28，
  即修复之前，且属**另一台机器**。）
- **TLS / ALPN / h2 stall**：`crates/app/src/serve.rs:234-236` 的 `alpn_protocols()`
  只宣告 `http/1.1`（有守卫测试 `alpn_advertises_http11_only`），`Cargo.toml:18` 的
  axum 未开 `http2`；`--http1.1` 与默认实测结果一致，协商到的都是 h1，无 stall。
  `serve.rs:222-233` 那段「宣告了做不到的事」是历史复盘注释，**不是现状**。

### 两处更正（避免沿用错数字）

1. 本机 / 桌面库上那两个大任务（`01M2VSNJ…` 530 命令 / 2.8 MB、
   `01M3BGVC…` 48 轮 / 2.1 MB）**不在 106 上**（`/tasks/{id}` 返回 404）。它们是
   本机开发库的规模，引用时必须与 106 现场分开。
2. 「载荷搬运不是主因」这个初始假设（基于本机 localhost 3–35 ms）**只在本机成立**；
   在 106 的公网链路上，载荷搬运就是主因。

### 客户端侧的实测与推断

- `buildTaskScene`（`frontend/src/lib/taskScene.ts:357`）在本机最坏数据（48 轮 /
  1303 消息 / 433 命令）上全量跑一次 **3.3 ms**（`stepsFromMessages` 占 2.1 ms）——
  **落地数据的归约不是瓶颈**。
- 但它随 `liveDeltas` 累积近似线性上升（10 万条 ~54 ms），而 `liveDeltas` 在 reducer 里
  只增不减（`frontend/src/realtime/reduce.ts:384`，只有切任务才被 `emptyTaskDetailState()`
  清）。按真实单请求 36k completion tokens 估算尾部可到 130 ms+，事件以 ~50/s 到达。
  **这是另一条病**，不在本批（见「另立票」）。
- 手机链路比测量机快：1.33 MB 在 3~4 s 内搬完对应约 380 KB/s。**秒数会因链路而异，
  结论不依赖它**——「1.33 MB 未压缩载荷在首屏关键路径上」是实测事实。

## 决议

1. **压缩独立成第一张票**（票 01）：`tower-http` `CompressionLayer`（br 优先、gzip 回退）。
   这是**一处改动、全局受益**（所有 JSON 端点瘦身，含项目页 / 值班长页），预期把 1.33 MB
   压到约 1/5。**必须显式排除 SSE 路由**（`/tasks/{id}/stream`、值班长流），否则事件流会
   被压在缓冲里不发。
2. **commands 移出首屏关键路径**（票 02）：把 `getCommands` 从 `load()` 的 `Promise.all`
   挪出、改为**不阻塞首屏 + 后台补拉**后并入 `state.commands`。
   - 因此**不需要新增计数端点**：页签徽标（`frontend/src/routes/TaskDetail.svelte:534`）
     沿用补拉结果（初始短暂显示 0，随后更新）。
   - 同时给 preview 加**字节兜底**：保留 `head_tail(50, 100)` 的行数口径
     （`crates/core/src/agent/tools.rs:2304`），加 4 KB 字节上限，超出截断并留标记。
     现状是只按行数截，遇到超长单行就失控——本机库实测**单条 preview 最大 183,241 字符**，
     一个任务 preview 合计 2.4 MB，是该端点载荷的主体。
3. **批量会话 + 渐进填充**（票 03）：给会话读法加 `?include_messages=true`（加性参数，
   与决策 312 给 messages 定的口径一致），一趟取回该任务全部轮的完整会话；现场页签首屏
   先出轮名牌，正文到位再填。**不引入通用分页 / 游标**（见「明确不做」）。
4. **bench 设施**（票 04）：Playwright e2e（本地起服务）+ vitest bench（纯函数层）。
   - **真正可回归的指标是「首屏载荷字节数与请求数」，不是「秒数」**——字节数确定、不随
     网络抖动，可以直接断言；秒数只配当人工验收的读数。
   - **闸门 = 字节数断言**（票 02 的判据由它承载）；秒数只记录。
   - 两条腿：本地跑进 CI，106 上人工跑一次留读数。
   - 不引入 `github-action-benchmark`（见「明确不做」）。
5. **批二在批一于 106 上验收通过之后动工**（票 05–07）：HTTP/2、渲染守卫、ETag。
   拆两批的理由：**压缩和 h2 都要碰 TLS 与 SSE 这条路由**（h2 下 SSE 的流控与压缩行为
   要重新验），叠在一起出问题很难二分定位；而批一独立就能拿掉首屏那 1.33 MB，且可用
   同一组 curl 数字前后对照。
6. **渲染守卫只做到「对齐既有折叠模式」这一档**（票 06）：`SceneTimeline.svelte` 的五种
   可折叠步里，prompt / tool / command 用 `{#if ...Open[...]}` 守卫（展开才进 DOM），
   而**系统消息**（`:240-244`）与**思考步**（`:250-260`）的完整正文一直躺在 DOM 里，
   只被 `<details>` 视觉隐藏——而这两块恰是最长的文本（注释自陈系统段「常以万字计」、
   思考「常比回话长一个量级」）。补 `{#if}` 与另外三处对齐即可。
7. **流程**：本批走 `.scratch/scene-read-path-perf/` 全流程（本 spec + `issues/01..07`），
   收口追加决策 **361**。**计数不必同步**：起草时（10-01）`docs/decisions.md` 的 max 是
   360、`AGENTS.md:5` 写着 `#1–358`，但同一天 362–364 先落地，361 恰好是个空号——
   补上它之后 max 仍是 364，两处 `#1–364` 原样成立。动手前先 `make hooks`（决策 348）；
   各层窄跑按 `docs/testing.md` §10 选。

   **批一落地记录（2026-10-01）**：票 01–04 全部 `done`，本机全绿
   （`make check-lint` / core 与 app 的窄跑 / `npm test` / `npm run check` /
   `first-paint-budget.spec.ts` 两条）。**唯一未做的是 106 侧的验收**：票 01 的三条 curl、
   票 04 的秒数读数都要在部署之后跑（本批只在本机 L3 上验过行为，公网上的真实字节数还没读）。
   批二（05–07）按原计划，等批一在 106 上验收通过再动工。

## 与既有决策的关系

- **决策 319**（长列表窗口化，`:332`）：其「明确不做」写着「不动后端——本机单用户数据量
  有上界，后端分页等数据量证明必要再做」。本批**承认触发条件成立**（单任务 1.33 MB 单次
  载荷、106 实测 8.5–10.9 s），但**只做「批量取会话」这一项最小协议改动**，不引入通用
  分页——通用分页会与 319 / 349 的「前端切片」边界正面冲突，收益却与批量端点相当。
- **决策 312**（`:326`）：已给 messages 定过「500 缺省 + `before_id` 向上游标」，
  会话**列表**维持不分页。本批的 `include_messages` 是加性参数，不改列表口径。
- **决策 349 / 359 / 360**（`:362` / `:372` / `:373`）：现场时间线的形状与直播语义
  **不修订**。票 06 只改折叠体的 DOM 进出时机，不改任何归约判断。
- **决策 321**（`:334`）：本批**不修订**它「不做连接池扩容」的裁定（见「明确不做」），
  也不动决策 312 的 250 ms 在途刷写节拍。

## 明确不做

- **读池隔离 / 连接池扩容**：不是本批痛点（106 慢语句 0 条），且两者都要求修订决策 321
  的既有裁定，成本/风险对不上收益。真复发时另开票，并带上那时的水位读数。
- **通用分页 / 会话列表游标**：决策 319 的边界；106 只有 23 轮，窗口收益为零。
  `DEFAULT_PAGE = 50` 轮已覆盖观测到的最大规模（48 轮）。
- **场景时间线虚拟滚动、把 `DEFAULT_PAGE` 降到更小**：与决策 349 / 359 的「整条时间线
  摆得开、直播贴底、深链落点」语义冲突大，是三档渲染优化里收益最小的一档。
- **`github-action-benchmark` 趋势设施**：本批的回归面窄（「有人把 commands 塞回首屏」
  「批量端点被绕过」），一条字节数断言就能钉死；趋势设施要新增 CI 依赖与数据文件，
  等真有需要看趋势的指标再引。
- **`liveDeltas` 无界累积**：不在本批，另立票（见下）。

## 票

批一（传输；在 106 上用上面的基线验收）

- `issues/01-compression.md` —— `tower-http` CompressionLayer（br/gzip）+ 排除 SSE 路由
- `issues/02-commands-off-first-paint.md` —— commands 移出首屏 + preview 字节兜底
- `issues/03-batch-conversations.md` —— `include_messages` 批量读法 + 渐进填充
- `issues/04-bench-harness.md` —— Playwright e2e 字节数闸门 + vitest bench

批二（批一验收通过后动工）

- `issues/05-http2.md` —— axum `http2` + ALPN + 重验 SSE
- `issues/06-render-collapse-guards.md` —— 系统消息 / 思考步补 `{#if}` 守卫
- `issues/07-etag-304.md` —— 批量会话端点与 `GET /tasks/{id}` 的 ETag / 304

另立票（不在本批）

- `.scratch/live-delta-retention/issues/01-unbounded-live-deltas.md` —— `liveDeltas` /
  `liveTools` 只增不减（`Status: ready-for-agent`；丢弃语义已由决策 362 裁定，见其 `spec.md`）
