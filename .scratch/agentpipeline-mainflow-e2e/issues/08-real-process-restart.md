# 08: 真进程重启后的恢复（spawn + 真 kill）

**What to build:** E2E-13 崩溃恢复**全部是 in-process 模拟**（`tests/e2e/tests/crash_recovery.rs:1-14`
头注明确写「不 spawn 真二进制做 kill -9」）：在 `executor.run` 的 LLM 调用处 `abort()`，
清理 `executor_owner` 残留，再重跑断言续跑。这覆盖了**游标检查点语义**，但没覆盖：

- 进程真的退出后，**启动恢复**（决策 127）在真实启动路径上是否执行（in-process 用手动
  `clear_executor_owners` 代替了启动时的自动清理）；
- 退出时**未优雅关闭**（kill，非 SIGINT）留下的中间态：worktree 残留、DB 在 WAL 中的未完成事务、
  `executor_owner` 泄漏、半写的任务产出文件；
- 重启后**服务端**能否正常起来（DB 可打开、迁移可跑、端口可绑）。

用户会关终端、会重启机器、会 `kill` 进程。这是「测试说能恢复、用户重启后任务卡死」的风险面。

**Blocked by:** 01

**Status:** done

- [x] 用例：真 spawn 二进制 → 任务推进中 **`SIGKILL` 子进程** → 断言进程确已退出。
      实现走票面**降级预案**的 Rust spawn 测试形态（Chromium 侧无需覆盖重启场景——
      刷新恢复已由票 07 的 reload 用例覆盖），故无 playwright timeout 调整项：
      `crates/app/tests/restart_recovery.rs::kill_9_mid_run_then_restart_recovers_to_done`
- [x] **重启**：同一个 `AGENTPIPELINE_HOME` 再次 spawn → 服务起来（就绪行 + `/metrics`）；
      任务被启动恢复接管（见缺陷记录：仅决策 127 清锁不够，须归队）→ 从游标续跑到
      merge_approval → 端点审批 → done
- [x] 断言「不重复劳动」：init run 行 == 1（上游不重跑）
- [x] **waiting_join 跨重启**：杀点选在 mock 已收到 `DevelopDesign.Execute` 请求的
      **并行分支窗口**（比票面建议的 develop.execute 更早、且可确定性观测——按 persona
      路由轮询 mock 请求记录），重启后 sync-check execute run 行 == 1（join 恰一次）
- [x] 未优雅退出无泄漏：done 后 worktree 已清理、`kanban/t1` 分支已删除（决策 3）
- [x] 分工已写进 `docs/testing.md`：in-process（E2E-13）验检查点语义；本用例验进程边界
      （真实启动路径的恢复 + kill -9 中间态）。**不推翻决策 152**（它针对 E2E-13 的实现
      方式），只补它未覆盖的进程边界
- [x] 决策 152：显式引用如上；新增用例是独立用例，未改 E2E-13
- [x] 用例时长：mock 下全流程 ~4s（杀点等待 + 重启 + 续跑），Rust 测试无超时上限问题；
      连跑 7 次全绿
- [x] 全量闸门绿（fmt / clippy / workspace tests / vitest / svelte-check / build）

**实现中暴露并修复的缺陷（决策 162）：**

1. **孤儿 running 任务重启后永久挂起**（用户可见：kill / 关机后重启，任务再也不动）。
   决策 127 只清 `executor_owner`，而调度器准入只认 `queued`。修复：启动恢复第二步
   `Store::requeue_running_tasks()`（serve.rs 在清锁后调用），归队任务由 tick 重新准入、
   从游标续跑。回归：`cursor_lifecycle.rs::startup_recovery_requeues_orphaned_running_tasks`
   + 本用例本身（修复前红：120s 超时，任务停在 running/architect-design）。
2. **testkit `MockLlm::from_script` 在 Submit 步后继续吐下一步**——真实模型提交元数据后
   返回无工具调用文本，agent loop 收束；原 mock 让节点一次 run 吃光整份脚本，重启重放
   必然「脚本已结束」。修复：Submit 后同节点下一请求回收尾文本（与真实 LLM 行为一致，
   smoke.rs 单跑语义不变）。

**注意事项（给后来者）：** 重启用例的脚本须压**两份**（`pipeline_script` 调两次 + 伪阶段
push 两次）：一次 run 恰消费一份，重放消费第二份。

**降级预案（若真实重启在 Chromium 自动化下不稳定）：** 至少落地「重启后的服务可用性 +
游标可继续」的非浏览器部分（可直接用 Rust spawn 测试），并把浏览器侧降级为
「重启后刷新界面状态正确」——但**必须在票面显式记录降级内容与原因**，不得静默缩减。
