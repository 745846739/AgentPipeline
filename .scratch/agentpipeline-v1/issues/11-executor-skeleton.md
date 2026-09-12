# 11: 执行器骨架与串行 happy path

**What to build:** 流水线的运行时基石：执行器循环让一个任务从创建到 done 被自动推进——execute_node 分发（agent 节点走 LlmClient，纯代码节点走系统路径）、advance_cursor 按路由函数推进游标、node_runs 落库与心跳、节点级 SSE 事件、进程内单执行者注册表、终态判定、join 推进挂点。交付后 e2e 从「手动推 store」改为 FakeAgent 驱动完整流水线。

**Blocked by:** None（01–10 已全部实现）

**Status:** done（已实现；两条注记见下）

- [x] FakeAgent 驱动 init → architect-design → develop → review → test → merge → done 的 e2e 全绿（e2e happy_path 已改为 FakeAgent + executor.run 驱动）
- [x] 多游标 executor：pending 游标移出可运行集合、单游标失败不传播（决策 89）、无可运行且无 pending 才暂停等 resume（L2 `one_branch_pending_does_not_stop_the_other`）
- [x] 进程内 Mutex<task_id> 注册表 + DB executor_owner 乐观锁双保险（决策 36，L2 `concurrent_executors_deduplicate_on_the_same_task`）
- [x] waiting_join 由 advance_cursor 唯一写入；advance_join 在 join 就绪时推进一次（决策 107，sync-check run 恰一次有断言）
- [x] 节点级 SSE 有生产者：node_started / node_finished（含 token 计量）/ stage_changed / cursor_changed / task_done（决策 76/84）。注记①：`task_failed` 暂无生产者——执行器把节点失败一律呈现为 pending(retry_exhausted)，v1 执行路径不产生 failed 终态；该事件的生产者随未来 failed 终态流程落地
- [x] 节点失败按 agent_retry_max 干净对话重试，耗尽置 pending（决策 33/G13；attempt 计数与 scheduler 超时重试口径一致）
- [x] kanban_node_runs 生产写入（含纯代码节点 agent_type="system"，决策 99/114；init / sync-check / done / 各纯代码闸门 / merge 阶段均有 system run）
- [x] 会话落库按 conversation_max_chars 截断（Value 层截断：丢最旧轮次、极端情况整条替换为标记消息，落库恒为合法 JSON）

**随本票一并落地（吸收自票 15）：** merge 阶段 A/B 的执行与闸门——串行 happy path 须经 merge 到 done，无法再拆。剩余缺口见票 15。

**注记②：** `prompt_template_hash` 按 implementation.md §11.5 原文取「最终组装 system prompt 的 SHA-256 前 16 位」；正式 §10.3 模板与 AGENTS.md/stage_configs 消费在票 12。
