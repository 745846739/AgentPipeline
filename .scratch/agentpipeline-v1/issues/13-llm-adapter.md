# 13: 生产 LLM 适配器

**What to build:** LlmClient 的真实实现：至少 OpenAI 兼容与 Anthropic 两族适配器（base_url 可配），请求计量（prompt/completion tokens）、prompt cache token 落库、流式响应（供 SSE conversation_delta / tool_event，决策 123）；配置沿用 providers 表（context_window 查找路径唯一，决策 111）。

**Blocked by:** 11（执行器调用点）、12（真实 prompt 才有冒烟意义）

**Status:** ready-for-agent

- [ ] openai 兼容 + anthropic 适配器，SUPPORTED_ADAPTERS 白名单内可配 base_url
- [ ] token 计量进 node_runs；cache_read/cache_write 有真实生产者（决策 46）
- [ ] 流式：conversation_delta / tool_event SSE 有生产者（决策 123）
- [ ] #[ignore] 真实 LLM 冒烟测试（当前 testing.md 承认此为空白）
- [ ] 心跳：流式 token 活动刷新 last_activity_at（决策 64）
