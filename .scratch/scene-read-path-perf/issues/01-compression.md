# 01: 响应压缩（tower-http CompressionLayer，排除 SSE 路由）

**What to build:** 给 app 的 HTTP 层加 `tower-http` 的 `CompressionLayer`（br 优先、
gzip 回退，按 `Accept-Encoding` 协商）。本仓此前**完全没有压缩**（`Cargo.toml` 无
`tower-http`，`crates/app/src/` 无任何 `CompressionLayer`），106 上 `/tasks/{id}/commands`
的 1,330,415 字节是原样过网的（响应头只有 `content-type` + `content-length`，实测无
`content-encoding`）。这是本批性价比最高的一刀：一处改动、**全局受益**（详情页、项目页、
值班长页、看板列表全部瘦身）。

**必须显式排除 SSE 路由**：`GET /tasks/{id}/stream`（与值班长的流），否则事件会被压在
压缩缓冲里不发、直播看起来「卡住」。压缩层要在路由器之上、且对流式响应不生效。

**Blocked by:** None

**Status:** done（2026-10-01，决策 361；106 上的 curl 验收待部署后补做）

## 落点

- `crates/app/Cargo.toml`：加 `tower-http`（feature `compression-br` / `compression-gzip`）。
- `crates/app/src/serve.rs`：在构造服务的那一层加压缩层（`serve.rs:335-339` 附近，
  `axum::serve(listener, into_serving_service(router))` 的上游），保证对 TLS 与明文两条
  路径都生效。
- SSE 路由定义在 `crates/app/src/lib.rs:53`，其余路由 `:82-97`——排除要按路由而不是按
  「路径前缀猜」。

## 验收

- [ ] `curl -k -H "X-AgentPipeline-Token: <token>" -H 'Accept-Encoding: br' -w '%{size_download} %{time_total}\n' https://106.12.12.6:3389/tasks/01M3QW8CKS07R3MWG9XM4FNYER/commands`
      的 `size_download` 降到 **≤ 1,330,415 的 1/5**（即 ≤ 约 266 KB），响应头出现 `content-encoding: br`
- [ ] 同一命令带 `Accept-Encoding: gzip` 也生效（回退路径）
- [ ] 不带 `Accept-Encoding` 时仍返回未压缩响应（不破坏既有客户端与测试断言）
- [ ] **SSE 不被压缩**：`/tasks/{id}/stream` 的 `content-encoding` 为空，且事件仍**逐条**
      到达（造一个持续产事件的场景，确认不是等缓冲满才一次性吐出）——这条要有测试钉住
- [ ] 既有 `make api` 契约测试全绿（响应体语义不变）

**明确不做**：静态资源（前端 bundle）的预压缩文件（`.br`/`.gz` 落盘）——运行时压缩已覆盖，
且本批的痛点在 JSON 端点，壳的加载用户已确认不慢。

**来源：** `.scratch/scene-read-path-perf/spec.md` 决议 1；106 实测
`/commands` 1,330,415 字节、TTFB 0.19 s、客户端 total 8.5–10.9 s、无 `content-encoding`。

## 落地

- `tower-http 0.6`（feature `compression-br` / `compression-gzip`）加进 workspace 与 `crates/app`；
  `crates/app/src/lib.rs` 新增 `compression_layer()`，挂在整个 router 的**最外层**
  （明文与 TLS 两条路都走它，两者都经 `build_router`）。
- **SSE 的排除方式**：按**响应 content-type**（`text/event-stream`）判，不按路由名单——
  名单只认当下这两个端点，而「事件流不压」这条不变式属于响应本身（将来新加的流自动落在正确一侧）。
  `NotForContentType::SSE` 在 `DefaultPredicate` 之外**显式写出来**，理由见 `compression_layer()` 的 doc
  （承重行为不寄托在第三方默认值上，同 `skill_import` 不把路径穿越判定外包给 zip）。
- 测试（`crates/app/tests/integration/api_contract.rs`）：
  `json_endpoint_negotiates_br_then_gzip_and_stays_plain_without_accept_encoding`（br / gzip 都
  **解回来逐字节比**，不是只看头；另含「无 `Accept-Encoding`」「只收 `deflate`」「q=0 显式拒绝」三条负例）、
  `sse_is_never_compressed`（任务流无 `content-encoding` + 心跳帧仍按时到达）、
  `quiet_foreman_stream_receives_keepalive_comment_frame` 补上值班长流的同款断言。
- **待补**：106 上那三条 curl（`size_download` ≤ 1/5、gzip 回退、无头不压）要在部署之后跑——
  本批只在本机 L3 上验过行为，公网上的**真实字节数**还没读。
