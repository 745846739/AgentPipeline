# 07: B5 任务 worktree 的构建产物回收——归档时清 `target/`,40G 盘不吃穿

**来源:** 同 01 的监控实录:该任务在 worktree 里 `cargo build` 出 1.4GB 的
`target/`(debug)+ 101MB node_modules,106 的 40G 盘两天内两次逼近告警线
(一度 3.5 分钟掉 740MB,82% 用量)。worktree 归档后产物没有确定清理路径——
这次是 develop 代理「恰好自己清了」,不是机制保证。磁盘恢复过程与盘面归因见
监控文件第三~七次巡检。

**Blocked by:** None

**Status:** todo

- [ ] 任务落终态(done/failed/cancelled 且无归档保留意图)时,清 worktree 的
      `target/`(debug 档全清;release 档在确认无部署引用后清)与 `.rows/` 类
      运行期产物;保留 `.scratch/` 产物与未提交改动的**清单**(只列不改)
- [ ] 清理动作落一条任务事件(清了什么、释放多少字节),失败不阻塞终态
- [ ] 盘压兜底:磁盘剩余低于阈值时,对**已终态**任务的 worktree 先下手
      (复用 `storage::io_budget` 的水位快照,那里已经看着 disk_free)
- [ ] 集成测试:造一个带 target/ 的终态任务,断言目录被清、`.scratch/` 还在、
      事件里有释放量

**边界.** 不动运行中任务的 worktree;不动 `/opt/AgentPipeline` 主仓的 target
(部署增量靠它);不做共享 target-dir/缓存服务(另一个设计题,等票 03/05 之后再议)。

## 现场欠账

被取消的 ux-audit-3 任务 worktree
(`/root/.agentpipeline/worktrees/01M3X472FJF8NW9082K6BFZPKC`)仍留有 ~1.5GB:
其中有价值的改动(e2e 修复)已抢救直推 main(90aec20),此 worktree 本体可按
本票机制删除——作为本票的第一次人工执行与验收样本。
