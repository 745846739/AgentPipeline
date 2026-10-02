# 05: O4-C prompt cache 验证实验——前缀命中让重喂变便宜

**来源:** 同 01 的监控实录。O(n²) 的根子在「每轮整卷重发」;即便票 03 把体量压住,
重喂依然按全价计费。主流 provider 的前缀缓存(system prompt + 转录前缀不动则
命中折扣价)与本仓的转录形态天然契合——转录只追加、不改写。

**Blocked by:** None(与票 03/04 并行,互不阻塞)

**Status:** done(2026-10-03,决策 380)

- [x] 摸清 106 在用 provider 的 cache 支持(cache_read/write 字段已在
      `RunTokens` 里,说明响应侧已解析):哪些前缀可命中、计费折扣、TTL
- [x] `RequestPlan` 组装时把**稳定前缀**(system + 载入的历史转录)放在消息列表
      头部且字节级稳定(不插时间戳等易变内容),工具定义顺序固定
- [x] 用 FakeAgent/真 provider 各测一轮:断言 `cache_read_tokens` 在第二轮起显著
      非零,记账正确入库
- [x] 产出一份实验记录(`.scratch/106-stability/cache-findings.md`):命中率、
      节省比例、失败形态——数据说话后再决定是否把缓存友好性列为组装层的硬约束

**边界.** 不改计费口径(`run_tokens` 仍 prompt+completion,cache 单列——
既有注释已写明);不做缓存键的手工管理( provider 自动前缀匹配)。

---

## 验收实录（2026-10-03,决策 380）

**票面前提被数据推翻**：缓存**早就在命中**。106 历史账单（票 03/04 部署之前）：

- `kanban_model_requests`：2,674 次调用 / 1.93 亿 prompt token，**95.68% 命中**；
- `kanban_node_runs`：96.53%；
- 事故任务 `01M3X472FJF8NW9082K6BFZPKC`：**97.3%**，其 develop.execute 九个 attempt
  逐个 94.9%–98.1%（每个 attempt 13M 量级重喂）。

故「重喂按全价」不成立——真金白银烧在**体量**上（那正是票 03 治的）。

**一手探针**（在 106 上就地跑，只打印 `usage`，密钥不出机）：同一定长前缀连发两次 →
请求 1 `prompt_tokens 608 / cached_tokens 0`（冷启动），请求 2
`prompt_tokens 617 / cached_tokens 512`。两个读数：上游按 **64 token 整块**命中；
响应字段正是本仓已在读的 `usage.prompt_tokens_details.cached_tokens`，**解析侧无需改动**。

**跨 attempt 的 90 分钟熬得住**：事故任务 develop.execute 每个 attempt 的首个请求在间隔
25 / 38 / 41 / 49 / **90 / 90** 分钟后仍稳定命中 2,688 token（system 前缀）——
「超时梯子每档之间缓存会失效」这个最该担心的假设也被否掉。

**落地（零生产代码改动 + 三条回归钉）**：组装层已经是缓存友好的（wire 头
`[system][user][载入历史][本轮追加]`；轮循环只 append；无时间戳进 prompt；工具序 =
基线常量序 + 声明序）。本票只把它钉住：

- `model_request::assemble_freezes_a_byte_stable_head_and_a_fixed_tool_order`
- `providers::openai::the_next_round_is_a_prefix_extension_of_the_previous_wire_messages`
- `executor::provider_cache_readings_land_on_the_run_row`

**不做**：缓存键手工管理；**不**为缓存重排 prompt 分层（数据不支持，且是行为改动）；
不给网关侧精确折扣承诺（上游厂商标价 ≠ 本机网关账单）。

**与票面的两处出入，如实记**：① 第三条验收写的是「用 **FakeAgent**/真 provider 各测一轮」，
实际替身用的是仓内既有的自定义客户端手法（`executor.rs` 里 `PrefixCachingClient`），
**不是** `testkit::FakeAgent`——因为 `testkit/script.rs` 的 `Step` 只带 `prompt_tokens`，
发不出 `cache_read_tokens`，要用 FakeAgent 得先给它开一个只有这一处用的口子；
而 FakeAgent 的发注（脚本队列）本来就不需要，故按既有先例写替身。真 provider 那一轮
是 106 上的一次性探针（读数与复现口径见 findings 第一、三节），**没有**落成仓内可重跑
的用例——它要真密钥与真网络，按接缝纪律（决策 194）本就不进默认测试面。
② 第四条验收里的「节省比例」与「失败形态」在 findings 的第五、五之二节补全
（节省比例是**带假设的算式**，不是承诺；失败形态含一条「证不了也否不了」的观察项）。

**留给票 08 的一次复核**：票 03 的硬底让压缩更频繁，压缩与缓存反向（压掉的那段下游作废），
命中率可能掉一点——这是「体量换命中」的有意取舍。上面 95.68% 是**票 03 之前**的基线，
票 08 首小时监控顺带复核（查询写在 findings 第六节），**低于 ~85% 才回头查组装层**。

**证据**：`.scratch/106-stability/cache-findings.md`
