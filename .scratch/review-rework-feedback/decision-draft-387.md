# 决策 387 草稿（随票 01 落地时按此条文写入 docs/decisions.md，只追加）

## 决策 387 · 评审打回反馈注入位置：续接转录末尾的 user turn（修订决策 133 的渲染落点；扩展 279 的形态与 371 的纪律）

**起因**：任务 01M450DK2GKZP4PJ4FVRAFAGXZ（第三轮 UX 审计）的实录——review 两次
`approved: false`，第一次打回后 develop 重入仅 32 秒、约 1500 completion token 就重新
交卷，5 个 `required_changes` 一项未动，`review-report.md` 在相关三个 run 的落库转录中
出现 0 次。而打回简报**确实注入了**：106 库 `kanban_node_conversations` 里 run 279/280
的 `user_prompt` 列实存决策 133 渲染的「## 评审必须修改项」五条。失效是形态性的：
① 反馈渲染在 90+ 条、约 24 万 token 续接转录的**首条消息**，转录末尾是模型自己上一轮
的「完成」总结——重入模型的注意力沿自己的完成叙事惯性滑行，段反馈被淹没；②
`required_changes` 只有文件路径，评审的实质发现在任务目录的 `review-report.md`（worktree
之外），简报只给裸文件名——agent 没找也没读。

**裁决 ①（落点：反馈 turn 化）**：评审打回 develop.execute 重入时，反馈不再渲染进
首条消息段，改为**续接转录末尾追加一条 user turn**（装配形态对齐决策 279 的
`carried.push`；首条消息逐字不变）。覆盖两条打回路径：`ResumeCause::Review`（agent
评审 + 用户按「打回开发修复」）与 `HumanReviewRejected`（human 评审端点）——两者
`resume_continues` 同为 true，反馈落点一致。

**裁决 ②（内容：内联 finding + 绝对路径）**：`ReviewResult.required_changes` 每项扩
`finding: Option<String>`——评审 execute 产出时逐项填写发现摘要（数据流最短路：发现
本就在评审 agent 手里）；打回 turn 内联全部 finding 并附 `review-report.md` 绝对路径。
旧格式输出降级为「只列路径 + 报告路径」。不解析报告 markdown（无 schema，脆弱）。

**裁决 ③（纪律：user turn 的两类来源可区分）**：决策 371 的「user turn 的内容仍必须
逐字是用户的话」修订为分档——用户原话 turn 逐字不加前缀；**系统注入的 turn 必须带
结构化前缀**（`【评审打回反馈·系统注入】`），两类 turn 在转录里可机器区分、模型可
辨识。纪律的意图（转录保真、可审计）不变，能力边界从「谁说的」精确到「谁说的 +
谁注入的」。

**与决策 380 的关系（划界，不修订）**：380 拒绝的是「为了让前缀更稳而**重排既有稳定
段**」——cache 动机、无行为收益证据，故不做。本决策定的是 **reentry 反馈的初始落点**：
反馈第一次注入就该落在模型下一个 token 的注意力点上，动机是行为有效性、有失败实录
为证；首条消息里的其余段（环境路径、模板变量等稳定段）一律不动。380 原文与适用范围
不变。

**对决策 279 的事实修正**：279 括注「execute 节点（节点间无转录可续）仍走 segment」
——该前提在它验收的场景（validate_input）成立，但 review / merge 打回 develop.execute
**带全卷转录续接**（`resume_continues` 表）。user turn 机制的能力边界按**续接判定表**
（`types.rs:551`）划，不按节点名划；279 原文的适用结论（validate_input 用 turn）不变，
错误前提由本决策更正。

**明确不做**：不动其余 reentry 段（gate_recheck / backtrack / retry——无淹没实录，
观察票 106-stability 无、本 slug 票 02）；不把评审报告搬进 worktree（绝对路径 +
内联已覆盖）；不动 review 阶段的 30 分钟超时配置（另一个问题，本决策减少无效重入
即是缓解）；不改 `review_mode=human` 的端点契约。

**验证**（落地时钉死）：集成用例——打回重入后转录末尾含带前缀 turn 且 finding 内联；
`approved=true` / 无 review 产出时不渲染；旧格式（无 finding）降级路径；撤掉追加后
用例先红。全仓 `cargo test --workspace` 绿。

**来源**：用户（2026-10-05 会话拷问逐条定案：落点 turn 化 / 内容内联 / 纪律分档 /
380 划界）；证据 106 库 `kanban_node_conversations`（run 279/280 的 user_prompt 与转录、
run 283 评审结论）；决策 133（被修订的渲染落点）、279（形态先例与被更正的前提）、
371（被分档的纪律）、380（被划界的不做裁决）；落地票
`.scratch/review-rework-feedback/issues/01-rework-feedback-as-user-turn.md`
