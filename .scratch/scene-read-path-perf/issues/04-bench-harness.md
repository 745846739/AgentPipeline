# 04: bench 设施——Playwright 首屏字节数闸门 + vitest 纯函数 bench

**What to build:** 本仓**零 bench 设施**（无 criterion、无 `[[bench]]`、无 `benches/`）。
本票引入最小可用的一套，只覆盖本批的回归面：**「有没有人把大载荷重新塞回首屏」**。

**关键取舍：可回归的指标是「首屏载荷字节数与请求数」，不是「秒数」。** 字节数是确定的
（不随网络抖动），可以断言；秒数在 CI 上不可断言，只当人工验收的读数。

两条腿：

1. **Playwright e2e（本地起服务）**：导航到任务详情，采集首屏期间的响应集合与总字节数。
   断言（闸门）：
   - 首屏响应总字节 **< 阈值**（阈值取 fixture 下的实测值 + 余量，写在测试里并注明来由）；
   - 首屏完成时 `/commands` **不参与**（拦截并挂住 `/commands`，断言时间线内容仍渲染）。
2. **vitest bench（纯函数层）**：`buildTaskScene` 在不同规模输入下的耗时，以及 reducer
   在 `liveDeltas` 累积下的追加/重算代价。**只记录不设闸**（秒数类指标）。
3. **106 上人工跑一次**，把秒数读数留在票面（对照 spec 里的基线：`/commands`
   1,330,415 字节 / TTFB 0.19 s / 客户端 total 8.5–10.9 s）。

**Blocked by:** None（闸门断言由本票提供，票 02 的验收挂在它上面）

**Status:** done（2026-10-01，决策 361）

## 落点

- `frontend/e2e/`（或既有 Playwright 目录，`npm run test:e2e` 已存在）：新增首屏载荷测试；
  起服务的夹具沿用仓库既有 e2e 的做法（`make check-e2e` / `.scratch/agentpipeline-e2e-mock`
  那套 mock 服务）。
- `frontend/src/lib/taskScene.bench.ts` / `frontend/src/realtime/reduce.bench.ts`：
  vitest 的 `bench` API（底层 tinybench）。
- `docs/testing.md` §10 的窄跑对照表里补上这两条的运行方式。

## 验收

- [ ] 闸门在**人为把 `getCommands` 挪回 `Promise.all`**（即回退票 02）时变红——本票必须
      验证这一点（把票 02 的改动临时回退，确认测试失败）
- [ ] vitest bench 有输出、可重复，且**不进** `make check`（bench 不进 CI 的断言集合）
- [ ] 106 上人工跑一次，读数记进本票的 Comments
- [ ] `make check` 全绿；`npm run test:e2e` 全绿

**明确不做**：`github-action-benchmark` 或任何趋势采集设施（本批回归面窄到一条断言就够）；
Rust criterion / iai-callgrind（store 层已证明不是瓶颈，见 spec「排查结论」）；
把秒数做成 CI 闸门（网络依赖，必然 flaky）。

**来源：** `.scratch/scene-read-path-perf/spec.md` 决议 4。

## 落地

- **Playwright 闸门**：`frontend/e2e/first-paint-budget.spec.ts`，两条真后端用例——
  ① 挂住 `/commands` → 时间线仍渲染（票 02 的判据）+ 首屏数据面响应（白名单全族：`/tasks` `/projects` `/providers` `/foreman` …）总字节 < **32,000**
  （实测基线 **11,658**：`/flow` 4,753 + 详情 2,564 + 会话摘要 2,296 + 看板 1,045 + `merge-proposal.diff` 505 +
  项目 307 + provider 173 + 值班长会话 15；预算取基线两倍出头，远小于任何一次「把 commands 塞回来」的 1.33 MB 起）；
  ② 挂住 `?include_messages=true` → 现场页签轮名牌先出、正文占位在场。
- **vitest bench**：`frontend/src/lib/taskScene.bench.ts`、`frontend/src/realtime/reduce.bench.ts`
  （`bench` API；`vite.config.ts` 加 `test.benchmark.include`，`package.json` 加 `"bench": "vitest bench --run"`）。
  本机读数（2026-10-01）：`buildTaskScene` 48 轮 / ~1.2k 消息 / 432 命令 **mean ≈0.5–0.6 ms**、
  四倍规模 **≈2.4–3.7 ms**（近似线性）；reducer 单条追加在已灌入 1 千 / 1 万 / 4 万 条时
  **≈0.004 / 0.04 / 0.04 ms**——后两档**持平**，正是决策 362 的 `LIVE_WINDOW_LIMIT = 10_000`
  环形缓冲真的封住了窗口的证据（这一档读数是 `.scratch/live-delta-retention/` 那条病的量级）。
- `docs/testing.md` §10 补「bench 两条腿」一段：手动运行方式 + **不进 `make check`**
  （`test.include` 只收 `src/**/*.test.ts`，`benchmark.include` 只收 `src/**/*.bench.ts`，两集合互不相交）。
- **牙齿检查（本票的硬要求）**：把 `getCommands` 挪回 `load()` 的 `Promise.all` → ① 在
  `.tline .trow` 那一步红（31.9 s 超时），恢复后绿。已实测。
- **106 人工读数**：待部署后补（spec 的基线：`/commands` 1,330,415 字节 / TTFB 0.19 s / 客户端 total 8.5–10.9 s）。
