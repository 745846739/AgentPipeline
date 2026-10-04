# 10: 工具日志收场行在高负载下丢失——捕获时序 flake

**What to build:** `every_tool_call_leaves_start_and_end_lines_in_the_log_stream`
这类日志断言在满载机器上不再偶发红：测试对「工具调用开始/收场成对出现」的
断言要么改成对捕获时序不敏感的形状，要么修掉发射/捕获之间的真实竞态——
满载回归下连跑稳定绿。merge 闸门不再因为这条 flake 假红、把任务无谓打回。

**Blocked by:** None (can start immediately)

**Status:** done（2026-10-04，根因在 tracing 捕获层的 callsite interest 缓存，非发射时序）

- [x] 诊断定案：发射路径无罪——开始/收场两行都在 `execute` 内**同一 await 上下文**
      同步发射，缓冲里不可能「收场比开始晚到」。真凶在**捕获层**：tracing 的每个
      日志调用点（callsite）在进程内**第一次发射**时按当时的派发器状态缓存一次
      interest；本测试二进制没有全局订阅者，`set_default` 的 scoped（线程局部）
      订阅者只在持有它的线程可见——752 个测试并行满载时，收场行的 callsite 首次
      发射恰落在无 scoped 订阅者的线程/时刻 → interest 缓存成 `never` → 此后
      **全进程**该调用点的事件在宏层被静默丢弃（丢弃，不是迟到——轮询等不来）。
      **证据**：106 闸门实红签名（2 开始/0 收场，两次 execute 都已 await）在本地
      满载复现——`--test-threads=2` 全量 40 轮 **15 红（37%）**，全部红在同一条
      断言、缓冲里恰是「2 开始 0 收场」；同窗口的另一个订阅者测试
      （`failing_tool_still_leaves…`）却全绿——各 callsite 注册时刻互不相干，
      逐 callsite 翻面。与决策 342⑦ 的「没读到被当失败」同族但机制不同：那次
      是连接层迟到，这次是宏层丢弃。
- [x] 修复落地：**捕获层**加全局兜底订阅者（`warm_interest_cache`，writer 为
      sink、事件全丢，Once 一次性）。全局注册者一旦存在，所有 callsite 的注册
      与重建都只会算出 `always`，注册竞态从此不可达；派发侧 scoped 订阅者仍优先
      （`get_default` 先看线程局部），捕获语义不变。不改成对断言本身；不依赖
      「等待收场行到场」（事件是丢弃不是迟到，等不来）。
- [x] 回归验证：修复后同口径满载循环（`--test-threads=2` 全量）40 轮 **0 红**
      （修复前同口径 15/40 红）；每个事件格式的 sink 成本无行为影响（单轮耗时
      20.5s → 17s，波动内）。工具日志断言（票 runner-offload/01 的三段分布上游）全绿。
