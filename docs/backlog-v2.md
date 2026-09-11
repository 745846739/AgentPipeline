# v2 预留

> 拆分自 agent-pipeline.md（原 附录 B）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

## 附录 B：v2 预留

以下设计已在原方案中成形，但 v1 明确不实现，移入本附录保留，避免后续重开讨论。

### B.1 MCP 接入（原 §10.6.7）

阶段可按需启用 MCP server，与内置工具统一到同一套 tool 调用机制：

```python
# 阶段的工具集 = 内置工具 ∪ 基线强制 MCP 工具 ∪ 阶段 MCP 工具
# 注：`mandatory_mcp` 字段已被决策 32 从 SystemBaseline 删除，实现 v2 时需先恢复该字段
async def build_stage_tools(cfg: StageAgentConfig, baseline: SystemBaseline) -> list[Tool]:
    builtin = load_builtin_tools()
    mcp_names = set(baseline.mandatory_mcp) | set(cfg.mcp_servers or [])
    mcp_tools = await load_mcp_tools(mcp_names)      # 从 MCP server 拉取工具定义

    all_tools = builtin + mcp_tools
    allowed = set(baseline.mandatory_tools) | set(cfg.tools or [])
    return [t for t in all_tools if t.name in allowed and t.name not in baseline.forbidden_tools]
```

**MCP 约束：** 与内置工具同受**文件工具路径策略**（`FileToolPolicy`，决策 104）约束——注意这不是系统级沙箱，MCP server 若自身提供 shell 能力则不受限；server 启动失败 → 配置加载失败（fail fast），不降级静默忽略；工具调用同样计入计量与审计。

**Skill 与 MCP 的区别：** skill 是注入 prompt 的**知识/流程指引**；MCP 是提供**可调用工具**的外部服务。两者互补：skill 告诉 agent 怎么用，MCP 提供能力。

### B.2 对话 agent（自然语言创建 kanban 任务）

v1 不实现。规划形态：复用现有对话窗口（§12.11），新增 `create_task` 工具，把自然语言转成结构化任务（title / description / project_id / depends_on / review_mode），并复用 `POST /tasks` 的全部校验（循环依赖检测、worktree 准入）。v1 仅保留扩展点，不定义模型 / 工具集 / 存储 / UI 归属。

### B.3 离线通知渠道

Webhook / 邮件 / 飞书 / Slack。v1 只做 SSE 应用内通知；`NotificationPolicy`（cooldown、quiet_hours）结构已保留，渠道实现留待 v2。

### B.4 自适应强制超时

v1 的自适应 P50/P90 仅用于进度展示与告警（决策 66）。若将来要用自适应值作为强制阈值，需先解决"挂死节点耗时长会自我抬高阈值"的反馈回路问题。

### B.5 其他延后项

| 项 | 说明 |
|---|---|
| `human_if_risk` review 模式 | v1 只支持 `agent` / `human`（决策 25） |
| 远程仓库 push / PR | v1 纯本地合并（决策 6）；合入后可选 push 但不建 PR |
| 桌面端 Tauri 包装 | v1 纯 Web（决策 16） |
| 多项目目录维度 | 当前路径不含 project_id，靠前端过滤（决策 58） |
| 系统级沙箱 | v1 只有 `FileToolPolicy`（文件工具层面），shell 不受限（决策 104 / 19 修订）；OS 级 confinement 延后 |
| 密钥加密存储 | v1 明文（决策 112），仅靠目录权限；加密方案已评估并否决 |
