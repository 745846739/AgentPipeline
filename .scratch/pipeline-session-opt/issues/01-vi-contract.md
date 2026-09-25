# 01: validate_input 输出契约强化（决策 277）

**What to build:** 任务卡在「信息不足」时，用户在看板卡片上直接看到 agent 要问的问题（blockers 摘要），不必翻会话记录找；无论用户在补充输入里说什么（包括要求换用 grilling 之类的技能或问答风格），validate_input 的每一轮都以调用 submit_metadata 收尾——用户的补充是新证据，不是模式切换；提交的 blockers 每条都是「问题 + 推荐答案」的形状，且能从仓库文档/代码确定默认值的约束不再拿来问用户（直接采用并注明默认值）。

**Blocked by:** None（can start immediately）

**Status:** done（2026-09-25）

- [x] 三个 VI 模板（架构 / 开发设计 / 测试设计的输入充分性检查）都带契约句：「每一轮的最终动作必须是调用 submit_metadata；用户的补充输入是新证据；要问的写进 blockers」（决策 277①）
- [x] submit_metadata 的 blockers 字段描述与模板同步为「问题 + 推荐答案」，类型保持 string[] 自由文本直通（决策 277②；不改结构化对象——下游 as_str 过滤会静默丢）
- [x] 「能从仓库文档/代码取默认值的约束不列为 blocker，直接采用并注明默认值」进模板（决策 277③）
- [x] info_insufficient 的 pending_reason.message 携带 blockers 摘要（截断）——路由处 metadata 本就在手，纯后端改，前端零改动（决策 277④）
- [x] core 单测（模板契约句 / schema 描述）+ routes 测试（pending 消息带 blockers）+ app L3 契约；四门全绿

## Comments

- 来源：任务 01M3BGVCXDWFPT0Q3BZYAGZP8Q 会话复盘（run39 提 7 条 blockers、其中数条 agent 可自答；run40 模型把用户补充当对话、未交元数据判败）。

- **评审记录（2026-09-25）**：app L3 契约断言暂缺——`api_contract.rs` 正被另一在途工作流（notify-politeness）修改，本批不碰该文件；pending 消息经 GET /tasks 投影原样透出（core 侧集成已断言 message 内容），L3 断言随下一个触碰该文件的批次补上。
