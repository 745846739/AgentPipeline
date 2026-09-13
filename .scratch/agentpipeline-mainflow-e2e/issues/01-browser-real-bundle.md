# 01: 浏览器 E2E 改走单二进制同源托管（真实产物）

**What to build:** 现有 `frontend/e2e/harness.ts` 起的装置是「真后端 + **Vite dev server**」，
浏览器加载的是 vite 开发态资源，API 经 vite 代理转发。而用户实际用的是 `make build` 出的
**单二进制**：前端 dist 在编译期内嵌（决策 155），axum 同源托管 `/` 与 `/assets/{*path}`，
零代理、零 CORS。两条路径的 JS 产物、CI 态、host 端口、相对路径解析都不同——**当前
playwright 从未加载过用户真正会加载的那份产物**。

本票把 harness 的浏览器入口切到内嵌产物：后端起的仍是同一个 `agent-pipeline serve --port 0`，
但页面地址改为后端自身 origin（回读到的 `apiBase`），不再起 vite。这样「浏览器加载真实产物」
本身就是被验的对象。

收益不止「换了个入口」：一旦走真实产物，`/` 是否返回内嵌 index、`/assets/{hash}.js` 的
Content-Type 是否被浏览器接受、index.html 里的 `src="/assets/..."` 绝对路径在同源下是否解析正确、
模块类型 script 是否执行——这些目前只有 HTTP 层断言（`api_contract.rs:2045-2115` 只断言
状态码与字节），没有被任何浏览器执行过。**「测试全绿、用户打开白屏」的典型来源正在这里。**

**Blocked by:** None

**Status:** done（2026-09-13）

- [x] `harness.ts` 去掉 vite 子进程与 `VITE_API_PROXY_TARGET`；`webBase` 直接等于 `apiBase`
- [x] `App` 接口保留 `apiBase` / `webBase` 两个字段（消费方不改），但两者同值并加注释说明原因
- [x] 浏览器入口改为后端 origin 后，断言页面**真的执行了前端 JS**：至少一项
      （如看板空态文案 / 顶栏 wordmark 由 JS 渲染而非 index.html 静态存在）；
      仅断言 `page.goto` 成功**不算**通过——那对白屏同样会绿
- [x] 断言静态资源加载无 404：监听 `response` 事件，任何 `/assets/*` 非 2xx 即失败
- [x] 断言无未捕获的页面错误（`pageerror` / console error），把「内嵌产物跑不起来」变成红
- [x] 前置产物守卫：harness 启动前检查 `EMBEDDED_ASSETS` 非空（可通过后端 `GET /` 返回体
      是否含 `/assets/` 判断），空则**明确报错并提示先跑 `make build`**，不允许静默退化成
      「前端未构建」提示页后继续跑出一个假绿
- [x] 两条既有用例（`happy-path.spec.ts` / `pending-resume.spec.ts`）在新入口下全过
- [x] 更新 `docs/testing.md` §9 的 harness 描述（从「Vite dev server 代理」改为「同源内嵌产物」）
- [x] `just frontend-e2e` 全绿

**注意（不得顺手弱化）：** vite dev server 是前端热更开发的正常路径，不要在 `vite.config.ts`
里删代理；本票只改 e2e harness 的启动方式。

**风险提示：** 若内嵌产物在浏览器里跑不起来，本票会立刻暴露既有缺陷——这是本票的价值而非障碍，
暴露后按缺陷修复并在票面记录（该缺陷此前被 dev-server 路径掩盖）。
