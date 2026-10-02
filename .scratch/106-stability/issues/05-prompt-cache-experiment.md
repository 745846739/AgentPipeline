# 05: O4-C prompt cache 验证实验——前缀命中让重喂变便宜

**来源:** 同 01 的监控实录。O(n²) 的根子在「每轮整卷重发」;即便票 03 把体量压住,
重喂依然按全价计费。主流 provider 的前缀缓存(system prompt + 转录前缀不动则
命中折扣价)与本仓的转录形态天然契合——转录只追加、不改写。

**Blocked by:** None(与票 03/04 并行,互不阻塞)

**Status:** todo

- [ ] 摸清 106 在用 provider 的 cache 支持(cache_read/write 字段已在
      `RunTokens` 里,说明响应侧已解析):哪些前缀可命中、计费折扣、TTL
- [ ] `RequestPlan` 组装时把**稳定前缀**(system + 载入的历史转录)放在消息列表
      头部且字节级稳定(不插时间戳等易变内容),工具定义顺序固定
- [ ] 用 FakeAgent/真 provider 各测一轮:断言 `cache_read_tokens` 在第二轮起显著
      非零,记账正确入库
- [ ] 产出一份实验记录(`.scratch/106-stability/cache-findings.md`):命中率、
      节省比例、失败形态——数据说话后再决定是否把缓存友好性列为组装层的硬约束

**边界.** 不改计费口径(`run_tokens` 仍 prompt+completion,cache 单列——
既有注释已写明);不做缓存键的手工管理( provider 自动前缀匹配)。
