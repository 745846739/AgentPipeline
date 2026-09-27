# 01: 失败重试按错误类别分流——传输类不追加错误 turn、配置类不进下一轮（决策 298）

**What to build:** 决策 278 的「续接转录 + 错误 turn」对**任何**失败一视同仁，于是连「连不上 provider」这种请求根本没送到模型的失败，也被往 prompt 里灌一条「上一轮的输出未按输出契约提交、已判废（连不上 provider：请检查 base_url 是否正确、网络是否可达）／本轮的最终动作必须是调用 submit_metadata」——前半句是假话（转录末尾是上一次成功的完好回合），后半句括号里是写给人看的运维指引。同一条分支还接了鉴权 / 模型名 / 额度三类「等一等也没用」的失败，坏 api_key 要重试 `agent_retry_max` 遍才罢休。按错误类别分流：传输类**照旧重试但不追加错误 turn**（转录原样续接，278 省下的探索钱不丢）；配置类**一次都不重试**，直接带分类自带的 `advice()` 收口；输出契约类（元数据/校验失败、退化判废、工具超限）= 278 的本体，一字不动。

**Blocked by:** None（can start immediately）

**Status:** done（2026-09-27）

- [x] 两条谓词住 `agent/providers/mod.rs`，与 `is_context_window` 同址、同「按 kind 字段判、不按报文字样」口径：`is_wait_useless`（超窗 / `llm_auth` / `llm_model_not_found` / `llm_quota`）、`is_transport`（`llm_network` / `llm_idle_timeout` / 未分类的 `Error::Llm`）
- [x] `pipeline/model_invoke.rs` 重试循环：295 的超窗分支与新的配置类分支并成一条 `is_wait_useless`（日志带 `kind`）；错误 turn 的追加加一道 `!is_transport` 门槛
- [x] 传输类失败仍按 `agent_retry_max` 重试，续接的是**失败时的转录**（一条不多）
- [x] 配置类失败只打一次调用，`pending.message` 是分类自带的人话（不写「重试耗尽」），原始诊断照旧进 `context.diagnostic`
- [x] integration 两条：`executor::a_transport_failure_retries_without_an_error_turn`（含输出契约类对照组）、`executor::a_config_failure_fails_fast_without_burning_retries`
- [x] 单测一条：`providers::tests::retry_split_predicates_have_disjoint_truth_tables`（两支互不重叠、互不漏）
- [x] 决策 298 落表；278 行加适用边界标注；`AGENTS.md` / `docs/README.md` 决策号区间推到 298；`docs/testing.md` §10 补一行
- [x] 门禁实际结果（2026-09-27，**不是「四门全绿」**，如实记）：`check-test` = `cargo test --workspace` → core 单测 **592 passed / 0 failed**、core 集成 **431 passed / 1 failed**，那 1 条是 `production_llm::anthropic_stream_maps_usage_tool_blocks_and_request_shape`，**属并行会话的 prompt-cache 改动**（`providers/anthropic.rs` 把 system 改成带 `cache_control` 的 content block 数组，测试文件 `production_llm.rs` 未被本次改动碰过、仍断言字符串）；`check-lint` → `fmt --check` 报的差异只在并行会话的 `openai.rs` / `tests/integration/foreman.rs`，`clippy -D warnings` 的唯一一条 `needless_late_init` 在并行会话新增的 `pipeline/foreman.rs:2150`，**放行那一条 lint 后 clippy 对全 workspace「No issues found」**（含本次改动的 lib 与集成测试）；`check-frontend` / `check-e2e` **未跑**（本次改动零前端面，且仓库正被并行会话写入）

## Comments

- 来源：2026-09-27 本仓会话问答「当前项目是否会把 llm 连接错误回灌到 prompt 中，是否应该去掉？」→「落实」。读代码当场坐实的路径：`providers/mod.rs` 把连接失败归成 `LlmClassified{kind: llm_network}`，`model_invoke.rs` 的重试循环只拦 `is_cancelled` 与超窗，其余一律 `retry_prompt(&last_error)` 追加成 user turn——而 `LlmClassified` 的 Display 就是那句给**人**看的 `advice()`。

- **不整体去掉机制的理由**（决策 298 的立场）：错误 turn 对**输出契约类**失败是属实的——转录末尾确实是被判废的那轮产出，278 的实测依据（run42 带前文一次自我修正成功）正是这一类。错的只是它对错误类别不设防。

- **未分类 `Error::Llm` 归传输类不是新判断**：值班长的 `turn_failure_reason`（`pipeline/foreman.rs`）早写着「适配器层的失败……它就是网络那一类」。其中「provider 不存在 / 已被禁用」更像配置类，但把它们判成配置类属分类那一层的事——`error.rs` 的纪律是「宁可退回原始串，也不把未识别的错误误标成已知类别」，本票不为这条破例。它们此前一直被重试，本票只是不再给它们加那句假话。

- **超窗那条与本票并成同一条分支**（295 的分支合并）：行为一字不变（调用点先压一次、压不动就 `return Err(error)`），变的只是外层从两个 `if` 收成一个，日志带 `kind`。295 的验收用例（`a_context_window_failure_is_not_retried_to_exhaustion` / `..._compacts_and_retries_that_one_call`）原样通过。

- **明确不做**的另外两条见决策 298：不动调度器判的节点级超时那一层（本票 02 的评审记录已划清边界）、不做传输类的就地退避重试（那是 295「压缩后重试这一次调用」的同族动作，要做得单独立决策）。
