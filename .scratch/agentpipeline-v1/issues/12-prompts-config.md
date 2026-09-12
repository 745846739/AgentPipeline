# 12: Prompt 模板与阶段配置消费

**What to build:** 让执行中的节点拿到真实的 prompt 与配置：§10.3 十阶段节点模板内嵌为默认、prompts/ 用户覆盖、AGENTS.md 内容进 system prompt（G3）、worktree/任务目录绝对路径注入（G12）、SystemBaseline 最小工具基线（G6）、stage_configs 的 persona/temperature/tools 真正参与 prompt 组装与工具集构建（当前这些字段无人消费）。

**Blocked by:** 11（执行器骨架——需要 execute_node 的组装调用点）

**Status:** done（commit 6f47f1b + review 修正）

- [x] 十阶段全部节点模板内嵌（agents.md §10.3），prompts/{stage}/{node}.md 覆盖生效 —— `agent/templates.rs`（12 个 agent 节点 system+user 全量内嵌，只引用已声明变量的守卫测试）；覆盖优先级：persona_path > prompts/ 覆盖 > 内嵌
- [x] AGENTS.md 加载进 agents_context（G3）；G12 绝对路径注入 user prompt —— `load_agents_context`（缺失回退非空默认）；system「工作目录」段 + user「环境路径」段共用 `workdirs_line` 防漂移
- [x] SystemBaseline：mandatory/forbidden 工具 + 增量并集（G6，决策 104 配套）—— `agent/baseline.rs`：`(mandatory ∪ 声明) − forbidden`，mandatory 不可移除（有守卫测试）；声明未实现的工具 tracing::warn 后忽略（v1 无插件工具）
- [x] stage_configs 消费：persona_path/persona_append、temperature、max_tokens、tools_json/skills_json 进入组装（决策 22/46/111）—— temperature/max_tokens 透传 `LlmRequest`（生产适配器票 13 消费）；skills 与基线取并集，不存在 skill 的 fail fast 在 `validate_startup`
- [x] prompt_template_hash 反映覆盖后的最终组装内容（决策 137）—— 对 AGENTS.md / 工作目录 / 技能 / persona 覆盖均敏感（测试断言）
- [x] 模板顺序 golden 测试更新 —— golden 顺序升级为 `[基线前言][工作目录][AGENTS.md][persona][技能清单][格式规则]`，insta 快照同步

**Notes（code-review 结论）:**
- §10.6.4 启动 fail fast 已补：`validate_startup` 新增 `home_root` 输入，persona_path 不可读/为空拒绝启动；运行时 `resolve_stage_persona` 保留同检查兜底（防手工改库）。
- 遗留（低优先，未修）：代码字段名 `persona_path` 与 §10.6.3 文档名 `persona.system_prompt_path` 不一致（DB schema 定于票 02，改名牵涉 API 契约，建议文档向代码对齐）；伪阶段（project_analysis/conflict_check/validator_cross_check）复用同一 persona 机制但未在 executor 接入——归票 16。
