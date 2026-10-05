# 01: 评审打回反馈改「转录末尾 user turn」+ required_changes 扩 finding 字段

**来源:** 任务 01M450DK2GKZP4PJ4FVRAFAGXZ（第三轮 UX 审计）实录：review 两次
`approved: false`，第一次打回后 develop 重入仅 32 秒、约 1500 token 就重新交卷，
5 个 `required_changes` 一项未动，`review-report.md` 在三个 run 的落库转录中出现
**0 次**。打回简报本身按决策 133 正常注入了（106 库 `kanban_node_conversations`
里 run 279/280 的 `user_prompt` 列实存「## 评审必须修改项」五条）——失效的是
**形态**，不是机制没触发：① 反馈渲染在 90+ 条、约 24 万 token 续接转录的**首条
消息**里，而转录末尾是模型自己上一轮的「完成」总结（「13 项机检全 PASS」），
注意力被自己的完成叙事淹没；② `required_changes` 只有文件路径没有内容，评审报告
（18KB）落在任务目录（worktree 之外），简报只给裸文件名不给路径——agent 没找、
没读。会话拷问（grilling）定案，决策草稿见 `../decision-draft-387.md`。

**What to build:** 评审打回 develop.execute 重入时，反馈不再渲染进首条消息段
（`model_request.rs::review_required_changes_segment`，决策 133 / pipeline-spec §6），
改为**续接转录末尾追加一条 user turn**（装配形态对齐决策 279 的 `carried.push`）：

- [x] `ReviewResult.required_changes` 每项扩 `finding: Option<String>` 字段——
      评审 execute 产出时逐项填写发现摘要（错在哪、该改成什么）；
      旧格式输出（无 finding）向后兼容，降级为「只列路径 + 报告绝对路径」
- [x] 打回 turn 的内容：系统前缀 `【评审打回反馈·系统注入】` + 每条修改项的
      finding 内联 + `review-report.md` 的**绝对路径**（任务目录）
- [x] 覆盖两条打回路径：`ResumeCause::Review`（agent 评审 + 用户按「打回开发修复」）
      与 `ResumeCause::HumanReviewRejected`（human 评审端点）——两者的续接判定同为
      true（`types.rs:551`），反馈落点应一致
- [x] 装配点：续接转录之后追加（对齐决策 279），**首条消息逐字不变**——prompt
      cache 前缀承诺从 run 内延伸到 resume（与决策 380 的划界见决策草稿）
- [x] 集成测试：打回重入后转录末尾含带前缀 turn + finding 内联；`approved=true`
      或无 review 产出时不渲染；旧格式输出走降级路径；撤掉追加后用例先红

**Blocked by:** None (can start immediately)

**Status:** done

**边界.** 只动 review 打回这一条反馈的落点与内容；其余 reentry 段
（gate_recheck / backtrack_feedback / retry_feedback）不动（见票 02）；不解析
review-report.md 摘录（markdown 无 schema，脆弱——内容由评审 execute 结构化产出）；
不把报告搬进 worktree；不动 review 阶段 30 分钟超时配置；不改 `review_mode=human`
的 UI 与 `/review` 端点契约。
