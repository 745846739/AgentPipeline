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

> **技能现状（2026-09-16 对齐，决策 172 / 181 / 185 / 187）**：技能的落地形态**不再是「注入 prompt 的一段文本」**——内嵌技能已退场（决策 172①，二进制不含任何正文），来源是**用户 markdown + 技能市场**（本地导入 / 远程 registry；PATH 工具型技能随决策 185 退场——二进制由 `run_command` 与系统权限管，不是技能）；来源白名单在「设置 · 技能市场」页上可改（决策 187）；渲染分**三态**（全文 / 名字 / 目录），只有全文态的正文进 system prompt，名字态的正文由 `Skill` 工具（决策 172③）**按需拉取**——`Skill` 因此成了与 MCP 同类的东西：**可调用的能力**，只不过它的返回值是知识而不是动作。上表的 `build_stage_tools` 形态对 MCP 仍成立（MCP 本身仍是 v2 预留）。

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
| 桌面端 Tauri 包装 | v1 纯 Web（决策 16）；形态已定：Tauri 只当外壳与打包器，传输层复用 HTTP + SSE、不重写为 Tauri IPC（决策 153，实现期防御约束见 frontend-design.md §4） |
| 多项目目录维度 | 当前路径不含 project_id，靠前端过滤（决策 58） |
| 系统级沙箱 | v1 只有 `FileToolPolicy`（文件工具层面），shell 不受限（决策 104 / 19 修订）；OS 级 confinement 延后 |
| 密钥加密存储 | v1 明文（决策 112），仅靠目录权限；加密方案已评估并否决 |
| 逃逸率自动归因（`escaped_from`） | v1 只提供逃逸率查询口径（决策 137）；把下游质量事件自动推断归属到具体上游闸门的标签列留 v2——推断准确率未经验证，不固化数据模型 |

### B.6 离线回放 eval（prompt 回归测试）

v1 已落 `prompt_template_hash`（决策 137），指标可按 prompt 版本对比；本项把"事后对比"升级为"事前拦截"：从历史终态任务中筛选 golden set（任务描述 + 任务目录产出 + 实际判定结果），用当前 prompt 模板离线重放关键判定节点（validate_input / validate_output / review.execute），对比判定结论与历史实际，输出通过率报告。改动 prompt 后先跑 eval 再投入使用，防止 prompt 回归。素材基础 v1 已具备：会话全量落库（§12.4.3）+ prompt 版本标注（决策 137）。

### B.7 post-merge 验证与 revert 任务模板

v1 合入即 done（决策 59），合入后的质量由 merge 闸门前置保障，无合入后验证。预留两项：① **post-merge smoke**——合入后可选触发轻量验证任务（复用 test 阶段的执行子集，针对合入后的 `default_branch` 跑冒烟测试）；② **revert 任务模板**——以 `git revert <merge-commit>` 为 init 基底的一键回滚任务类型，复用现有流水线走完整的 develop / review / test / merge 链路，回滚本身也受闸门保护。二者均不改 v1 状态机。
