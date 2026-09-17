# 01: 桌面壳不再吃随机端口——端口跨重启稳定，退让时说清

**来源**：2026-09-17 使用者实测报告——「每次桌面端重启都会换端口，导致之前扫码的手机保存的网址失效」。
取证：装在 `/Applications` 的实例当时监听 `TCP *:63378`（macOS 临时端口段），而它的库里
`kanban_server_bind = 0.0.0.0`（2026-09-16 写入）、`kanban_pairing_token` 自 2026-09-15 未变——
**设定与令牌都是跨重启有效的，唯一每开一次就变的是端口**。

**What to build:**

1. **桌面壳不再硬写 `port_override = Some(0)`**（`crates/desktop/src/main.rs`）。改用配置里的
   `[server] port`（缺省 8788，决策 171 的唯一事实源），于是二维码 URL、手机书签、桌面窗口地址
   跨重启都指着同一个号码。
2. **首选端口被占时退让而不是拒绝开窗**：`ServeOptions::port_fallback_to_ephemeral`（缺省 `false`，
   只有桌面壳打开）——`--port` 显式指定时仍 fail fast（「端口被占用或无法绑定」是唯一不需要猜的错误），
   桌面应用没有命令行，**打不开窗比端口变一次更糟**。退让**只在 `EADDRINUSE` 时发生**（按错误链判，
   不按报文判：`bind_listener` 的上下文同时罩着权限 / 地址不可用等不该退让的失败）。
3. **退让必须被说出来**：`/server-info` 新增 `port_source`（`startup` / `config` / `fallback`），
   「手机访问」页在 `fallback` 时说明「这次没绑上固定端口（被别的程序占着），当前是临时端口 N，
   手机上存过的网址要重新扫一次」——日志在桌面应用里看不到，不摆到页面上就等于没有痕迹。

**Blocked by:** None

**Status:** done

- [x] 不传 `--port` 时端口来自 `[server] port`，**重启后不变**（真二进制跑两次，`port_stability.rs`）
- [x] 首选端口被占 + 打开退让 → 换端口起来，且 `/server-info.port_source = fallback`（in-process
      `serve`，桌面壳那套参数）
- [x] 首选端口被占 + 未开退让 → 仍明确报错（`bind_preferred` 纯单测 + 既有 smoke 的端口占用用例）
- [x] `bind_listener` 的两条既有用例与 `smoke.rs` 的「端口占用应启动失败」全绿（默认姿态没被改）
- [x] 分享页在退让时说清（`lib/sharePairing.ts::portFallbackNote` + `Share.test.ts` 两条组件用例）
- [x] 决策日志追加 209（修订 153⑤ / 156 的桌面壳随机端口路线，不动 171 的数值）

**不做**：不给命令行加 `--port-fallback` 开关（只有桌面壳需要这个姿态，多一个旋钮多一种误用）；
不做「端口被占用时接管既有实例」（那是单实例锁与启动恢复的边界之外）。

## 交付

代码：`crates/desktop/src/main.rs`（`port_override: None` + 退让开关）、`crates/app/src/serve.rs`
（`bind_preferred` / `is_addr_in_use` / `ServeOptions::port_fallback_to_ephemeral`）、
`crates/app/src/state.rs`（`PortSource` + `AppState::port_source` / `with_port_source`）、
`crates/app/src/routes/server_info.rs`（上报 `port_source`）、`frontend/src/{api/types.ts,lib/sharePairing.ts,routes/Share.svelte}`。

用例：`crates/app/src/serve.rs` 单测 6 条（首选空闲用它 / 占用退让 / 占用 fail fast / 退让是 opt-in /
只认 `AddrInUse` / 三个串即契约）、`crates/app/tests/port_stability.rs` 2 条（重启不变、占用退让并上报）、
`crates/app/tests/api_contract.rs` 2 条（缺省 `config`、注入 `fallback`）、
`frontend/src/lib/sharePairing.test.ts` 5 条、`frontend/src/routes/Share.test.ts` 2 条。
