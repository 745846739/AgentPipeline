# 09: review/test agent 的 provider-400——长转录绕过了压缩硬底

**What to build:** 84 万–120 万 prompt token 的转录不再能撞穿 provider 的请求上限：
review / test 这类后段 agent 的长会话要么在硬底判据处被压住，要么压后仍超限时
有可解释的降级路径——总之不再出现「agent 节点重试耗尽：HTTP 400 Bad Request、
需要用户介入」这个形态。修好后，今天三次人工 skip 裁定的剧本不再重演。

**Blocked by:** None (can start immediately)

**Status:** done（2026-10-04，诊断推翻票面全部四个候选——根因是压缩产出 wire 非法转录）

- [x] 诊断定案：决策 378 的硬底**走到了也压了**——成功请求全程被压在 80–90k tokens
      （106 库逐请求核过），四个候选全不成立。真根因在压缩的**产出**：
      `compact_messages_from` 的 keep 窗口按**消息条数**切（`len - keep_recent_rounds`，
      注释说的是「轮」），工具往返一轮至少两条（assistant 载体 + tool 结果），切点
      落在 tool 结果上时 kept 转录以**孤儿 tool 消息**开头（主人 assistant 已被压成
      摘要）。OpenAI 兼容上游要求 tool 消息紧跟带对应 `tool_call_id` 的 assistant，
      违反即整请求 `HTTP 400 invalid_request_error`（网关回的报文无任何 context/length
      字样，`is_context_window` 认不出也无关紧要了）。**证据链**：run 209 的 seq 38
      （ok，85k tokens）→ 27 秒后 seq 39–41 恒 400；此前一次压缩刚发生；attempt 3/4
      的 seq 1,2,3 **第一发就 400**（毒转录落库后原样重载）；run 220 落库转录 6 条
      消息、**第 1 条 role=tool**——直接铁证。FakeAgent 不校验序列合法性，决策 378
      的集成用例因此看不见这个洞。
- [x] 修复落地（两层）：
      ① 压缩器切点**对齐轮边界**——`keep_from` 落在 tool 结果上时回退到 owning
      assistant（`agent/context.rs`，压缩产出生来干净）；
      ② **出口消毒兜底**——`sanitize_tool_sequence`（同文件）：孤儿 tool 结果摘除、
      未回执的 tool_call 摘除、健康转录零改动（不碰决策 380 的前缀缓存），接在
      provider 适配器出口（openai / anthropic 两个 `build_body`，一切调用方的
      执行点；anthropic 侧是评审补上的——只接 openai 会让「一切调用方」名不副实），
      摘除时 WARN 留痕。
- [x] 回归验证：单测四条（切点回退/孤儿+悬空双摘/健康零改动/wire 出局）、集成用例
      `a_long_tool_round_trip_node_is_capped_by_the_char_floor` 增加**逐请求 wire 合法性
      断言**（60 轮工具往返的负载形状，撤掉修复即红——「孤儿 tool 消息进了请求」，
      正是事故形状）；全仓 `cargo test --workspace` 绿。已中毒的历史库也能自愈：
      消毒在出口，重载的毒转录到不了 provider。
- [ ] 真机验收：106 上下一个走到 review/test 的自然任务不再出现 400 重试耗尽。

> **证据记录（2026-10-04）**：三次 400 分别是 01M40NEDA0… 的 review（841k
> prompt tokens）与 test（attempt 4）、01M428HPK… 的 test（attempt 3）；每次
> 重试都整卷重喂、烧几十万 token 后仍 400，操作员只能 skip（决策 382 记录在案）。
> 同族背景：决策 376 的 O(n²) 病根、378 的硬底、380 的缓存命中——本票治的是
> 「硬底为什么没兜住后段」这一段。
