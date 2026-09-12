# 12: Prompt 模板与阶段配置消费

**What to build:** 让执行中的节点拿到真实的 prompt 与配置：§10.3 十阶段节点模板内嵌为默认、prompts/ 用户覆盖、AGENTS.md 内容进 system prompt（G3）、worktree/任务目录绝对路径注入（G12）、SystemBaseline 最小工具基线（G6）、stage_configs 的 persona/temperature/tools 真正参与 prompt 组装与工具集构建（当前这些字段无人消费）。

**Blocked by:** 11（执行器骨架——需要 execute_node 的组装调用点）

**Status:** ready-for-agent

- [ ] 十阶段全部节点模板内嵌（agents.md §10.3），prompts/{stage}/{node}.md 覆盖生效
- [ ] AGENTS.md 加载进 agents_context（G3）；G12 绝对路径注入 user prompt
- [ ] SystemBaseline：mandatory/forbidden 工具 + 增量并集（G6，决策 104 配套）
- [ ] stage_configs 消费：persona_path/persona_append、temperature、max_tokens、tools_json/skills_json 进入组装（决策 22/46/111）
- [ ] prompt_template_hash 反映覆盖后的最终组装内容（决策 137）
- [ ] 模板顺序 golden 测试更新（基线前言 → AGENTS.md → persona → 格式规则）
