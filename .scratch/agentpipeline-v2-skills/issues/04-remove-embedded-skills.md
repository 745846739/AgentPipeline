# 04: 移除内嵌技能

**What to build:** 删掉 `EMBEDDED_SKILLS` 常量、两个流水线原生改写正文、`SkillSource::Embedded`
变体，以及 `body_of` 里指向内嵌的兜底分支。技能来源收敛为：用户 markdown（技能根内
`{name}/SKILL.md`）、PATH 可执行文件（工具型，只见名字）。

这是宽重构的 **contract 阶段**：前置票已把断言迁到用户目录 fixture，此刻没有任何调用方还依赖内嵌。

**Blocked by:** 03（契约迁移——迁移未落地就删内嵌，那批按名字钉住 `grilling` / `to-spec` 的断言会同时变红，中间无绿灯）

**Status:** ready-for-agent

- [ ] `EMBEDDED_SKILLS` 常量与两个改写正文（`GRILLING_BODY` / `TO_SPEC_BODY`）删除
- [ ] `SkillSource::Embedded` 变体删除；`discover` / `body_of` / `resolve` 相应分支收敛
- [ ] 技能来源只剩「用户 markdown ∪ PATH 工具型」，`discover` 的两类合并逻辑与同名让位规则保留
- [ ] `body_of` 在用户目录找不到文件时**不再有内嵌兜底**：知识型技能名不存在 → 启动校验 fail fast；
      PATH 工具型 → 仍返回 `None`（只列名字）。**不得**出现「名字存在但静默降级成空子弹」的路径
- [ ] 全仓 `grep` 确认无 `EMBEDDED_SKILLS` / `Embedded` 残留引用
- [ ] 全仓测试通过（`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`）

**Notes（实现提示）:**
- 移除内嵌**不影响默认行为**：`BASELINE_MANDATORY_SKILLS` 为空、且 `stage_configs` 无种子行，
  故没有任何阶段默认引用这两个技能。真实影响面只有「可用池少两个名字」与「引用它们的配置会
  fail fast」——后者是期望行为（技能名是唯一身份）。
- 推荐默认技能由配置界面承载（票 16），不在二进制里留任何技能正文——这同时解掉上游内容的
  再分发授权问题（27 个上游技能中仅 1 个带许可声明，而本仓是 MIT）。
