# 17: 生产进程接线

**What to build:** 让真实二进制真正「活」起来：main.rs 构造并启动 KanbanScheduler 的 10s run_loop 与小时级 maintenance、resume_hook 从空 no-op 换成真正 spawn 执行器（含冷却）、run_command 传播真实进程组 id 使超时杀进程组可用。

**Blocked by:** 11（执行器）、13（生产适配器）

**Status:** done

完成记录：`crates/app/src/runtime.rs` 接线 `KanbanScheduler.run_loop`（`tick_interval_sec`）+ 小时级 maintenance + 共享 shutdown watch（SIGINT 一次停全部，决策 54/55）；`main.rs` 组装 `ProductionLlm`/`Executor`/`RealProcessKiller` 并注入真实 `resume_hook`（review/merge-decision/resume 端点到执行器）；`run_command` 以独立进程组启动并回填真实 pgid（`process.rs` 新增 `spawn_in_own_process_group`，`CommandRecorder::set_process_group`，决策 66）。验收：`real_binary_advances_task_to_terminal_with_mock_llm`（真二进制 + mock LLM 到 done）、`run_command_records_real_process_group_id`。遗留：`run_command` 的流式输出 SSE（决策 100）仍未做（票面未要求）。

- [x] 启动即跑 run_loop（tick_interval_sec）与 maintenance 定时器；SIGINT 一并停掉（决策 54/55）
- [x] resume/review/merge-decision 端点经 resume_hook 真正驱动 executor；dependency_failed continue 例外不 spawn（决策 130⑤）
- [x] run_command 以进程组启动并回填真实 pgid 到 node_runs（当前 kill(0) no-op）
- [x] 真实二进制端到端：创建任务 → 自动推进到终态（接 mock LLM server 即可验收，不必真 LLM）
- [x] smoke 测试扩展：启动后 tick 循环在跑（可用假时钟或缩短间隔验证）
