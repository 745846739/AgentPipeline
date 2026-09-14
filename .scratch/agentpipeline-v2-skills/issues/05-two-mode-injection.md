# 05: 二档注入与技能字段形态

**What to build:** 启用技能时可选择「注入全文」或「只注入名字」；技能声明字段由 `string[]` 扩展为
`string | {name, mode, trusted}` 的混合数组，裸字符串按 `{mode: "full", trusted: false}` 解释
（**向后兼容今天的配置行**）。渲染分三态：目录态（未声明、仅在可用池，`- {name}: {description}`）、
名字态（`- {name}`，正文由 `Skill` 工具按需拉取）、全文态（`### {name}` + 正文）。

**Blocked by:** 02（frontmatter 四键解析——目录态需要 `description`，不信注入需要 `disable-model-invocation`）

**Status:** ready-for-agent

- [ ] 混合数组解析：裸字符串 → `{mode:"full", trusted:false}`；对象形态校验 `mode ∈ {full, name}`，
      非法值报错定位到阶段/节点
- [ ] 三态渲染落地，`[技能清单]` 段位置不变（golden 顺序仍为
      `[基线前言][工作目录][AGENTS.md][persona][技能清单][格式规则]`）
- [ ] `disable-model-invocation: true` 的技能**不进目录态**、不被自动注入
- [ ] 未信任技能（`trusted: false`）**不得**以 `mode:"full"` 保存——写入时拒绝并报错
- [ ] 有效集仍为 `基线 ∪ 阶段级 ∪ 节点级`（**只增不减**不变量保留）
- [ ] 旧配置行（纯字符串数组）行为逐字不变——现有 `skills_json` / `node_overrides_json` 数据零迁移
- [ ] `prompt_template_hash` **只对全文态敏感**：名字态与目录态的技能名变化不应造成同样的正文级敏感
      （正文不进 system prompt 时 hash 不该随之抖动）
- [ ] 单测：混合数组解析 / 三态渲染 / 非法 mode 拒绝 / 未信任 + full 拒绝 / 旧格式兼容 / golden 顺序

**Notes（实现提示）:**
- 字段形态的解析应集中在一处（`config` 层的技能声明读取函数），executor 与启动校验共用，
  避免两套解读。节点级（`node_overrides_json[node].skills`）与阶段级用同一解析器。
- 三态里的「目录态」是本票唯一的**新增可见输出**，也是渐进披露的落点：模型据此知道有哪些能力
  可用，而不必预载全部正文。
