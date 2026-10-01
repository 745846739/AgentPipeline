# 05: HTTP/2（axum http2 特性 + ALPN + 重验 SSE）

**What to build:** 目前 app **只讲 HTTP/1.1**：`crates/app/src/serve.rs:234-236` 的
`alpn_protocols()` 只宣告 `http/1.1`（有守卫测试 `alpn_advertises_http11_only`，
`:1086-1094`），`Cargo.toml:18` 的 axum 未开 `http2` feature。`serve.rs:222-233` 记着这段
历史：曾宣告过 h2 却讲不了，客户端按 h2 发就坏，故当时退回只讲 h1。

用户的主用场景是**手机 Safari 经公网访问 106**，h2 的收益是实打实的：多路复用后请求不再
受 HTTP/1.1 单源约 6 连接的约束，常驻的几条 SSE（详情页流、对讲台流）也不再各占一个 slot；
高 RTT 链路上省掉的是若干轮往返。

**Blocked by:** 01, 02, 03, 04（批一在 106 上验收通过）。理由见 spec 决议 5：压缩和 h2
都会碰 TLS 与 SSE 这条路由，h2 下 SSE 的流控与压缩交互要重新验，两件事叠在一起做，
出问题很难二分定位。

**Status:** ready-for-agent

## 落点

- `crates/app/Cargo.toml:18`：axum 加 `http2` feature。
- `crates/app/src/serve.rs:234-236`：`alpn_protocols()` 加 `h2`；`:1086-1094` 的守卫测试
  改成新预期（**保留**「宣告的能力必须真会讲」这条纪律——这正是当年那段注释的教训）。
- `crates/app/src/serve.rs:222-233`：把历史注释更新为新的现状（别再留一段与代码矛盾的
  「宣告了做不到的事」）。

## 验收

- [ ] `curl --http2 -k https://106.12.12.6:3389/` 的 `%{http_version}` 为 `2`（本地明文
      桌面端不受影响：浏览器不讲 h2c，仍是 h1——这一条要写进测试/注释，避免有人误以为
      本地也是 h2）
- [ ] **SSE over h2 仍逐条到达**：造持续产事件的场景，确认事件不是等缓冲/流控窗口才成批吐出
- [ ] **压缩与 h2 叠加**：票 01 的排除规则在 h2 下仍生效（流式响应不被压缩），其余端点
      仍带 `content-encoding`
- [ ] 守卫测试按新预期更新并通过；`make api` / `make check` 全绿
- [ ] 106 上实测一次首屏与现场页签的读数（与票 04 的基线同口径），记进 Comments

**明确不做**：h2c（明文 h2）——浏览器不支持，本地桌面端维持 h1；HTTP/3 / QUIC——超出本批。

**来源：** `.scratch/scene-read-path-perf/spec.md` 决议 5；现状见
`crates/app/src/serve.rs:222-236`、`Cargo.toml:18`。
