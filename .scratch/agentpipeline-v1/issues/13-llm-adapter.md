# 13: 生产 LLM 适配器

**What to build:** LlmClient 的真实实现：至少 OpenAI 兼容与 Anthropic 两族适配器（base_url 可配），请求计量（prompt/completion tokens）、prompt cache token 落库、流式响应（供 SSE conversation_delta / tool_event，决策 123）；配置沿用 providers 表（context_window 查找路径唯一，决策 111）。

**Blocked by:** 11（执行器调用点）、12（真实 prompt 才有冒烟意义）

**Status:** done

- [x] openai 兼容 + anthropic 适配器，SUPPORTED_ADAPTERS 白名单内可配 base_url（`crates/core/src/agent/providers/`：openai 与 deepseek 同走 OpenAI 兼容族；base_url 缺省按 vendor 取官方默认，显式配置优先）
- [x] token 计量进 node_runs；cache_read/cache_write 有真实生产者（决策 46）（executor 的 `RunTokens` 累计进 `RunOutcome`；OpenAI 取 `prompt_tokens_details.cached_tokens`，Anthropic 取 `cache_read_input_tokens` / `cache_creation_input_tokens` 且 prompt 归一为 input + cache_read + cache_creation）
- [x] 流式：conversation_delta / tool_event SSE 有生产者（决策 123）（生产适配器全程流式：文本增量逐段发 delta、usage 收尾一条增量事件；tool_event start/end/error 由 executor 工具循环发射）
- [x] #[ignore] 真实 LLM 冒烟测试（`crates/core/tests/llm_smoke.rs`：`AGENTPIPELINE_SMOKE_API_KEY` 等环境变量驱动，验收流式 + 计量 + submit_metadata 结构化解析）
- [x] 心跳：流式 token 活动刷新 last_activity_at（决策 64）（节流 1s + 流结束确定性兜底一次；另：agent `run_command` 运行期周期心跳随本票落地——决策 100 映射表注记）

**实现注记：**
- 接缝扩展：`LlmRequest` 增加 `run: Option<RunContext>`（流式/心跳上下文）与 `provider_id: Option<String>`（任务覆盖）；`AgentResponse` 增加 cache_read/cache_write（serde default，FakeAgent 无感）。
- provider 解析按**决策 129 四级优先级**实现（node_overrides > 任务覆盖 > 阶段配置 > 首个 enabled 系统默认），集成测试逐级断言。
- 错误类型新增 `Error::Llm`（HTTP 失败 / 流解析失败 / provider 配置缺失都是干净节点错误，走干净对话重试）。
- **与决策 142 / 148 行内文字的显式偏离**：两处「rig 适配层」的表述以手写 reqwest 适配器替代（`docs/testing.md` §11 已同步披露）——rig 会引入重量级依赖，且其流式/计量/缓存 token 的映射粒度不满足决策 46 / 123 的落库口径；`LlmClient` 接缝本身不变。决策编号只增不改，此注记即修订标注。
- 测试基建：`testkit::mock_llm` 手写 HTTP/1.1 mock server（无新运行时依赖，reqwest 仅进 core），7 条集成测试覆盖两族协议的聚合/计量/映射/错误路径。
