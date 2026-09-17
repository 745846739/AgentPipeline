# 20: 前端：看板与实时流

**What to build:** 按 design/frontend-design.md「夜间调度台」定稿实现 Svelte + TS + Vite 应用（决策 16）的地基与看板：任务列表（GET /tasks 过滤）、看板列归属（并行区间独立槽位 + 两分支药丸，决策 92）、SSE 订阅与 reducer（设计稿 §9.1 流式契约，决策 76/84）、pending/提醒/stalled 高亮。后端 API 已全部就绪，本票无阻塞。

**Blocked by:** None（can start immediately）

**Status:** done — `frontend/` Vite+Svelte5+TS 脚手架、18 令牌与 §3 字体/形状、8 列看板（双轨合并列 + 分支药丸 + 9 站迷你轨）、顶栏过滤与待办下拉、fetch 流式 SSE + reduce.ts（§9.1 逐事件）+ 退避重连/visibilitychange、NotificationPolicy（cooldown/quiet_hours/cancelled）落地；vitest 45 用例、svelte-check 0 错 0 警、vite build 全绿。playwright 两条冒烟按票注暂不跑（后端 E2E 基建属票 19/21）。

- [ ] Vite + Svelte + TS 脚手架，视觉令牌对齐 design/frontend-design.md §3.1——**无法判定（形态已被取代）**：
      脚手架仍在（`frontend/package.json` 仍是 Vite 6 + Svelte 5 + TS），但 §3.1 那套「夜间调度台」
      18 令牌已归档（`design/frontend-design.md:69` 明写 §3.1–§3.4 为历史记录，视觉唯一权威改为
      `design/theme-6-pixel.md` §2，决策 169），今日 token 是像素主题那套，旧令牌名已不存在
- [x] 看板列 + 并行槽位 + 分支药丸渲染（数据源 GET /tasks 的 branches 摘要）
- [x] SSE 接入 /tasks/{id}/stream，reducer 按设计稿 §9.1 事件表归约
- [x] pending / stalled 高亮与提醒 toast 骨架
- [x] vitest reducer 单测（testing.md §9 要求）
