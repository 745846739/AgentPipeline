# 06: 窗口界——卸载放开 + 轮内压缩 + 撞墙恢复

**What to build:** 三条，让一轮的上下文只受 provider 窗口限制，而且撞到窗口之前有路走：(a) **人的那一轮**撤掉台账三件工具的 12k 内部截断，改走 L2 卸载（回执给绝对路径，模型用 `read_file` 回读）；**(b) 轮内**每次调用前查一次窗口预算，触软限就压缩（复用流水线那套容量算术）；(c) provider 报上下文超长 → 压缩后**重试一次这一次调用**，而不是原地判败。

**Blocked by:** 05（复用它的「单次调用重试外框」）

**Status:** ready-for-agent

- [ ] (a) `crates/core/src/agent/tools.rs:866-874` 的跳过名单 + 三件台账工具内部 12k（`read_conversation` / `read_diagnosis` / `read_task`）；`foreman.rs:737-742` 那条旧理由（「值班长没有 task_id，卸载无处可写」）**已经过时**——会话维度目录早通了（`home.rs:130-132`），大结果今天就在那儿落盘
- [ ] (a) **值守轮不放开**：`read_diagnosis` 的 12k 是「值守只读台账与诊断包**摘要**」那条分级纪律的一部分（决策 265 / 266），本次一字不改（裁决 4）
- [ ] (b) 容量算术复用 `pipeline/model_request.rs:219-314`（`estimate_context_capacity` / 软硬限 / `OUTPUT_RESERVE`）与 `agent/context.rs:52-78` / `:258`；`foreman.rs` 里 `context_window` 今天**读取次数为零**，这一步就是把它接上
- [ ] (b) `compact_history`(foreman.rs:1337-1408) 从「只在循环前压一次 DB 历史」扩展到**轮内 transcript**（工具结果累积处 :1819 之后查预算）
- [ ] (c) `LlmErrorKind::from_http`(providers/mod.rs:101-148) 的 `llm_context_window` 走到「压缩后重试一次」这条支；今天它是一类普通失败，而流水线还会拿着同一份超长转录重试到耗尽（后者见票 10）
- [ ] core 测试：大结果真卸载且可按路径回读；轮内到软限触发压缩且锚点位置正确；超长报错 → 压缩后成功
- [ ] 四门 + 决策落号

## Comments

- 来源：拷问 Q6、Q13、Q14 与裁决 6。
- 代价如实记：压缩会打断 provider 的 prefix 缓存（2026-09-26 实测 94% 命中，是这套东西唯一便宜的地方），压一次之后下一次调用几乎全量重算。故触发要**迟钝**（到窗口 ~80% 才压），不能每次调用都压。
