# 06: 前置改动——服务端能认出请求从哪来

**What to build:** 服务在启动方式上做一处改造，使**处理器能拿到请求的来源地址**。这不改变任何
既有行为，全部既有闸门保持绿——它的唯一目的是让 07 判定得了「仅回环可读」。

**Blocked by:** None（可立即开始，与 01 并行）

**Status:** done（2026-09-17 回填：实现与断言均已落地，逐项核对见文末）

> 这是一张纯前置票（「先把改动变简单，再做简单的改动」）。它本身不交付用户可见行为。
> 如果嫌它太碎，可以并进 07——但并进去之后 07 就会同时承担「改服务构造」与「做令牌」两件事，
> 而前者会波及所有既有测试。

- [x] 服务启动方式改为可获取对端地址；既有接口与行为一律不变
- [x] 全部既有闸门（core 单元 / API 契约 / 前端 / e2e / lint）保持绿
- [x] 一条断言：测试里能观察到处理器拿到的来源地址

---

## 状态回填（2026-09-17）

闸门读数（本次实跑，非转抄）：`cargo fmt --all -- --check` 干净；`clippy --workspace --all-targets -- -D warnings`
干净；`cargo test --workspace` 全绿；前端 vitest **495**、`svelte-check` 0 错 0 警告、`vite build` 通过；
Playwright 离线全量 **83 passed / 15 skipped / 0 failed**。

逐项证据：

- 服务可拿到对端地址：`crates/app/src/serve.rs:61` 的 `into_serving_service()` 换成
  `into_make_service_with_connect_info::<SocketAddr>()`，归一中间件在 `crates/app/src/peer.rs:30`，
  注册于 `crates/app/src/lib.rs:176`。既有行为不变的承重设计是 `peer.rs:47` 的「缺 `ConnectInfo`
  时视为回环」——L3 的 tower oneshot 无连接信息时仍判本机，既有契约矩阵因此不破。
- 断言：`crates/app/src/peer.rs:112` 的 `handler_observes_the_peer_address_from_connect_info`。

