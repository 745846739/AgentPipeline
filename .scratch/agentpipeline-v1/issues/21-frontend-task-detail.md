# 21: 前端：任务详情与 resume 交互

**What to build:** 任务详情视图：pending 卡片（展示阶段/节点/原因/已产出，G4）与 allowed_actions 纯渲染（resume 类走 POST /resume，side_effect 走配对端点——决策 101/119 的人机交互面）、流转时间线、会话/命令审计面板、产出文件查看、人工评审与 merge 审批交互。

**Blocked by:** 20（前端应用骨架）

**Status:** done — `/task/:id`：9 站 hero 轨道（节点字形/游标/双轨）、待办 dossier（360px，按分支分组纯渲染 allowed_actions，resume 带 cursor_id、side_effect 走配对端点、info_insufficient 行内输入）、时间线/会话查看器/命令与输出/产出文件（403 降级）/Diff 审批（仅「合入/返回修改」+ 基准前移提示）/人工评审三件套、长耗时按钮 SSE 回执复位、render/ 七件公共渲染件落地；配置/指标页留票 22 占位。vitest/svelte-check/build 全绿。

- [x] pending 卡片：按 allowed_actions 渲染，continue 需输入框、goto 需目标选择（决策 35/49/69）
- [x] side_effect 按钮全部接到配对端点（cancel/split/model-override/review/merge decision）
- [x] 时间线（GET /flow）、会话列表与详情、命令列表与卸载输出查看
- [x] 产出文件只读查看（GET /files/{path}，403 降级提示）
- [x] toast 冷却/静音时段（NotificationPolicy 前端侧，决策 65/130③）
