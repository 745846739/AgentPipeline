# 17: 生产进程接线

**What to build:** 让真实二进制真正「活」起来：main.rs 构造并启动 KanbanScheduler 的 10s run_loop 与小时级 maintenance、resume_hook 从空 no-op 换成真正 spawn 执行器（含冷却）、run_command 传播真实进程组 id 使超时杀进程组可用。

**Blocked by:** 11（执行器）、13（生产适配器）

**Status:** ready-for-agent

- [ ] 启动即跑 run_loop（tick_interval_sec）与 maintenance 定时器；SIGINT 一并停掉（决策 54/55）
- [ ] resume/review/merge-decision 端点经 resume_hook 真正驱动 executor；dependency_failed continue 例外不 spawn（决策 130⑤）
- [ ] run_command 以进程组启动并回填真实 pgid 到 node_runs（当前 kill(0) no-op）
- [ ] 真实二进制端到端：创建任务 → 自动推进到终态（接 mock LLM server 即可验收，不必真 LLM）
- [ ] smoke 测试扩展：启动后 tick 循环在跑（可用假时钟或缩短间隔验证）
