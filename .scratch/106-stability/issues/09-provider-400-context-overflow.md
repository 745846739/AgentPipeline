# 09: review/test agent 的 provider-400——长转录绕过了压缩硬底

**What to build:** 84 万–120 万 prompt token 的转录不再能撞穿 provider 的请求上限：
review / test 这类后段 agent 的长会话要么在硬底判据处被压住，要么压后仍超限时
有可解释的降级路径——总之不再出现「agent 节点重试耗尽：HTTP 400 Bad Request、
需要用户介入」这个形态。修好后，今天三次人工 skip 裁定的剧本不再重演。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] 诊断定案：决策 378 的硬底（转录字符量 20 万强制 L3）为何没拦住——候选：
      判据没走到（后段 agent 的组装路径不同）/ `conversation_max_chars` 缺省值
      对 1M 窗口 provider 形态偏宽 / 压缩发生了但每轮追加的工具输出又撑回去 /
      cache_read 占比高导致按 token 计的软限从未触发。证据在库：
      2026-10-04 任务 01M40NEDA0…（review 841k / test 3 次 400）与
      01M428HPK…（test 400 ×2）的 `kanban_node_runs.error` 与
      `kanban_model_requests` 逐请求 token 数。
- [ ] 修复落地：按诊断结论收口——阈值问题改缺省/配置，路径问题补判据，
      压缩回撑问题改压缩时机；**不**给 provider 上限做手抄常量（决策 378④ 的
      双向校准已管登记值）。
- [ ] 回归验证：用今天的真实事故形状做负载用例（后段 agent、长转录、
      工具输出占大头），断言请求体量压在 provider 拒收线以下、节点能自行收口；
      既有压缩/续接用例（决策 378/379 的钉子）全绿。
- [ ] 真机验收：106 上下一个走到 review/test 的自然任务不再出现 400 重试耗尽。

> **证据记录（2026-10-04）**：三次 400 分别是 01M40NEDA0… 的 review（841k
> prompt tokens）与 test（attempt 4）、01M428HPK… 的 test（attempt 3）；每次
> 重试都整卷重喂、烧几十万 token 后仍 400，操作员只能 skip（决策 382 记录在案）。
> 同族背景：决策 376 的 O(n²) 病根、378 的硬底、380 的缓存命中——本票治的是
> 「硬底为什么没兜住后段」这一段。
