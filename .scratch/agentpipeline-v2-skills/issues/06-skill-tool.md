# 06: `Skill` 工具

**What to build:** 新增第 8 个内置工具 `Skill`，入参 `{name}`，把该技能的正文作为 **tool result**
注入 `messages`（**不进 system prompt**，因此不影响 `prompt_template_hash`）。

工具名与上游同名是**功能性决定而非命名偏好**：上游技能的正文里写着
`Call the Skill tool with "grilling"`，同名使这些正文**无需改写即可执行**——这是「不要求技能适配」
这条选型的直接兑现。

**Blocked by:** 05（二档注入——名字态与目录态是它的主要使用场景）

**Status:** ready-for-agent

- [ ] `BUILTIN_TOOLS` 由 7 项扩为 8 项（`Skill`）；**不加入 `MANDATORY_TOOLS`**，由阶段声明启用
- [ ] 工具分发 `match` 增一条：解析技能名 → 取正文 → 作为 tool result 返回
- [ ] 调用后**下一轮 `LlmRequest.messages` 里出现该技能正文**（这是本票的核心可观察行为）
- [ ] 未声明的技能也能被加载（渐进披露的自动触发路径）；但 `disable-model-invocation: true`
      与未信任技能除外
- [ ] 未知技能名 → 工具返回错误文本（**不** fail fast，让模型自行纠正）
- [ ] 正文不进 system prompt：调用前后 `prompt_template_hash` 不变
- [ ] `tool_defs` 的放行闸只认 `BUILTIN_TOOLS`（非内置声明仍只 warn 忽略），故该常量是本工具
      唯一的启用点
- [ ] 集成用例：FakeAgent 脚本驱动「模型请求 `Skill` → 工具返回正文 → 下一轮 messages 含正文」

**Notes（实现提示）:**
- tool result 的注入路径已存在（工具结果 `push` 进 `messages`），本票不需要改造 agent loop——
  循环结构天然支持「工具结果进上下文」。
- 该工具**不**受 `FileToolPolicy` 影响：它读的是技能根（loader 侧），不是 worktree——技能根
  **不得**被放宽为 agent 可读（`{home}/data/` 与技能目录同父，而 provider 密钥明文存储）。
