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

> **技能现状（2026-09-16 对齐，决策 172 / 181 / 185 / 187 / 194）**：技能的落地形态**不再是「注入 prompt 的一段文本」**——内嵌技能已退场（决策 172①，二进制不含任何正文），来源是**用户 markdown + 技能市场**（本地导入 / **GitHub 仓安装**；PATH 工具型技能随决策 185 退场——二进制由 `run_command` 与系统权限管，不是技能）；**仓名单**（`owner/repo`，不再是 registry 的 origin 白名单）在「设置 · 技能市场」页上可改——决策 187 那两级结构（保存即生效 / 清掉回配置）由**决策 194 继承**，而自定 `/index.json` registry 那一层整层退场（权威身份改为 commit SHA）；渲染分**三态**（全文 / 名字 / 目录），只有全文态的正文进 system prompt，名字态的正文由 `Skill` 工具（决策 172③）**按需拉取**——`Skill` 因此成了与 MCP 同类的东西：**可调用的能力**，只不过它的返回值是知识而不是动作。上表的 `build_stage_tools` 形态对 MCP 仍成立（MCP 本身仍是 v2 预留）。

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

### B.8 界面整备的待改进项（UX 审计，决策 195–203）

一轮 UI/UX 审计的 27 张票在 v1 全部落地（取舍与对账见 `.scratch/ux-audit/DELIVERY.md`），
下面这些是那轮**主动推后或做不到**的，从本轮记录搬过来，避免下一轮重开。

**两件需要先定方案再动手的**

1. **隐喻词表的落地缺口（决策 200）**。词表给了 12 个词的定稿说法，但其中 7 个
   （`工头` / `对讲台` / `值班经理` / `信号灯` / `回流带` / `道具栏` / `台账`）在本站
   **没有可落的位置**：它们要么只作为页标题、顶栏控件名或发言者称谓出现（而同一决策的
   裁决 ④ 明说标题与第一屏控件不翻译），要么只在代码注释里出现。`台账` 还多一层问题——
   它在「设置这一类页面」这个义项上已按定稿说法落地，但 `Talk.svelte` 里
   「值班长正在查台账…」用的是**对话记录**这个义项，照抄定稿说过去会说错。
   要收口得先回答一件事：**词表是「一个词一条注释」还是「一个义项一条注释」**；
   按义项立表的话还得给每条一个「适用位置」判据，否则下一次仍会两边都说不过去。
2. **依赖任务 ID 的完整选择器**。v1 给了原生 `datalist`（票 05），它按**整串**过滤，
   所以填第二个依赖时不给候选——而票面同时要求「候选只对正在输入的那一段生效」。
   原生控件做不到这一点，做它就得自绘下拉（键盘可达、像素纪律、移动款各一套），
   本轮按 spec 的 Out of Scope 让路（「把依赖任务 ID 做成完整选择器——等开源后真有反馈再说」）。
   真要做时**这两条要求要先合并成一条**，别再一次只落一半。

**其他延后项**

| 项 | 说明 |
|---|---|
| 移动款底部动作坞遮住拍板内容 | 用户故事 30，本轮明确推后：常驻底部动作坞是视觉规格 §5 有意的设计形态，改它属于移动款版面变更（决策 192 只重做了对讲台的窄屏版面） |
| 共享基元：空态与提示块的容器 | `<EmptyState>` 是统一了，但**它外面那层包裹**没统一——`.banner` / `.blank` / `.gate` / `.no-stop` 在七个页面各写一份，内距与色档已开始漂移（本轮为了不破既有 e2e 定位器而沿用各页容器）。译文串同样在各页各持一份（这是「按页面首现」的口径决定的，抽共享常量模块反而错，但**同义改字时要记得七处一起改**） |
| 空态窄屏版面的 e2e 断言 | 票 13 点名要求，本轮由既有 `talk.spec.ts` 的窄屏几何用例覆盖（它正是抓到页头回归的那条），没有在 13 号票下补一条专门的 |
| 「配对之后从手机看」这一档的验收截图 | spec 明确推后（只影响文案类发现） |
| 证据产物的输出前缀 | `e2e/ux-audit.spec.ts` 用**固定文件名**输出到 `.scratch/ux-audit/*.png`，重跑一次就覆盖上一轮——本轮因此**没拍收敛后的截图**（那些图是决策 195–203 的判据来源，不能覆盖）。改法很小：给产物加一个轮次前缀或子目录，并同步放宽 `.gitignore` 的那条规则 |
| `.scratch/ux-audit/` 是否随仓库公开 | 已核实它不被整目录排除（只排除 `*.png`），随不随公开属维护者决定 |
| 代码卫生两处 | `components/ui/Modal.svelte` 的 `WeakSet` 事件去重与遮罩的 `stopPropagation` 是同一件事的两道机制（后者已足够）；`lib/pipeline.ts` 的 `spineBeltsAll` / `spineColumnX` 目前只有几何守护在用。两处都不影响行为，属清理项 |
