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

**Status:** done

- [x] 迁移 + 结构体 + catalog 映射 + `PUT /stage-configs` 全链路
- [x] 非法档位字符串写入时被拒（有测试）；前端下拉与后端取值同步（不出现前端能选、后端拒绝的值）
- [x] 全局默认 + 阶段级覆盖两层，**没有节点级**
- [x] 默认值等于现状：什么都不配时，流水线各阶段的行为与今天逐字相同（交付说明写出逐点核对结论）
- [x] `deny` 从**广告**的工具表里摘掉，而不是只在执行点拒（有测试断言广告集）
- [x] `foreman` 行默认 `ask`
- [x] 与 `.scratch/resume-semantics/02` 错开落地（两者改同一批文件，一个删列一个加列）

## 交付

- 迁移 `0016_env_mode.sql`：`ALTER TABLE stage_configs ADD COLUMN env_mode TEXT CHECK (env_mode IS NULL
  OR env_mode IN ('auto','ask','deny'))`（SQLite 支持带 CHECK 的 ADD COLUMN，写错的值进不来）。
- `EnvMode` 落在 `types.rs`（`as_str` / `parse` **严格、不静默降级** / `default_for` / `parse_or_message`），
  两层解析的唯一实现是 `effective_env_mode(全局默认, 阶段, 阶段行)`；`Settings.env_mode` 缺省 `Auto`
  （**等于现状**），`Config::validate()` 在解析期 fail fast。
- 全链路读写：`storage/catalog.rs` 的 `StageConfigRow`（**读宽松**——列被手工改坏时退回缺省，不在读路径上炸）、
  upsert 绑定、`PUT /stage-configs` 的字符串入参（照 `SkillMode::parse` 的姿态报「只能是 auto / ask / deny」）、
  前端 `StageConfigForm` 的三档单选 + `lib/stageConfigs.ts` 的草稿/payload。
- **一处补门**（规格 §4）：`ask` **只留给值班长**——`config.toml` 的全局默认与 `PUT /stage-configs`
  都拒非 `foreman` 行的 `ask`（报文说清该配 `deny`、以及该配在哪一行），表单也不摆那个选项。
  理由：流水线节点无人按那颗钮、又没有提议通道，配成 `ask` 的结果是静默收掉这个阶段全部的环境写动作。
- 取证：`tests/env_mode.rs::the_default_tier_keeps_everything_as_it_is_today`（真实阶段 auto、值班长 ask）、
  `a_stage_row_overrides_the_global_default`、`env_mode_parsing_is_strict`；`config.rs` 的
  `the_global_tier_accepts_auto_and_deny_but_not_ask`；契约层的
  `stage_config_env_mode_round_trips_and_rejects_junk` / `only_the_foreman_row_may_be_configured_as_ask` /
  `the_foreman_stage_accepts_an_env_mode_too`。
