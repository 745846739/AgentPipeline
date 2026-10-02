# 106-stability:ux-audit-3 监控事故的整改票组

证据基线与全部现场记录见 `.scratch/monitor-01M3X472FJF8NW9082K6BFZPKC.md`
(2026-10-02 对 106 上任务 01M3X472FJF8NW9082K6BFZPKC 的 7.6 小时监控实录:
三次 HTTP 挂死、计数冻结与倒退、超时续接梯子四连、7.6 小时零交付)。
决策 374–377 记录了 grilling 两轮收敛后的全部裁决。

| 票 | 内容 | 决策 | 状态 |
|---|---|---|---|
| 01 | B9:TLS accept 并发化 + 握手超时 | 374 | done |
| 02 | B3/B6:计数真实账 + 滚动入账 | 375 | done |
| 03 | O4-A:压缩硬底(转录体量兜底) | 376 | todo |
| 04 | O4-B:续接简报化 | 376 | todo(blocked by 03) |
| 05 | O4-C:prompt cache 验证实验 | 376 | todo |
| 06 | B1:截断救援显性化 + UTF-8 分块修复 | 377 | todo |
| 07 | B5:worktree 构建产物回收 | 377 | todo |
| 08 | 重建 ux-audit-3 任务 | 376 | todo(blocked by 03) |
