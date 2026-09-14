# 08: 只读子代理

**What to build:** 新增工具 `spawn_sub_agent`，复用 `agent_attempt` 的循环子集，**独立 context**——
把「读 20 个文件」的原文挡在父上下文之外，只回摘要。这让 `research`、`code-review`（两轴并行）、
`codebase-design`（DESIGN-IT-TWICE）这批以子代理为前提的技能真正跑起来。

工具集**固定为只读**（`read_file` / `list_dir`），这是安全边界的核心：子代理不能 `run_command`
（否则成为注入攻击的加速通道，而本系统无 OS 级沙箱）、不能写文件、**不继承阶段声明的工具**
（防阶段配置扩权）。

**Blocked by:** None (can start immediately)

> 本票不依赖技能运行时：它只复用既有 agent 循环与只读工具。排在技能票之后是**排序偏好**
> （子代理的典型用途是执行技能指令），不是硬依赖——可并行开工。

**Status:** ready-for-agent

- [ ] 工具 `spawn_sub_agent` 可用，入参含任务描述，返回摘要文本
- [ ] 子代理工具集**固定只读**：仅 `read_file` / `list_dir`；无 `run_command` / `write_file` /
      `edit_file` / `delete_file` / `submit_metadata`
- [ ] **不继承**阶段声明的工具（阶段配置无法给子代理扩权）
- [ ] 深度限制**一层**：子代理不再获得 `spawn_sub_agent`
- [ ] 子代理的 prompt 构建复用既有链路（`AGENTS.md` 加载、工作目录、persona 可简化为摘要任务前言）
- [ ] 落 run 行：`agent_type = "subagent"`、`parent_run_id` 指向父 run（两列均已存在，**无迁移**）
- [ ] 会话落 `kanban_node_conversations`，同样带 `agent_type` / `parent_run_id`
- [ ] token 记在**子代理自己的 run 行**上并计入任务总量；父 run **不重复累加**子代理用量
- [ ] 超时沿用节点级 `node_idle_timeout_sec` / `node_max_duration_sec` 作为该次调用上限
- [ ] 集成用例：FakeAgent 驱动「父请求派子代理 → 子代理只读工具 → 返回摘要进父 messages」；
      另有一条断言「子代理拿不到 `run_command`」
- [ ] **重开决策 154 的边界写清**：只读子代理是新增能力，「L4 兜底只有两级」与「分批 / 拆子代理
      不作为上下文超限兜底」的原裁决**不变**

**Notes（实现提示）:**
- 复用 `agent_attempt` 的循环子集而非重写：LLM 调用、工具循环、消息累积都已具备。
- 与「上下文超限兜底」的关系必须保持独立——本票给的是**技能可调用的能力**，不是 L4 兜底手段。
  决策 154 删除 `L4Plan` / `L4Action` 的历史不受影响。
