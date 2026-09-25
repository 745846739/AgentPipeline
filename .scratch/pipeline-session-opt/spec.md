# spec：pipeline-session-opt（看板会话复盘驱动的管线优化）

## 背景

用户 2026-09-25 要求分析看板任务 `01M3BGVCXDWFPT0Q3BZYAGZP8Q`（修复页面切换导航闪屏）的会话。该任务在 architect-design / validate_input 节点连跑四次、15m47s、约 68 万 prompt token，只为问出 3 个问题；其中两次失败暴露四个管线缺陷，一次成功暴露一个疑点。经 grilling 会话逐项定案（决策 277–280），拆为五张票。

## 证据（实测，来自 `kanban_node_runs` / `kanban_model_requests` / 会话转录）

- run39（attempt 1，成功）：判 readiness=false，提 7 条 blockers。
- 用户补充输入仅一句「使用grilling技能询问」（user-input.md，49 字节）。
- run40（失败，8s）：续接 run39 转录；模型把补充当对话、未调 submit_metadata →「校验错误：未找到结构化元数据」。首请求 prompt 25,800、cache_read 仅 126——补充输入经 user-input.md 重渲染进首条 user 消息，前缀全变，缓存打穿；且归档转录里没有用户发言。
- run41（失败，89s）：干净重试（无任何错误反馈）；加载 grilling 技能后输出退化（「Playwright 或」复读 150+ 次），仍未调工具；**且在调用 Skill 工具后的下一轮请求 cache_read 126 / 6,697，按设计解释不通**。
- run42（成功，4m11s）：再次干净重试，同批文件第三遍重读，以「问题 + 推荐答案」形状提交 3 问。
- 「错误回填」（retry_prompt）在生产路径零调用——死代码；自动重试一概裸重来。

## 决策映射

| 决策 | 内容 | 票 |
|---|---|---|
| 277 | validate_input 输出契约强化（每轮必交元数据 / blockers 问题+推荐答案 / 能自答的不问 / pending 消息带 blockers） | 01 |
| 278 | agent 失败重试续接转录 + 错误 turn（**显式修订 205②**） | 02 |
| 279 | 补充输入注入位置：续接会话追加 user turn（**限定 79**） | 03 |
| 280 | 模型退化护栏：流式 n-gram 检测 + degraded 打标 | 04 |
| — | Skill 调用后缓存全失疑点（调查，结论另立票） | 05 |

## 依赖边

01 / 02 / 03 / 05 相互独立可并行；04 被 02 阻塞（degraded 后的续接重试依赖 278 的转录保留语义）。

## 显式不立案

- 防连点双击落两条相同 user_resume 流转行——冷却已防重复 spawn，纯观感。
- 退化后换模型/采样参数——先攒护栏数据，配置面的事不进机制。
- blockers 改结构化对象——下游 as_str 过滤会静默丢，保持 string[] 直通。
