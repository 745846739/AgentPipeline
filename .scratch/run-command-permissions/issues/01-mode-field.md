# 01: 档位字段与分层——三档、两层、默认等于现状

**What to build:** 给 `stage_configs` 加一个**环境层权限档位**列（迁移 0016，落地时以当时实际的空号
为准——0012 归会话、0013 / 0014 归续接原因驱动、0015 归提议表；列名 `env_mode` 或 `command_mode`，
落地时定一个并写进交付说明），取值 `auto` / `ask` / `deny`。
逐层加上：`StageConfig`（`types.rs:1095-1114`）、`storage/catalog.rs` 的行映射与 upsert 绑定、
`PUT /stage-configs`（`routes/stage_configs.rs:41-66` 与 `put` 的 candidate 构造）、前端
`api/types.ts` + `lib/stageConfigs.ts`（草案字段、`emptyStageConfigDraft`、`draftFromStageConfig`、
`buildStageConfigPut`）+ `StageConfigForm.svelte` 一行下拉。

**分层两层**：全局默认在 `config.toml` 的 `[pipeline]`（不进 UI，照 `allow_dirty_worktree_merge`
那一批今天的做法），阶段级覆盖它。**不做节点级覆盖**（决策 206）。

**默认值**：真实阶段 `auto`——**等于今天的现状**，这是「落地不改变任何已运行行为」的兑现，交付说明里
要写出核对结论；`foreman` 行 `ask`。

**`deny` 是「不广告」**：`tool_defs` 里把该阶段不该有的工具摘掉（节点侧 `executor.rs:3254-3318`、
值班长侧 `foreman.rs:550-580`），执行点再拒一次兜底。

**验证**：非法档位字符串在写入时拒（照 `SkillMode::parse` 的姿态），前端下拉与之同步。

**Blocked by:** None（可立即开始——本票只加一列，不依赖提议接缝）

**Status:** ready-for-agent

- [ ] 迁移 + 结构体 + catalog 映射 + `PUT /stage-configs` 全链路
- [ ] 非法档位字符串写入时被拒（有测试）；前端下拉与后端取值同步（不出现前端能选、后端拒绝的值）
- [ ] 全局默认 + 阶段级覆盖两层，**没有节点级**
- [ ] 默认值等于现状：什么都不配时，流水线各阶段的行为与今天逐字相同（交付说明写出逐点核对结论）
- [ ] `deny` 从**广告**的工具表里摘掉，而不是只在执行点拒（有测试断言广告集）
- [ ] `foreman` 行默认 `ask`
- [ ] 与 `.scratch/resume-semantics/02` 错开落地（两者改同一批文件，一个删列一个加列）
