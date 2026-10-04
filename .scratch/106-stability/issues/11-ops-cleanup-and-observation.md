# 11: 106 运维清理 + 顺延观察清单收口

**What to build:** 两件事合一。其一（立即）：把僵尸任务的残留清掉——cancelled
任务 01M3X472FJF8NW9082K6BFZPKC 的 worktree（实测 2.8G，盘 69%）删除、任务
归档。其二（挂到下一个自然任务后）：把散在各票据里的顺延观察项收成一张清单，
一次取数收口，每项写明命令级口径。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] 清理：01M3X472FJF8NW9082K6BFZPKC 的 worktree 删除、任务归档，盘回到
      <65%；决策 377② 的口径（保留 `.scratch/` 与未提交改动清单——该任务
      已判定无保留价值的改动，2026-10-02 监控记录在案）。
- [ ] e2e 两介质对账（票 runner-offload/07 残项）：下一个带自然 e2e 走查的
      任务里，确认 playwright 跑在日志流（票 01 的工具行）与台账两处可对上
      （同一次执行、同一量级）。
- [ ] 票 runner-offload/03 的两条 106 实测：40G 盘连续构建类任务后水位有界；
      「改码→可走查」增量构建不劣于改前基线（计时用票 01 工具收场行）。
- [ ] 票 runner-offload/01 的 IO 压力：部署后的新任务不因新增日志产生明显
      IO 压力（storage-io-budget 水位快照读数对照）。
- [ ] 决策 380 的缓存命中率复核：按其 findings 里的口径查一次
      `cache_read` 占比，**低于 ~85% 才回头查组装层**，读数写回
      `.scratch/106-stability/cache-findings.md`。

> 口径：以上观察项全部依附「下一个自然任务」，不专门造任务；本票收口时若
> 某项的自然任务还没出现，如实留勾并注明等待的是哪个形态的任务。
