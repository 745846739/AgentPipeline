# 20: 前端：看板与实时流

**What to build:** 按 design/frontend-design.md「夜间调度台」定稿实现 Svelte + TS + Vite 应用（决策 16）的地基与看板：任务列表（GET /tasks 过滤）、看板列归属（并行区间独立槽位 + 两分支药丸，决策 92）、SSE 订阅与 reducer（设计稿 §9.1 流式契约，决策 76/84）、pending/提醒/stalled 高亮。后端 API 已全部就绪，本票无阻塞。

**Blocked by:** None（can start immediately）

**Status:** ready-for-agent

- [ ] Vite + Svelte + TS 脚手架，视觉令牌对齐 design/frontend-design.md §3.1
- [ ] 看板列 + 并行槽位 + 分支药丸渲染（数据源 GET /tasks 的 branches 摘要）
- [ ] SSE 接入 /tasks/{id}/stream，reducer 按设计稿 §9.1 事件表归约
- [ ] pending / stalled 高亮与提醒 toast 骨架
- [ ] vitest reducer 单测（testing.md §9 要求）
