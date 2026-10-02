# 08: 传输类失败的模型调用在**轮内**就地重发（现场新发现）

**Status:** 已落地（2026-10-02）
**Blocked by:** 无

## 现场（2026-10-02，106 真实数据）

事故任务续跑后走到 develop，`develop.execute` **连续三次都失败**，死因同一句话：

```
143 → LLM 调用失败：流在没有 [DONE] / finish_reason 的情况下结束（收到 32327 字节后断开，响应不完整）
144 → …（收到 2247803 字节后断开）
145 → …（收到 393247 字节后断开）
```

三次分别在 45 / 19 / 28 次模型请求之后断，各耗 41.7 / 51.8 / 48.3 分钟。
切分点看不出规律（3.2 万字节、22 万字节、39 万字节都断过；run 145 里 27 次请求含
3.6 万 token / 4.6 MB 的大响应都成功了），是**上游的偶发掐流**，不是尺寸阈值。

后果不对称得离谱：**一次偶发失败丢掉的是一整轮的预算**。`agent_retry_max` 一共 3 轮，
一次掐流花掉一轮；而重试轮按决策 278 续接转录，于是每一轮的 prompt 只会更大——
三轮的**平均 prompt 是 69k → 296k → 515k token**，每轮 40–50 分钟。三轮用完，
节点判死，游标落回 `pending` 等人工。

## 要修的

`agent_attempt_inner` 里模型调用那一处的兜底是 `Err(e) => return Err(e)`：除超窗之外
**任何**失败都当场判死整个 attempt。掐流是典型的瞬时失败——同一份请求原样再发一次
就有很大概率过去，代价只是**这一轮请求**的 prompt，比翻掉整个 attempt 小一个数量级。

## 判据复用既有那一处

`crate::agent::providers::is_transport`（决策 298）已经在回答「重发有没有意义」：
传输类（连不上 / 空闲判死 / 适配器层的未分类失败，含 `Error::Llm` 的流读失败与掐流）
= 请求根本没送到模型；配置类（鉴权 / 模型名 / 额度）与压不动的超窗 = 等一等没用。
值班长的 `complete_with_retry`（决策 288）就是只重试传输类的先例。

## 形状

- `ModelInvoke::complete_once_retrying_transport`：同一份 `req` 原样重发，
  上限 `LLM_TRANSPORT_RESEND_MAX = 2`（含首次共 3 次机会）。
- 两个调用点都改走它：轮内那次调用、以及超窗压缩后的那次重试调用。
- 转录**一个字节都不动**：不是「接着上次跑」，是「上次那一下没跑成」。
- 非传输类原样上报（`is_transport` 判 false），超窗照旧走它自己那条压缩重试，
  `Error::Cancelled`（人按停）不重发。
- 重发时 `tracing::warn` 记一次（带 `resent` / `budget` / 原文），现场一眼看得出
  「这一轮被上游掐过两次」。

## 验收

- [x] 断流两次后第三次拿到响应，调用计数 = 3（`a_cut_stream_is_resent_in_place_instead_of_killing_the_round`）
- [x] 断到底仍报错，且调用计数 = 预算 + 1，不是死循环（`the_transport_resend_budget_is_finite`）
- [x] 非传输类（校验类）一次都不重发（`a_non_transport_failure_is_not_resent`）
- [x] 既有三条受影响的集成用例改成「先把预算花掉，这一轮才真的失败」，并各自通过
- [x] `make check-lint` 绿；core lib 712 绿；core integration 486 绿

## 明确不做

- **伪阶段那两处调用**（`run_pseudo_stage` / `project_analysis`，`self.llm.complete` 直调）
  不动。它们的失败代价是「一次节点重试」（单次调用、秒到分钟级），与 agent 轮
  （40–50 分钟、且重试轮还带着更大的转录）不是一个量级；给它们加第二套重试机制
  目前没有现场证据支撑，属于投机性扩大范围。现场再撞到再说。
- 不调 `agent_retry_max`，不调 `max_duration_sec`，不改 provider 的 `context_window`
  （L3 压缩触发线是窗口的 80% = 800k，所以 515k 的转录本来就不会被压缩——
  这是另一个议题，本票不动它）。
- 不调 `offload_threshold_tokens`。

## 落地记录（2026-10-02）

`crates/core/src/pipeline/model_invoke.rs`：

- 新常量 `LLM_TRANSPORT_RESEND_MAX`（`pub`，并从 `pipeline/mod.rs` 转出——它是可观测的
  行为参数，集成用例要照着它把「预算用尽 → 这一轮才失败」打出来）。
- 新方法 `complete_once_retrying_transport`，两个调用点改道。
- 测试模块里新增 `FlakyLlm` 桩（按次数失败、可换错误构造器）与 `request_for`；
  原 `base()` 拆出 `base_with(llm)`，既有 6 条用例一行未动。

`crates/core/tests/integration/executor.rs`：三条用例的脚本改成连发
`LLM_TRANSPORT_RESEND_MAX + 1` 次 `fail_llm`（`a_failed_round_records_the_tokens_it_burned`、
`a_transport_failure_retries_without_an_error_turn`、
`llm_failure_writes_a_conversation_row_with_the_reason`）——它们钉的是**轮级**行为
（失败轮的 token 记账、决策 298 的「传输类不追加错误 turn」、失败会话行的类别），
而轮级行为现在必须先把轮内的重发预算花掉才轮得到，这是本票引入的第二档。

## 现场复验（部署后的第四次尝试，2026-10-02 07:23 CST）

run 146 在真库上跑：**67 次请求、46 分钟、掐流 0 次**——`同一份请求就地重发` 的 warn
一次都没打（前三次分别死在第 45 / 19 / 28 次请求上）。修复是保险，这一轮没用上它。

它最后死在**额度**上（`provider 额度不足（余额 / 配额）`，上游原文
`You have insufficient credits to make this request.`，HTTP 400），
而**重发计数仍是 0**——这正是本票要的判据：`Quota` 属 `is_wait_useless`（决策 295 / 298），
重发只是把同一份没钱的请求再问一遍，一次都不该发。分类正确、预算没被浪费。

也就是说本票的**反面**也在现场验到了：该重发的（掐流）重发，不该重发的（额度）一次不碰。
卡点是账户余额，不是代码；下一步见 `.scratch/silent-degradation/spec.md` 的同一节。
