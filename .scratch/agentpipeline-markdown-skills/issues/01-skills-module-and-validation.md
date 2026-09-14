# 01: skills 模块与校验扩展

**What to build:** 新增 `crates/core/src/agent/skills.rs`，把 skill 从「PATH 外部工具的名字」
扩展为**三类来源、名字唯一身份**：内嵌默认（`EMBEDDED_SKILLS`）、用户 markdown
（`{home}/skills/{name}/SKILL.md`，同名覆盖内嵌）、PATH 可执行文件（决策 47 原语义不变）。
知识型技能携带**正文**并在 `build_system_prompt` 的「已启用技能」段注入；工具型仍只列
`- {name}`。正文「存在且非空」在启动与写入时 fail fast（与 `persona_path` 同口径）。

**Blocked by:** None (can start immediately)

**Status:** done（2026-09-14，决策 170）

- [x] `skills.rs`：`SkillSource`（Tool / Embedded / Markdown）、`Skill`、`ResolvedSkill`、
      `discover()` / `skill_names()` / `resolve()` / `skill_file_path()`；
      `EMBEDDED_SKILLS` 内嵌 `grilling` / `to-spec` 正文
- [x] frontmatter 只做**文本剥离**（`---` 包围块），不引 YAML 依赖；`name` 取目录名
- [x] 用户 markdown 正文为空 → `Error::Config`（`empty_user_file_is_config_error`）
- [x] 用户文件同名覆盖内嵌，且来源登记为 `SkillSource::Markdown`（`user_markdown_overrides_embedded_body`）
- [x] 工具型技能无正文 → `resolve` 返回 `body: None`，渲染不变（`tool_skill_has_no_body`）
- [x] `Home::skills_dir()` 并纳入 `ensure_dirs`（0700）
- [x] `discover_available_skills(home_root)` 加参数并委托 `skills::skill_names`；
      两个调用点（`storage/catalog.rs` / `routes/stage_configs.rs`）同步
- [x] `StartupInputs` 新增 `skills_root: Option<PathBuf>`（照抄 `home_root` 的 `None` 跳过模式）
- [x] `build_system_prompt` 收 `&[ResolvedSkill]`：无正文 `- {name}`、有正文 `### {name}` + 正文；
      段落位置不变（golden 顺序测试保持通过）
- [x] L1 用例：三类发现 / 同名覆盖 / frontmatter 剥离 / 空正文拒绝 / 去重保序 /
      两种渲染 / `prompt_template_hash` 对正文敏感

**Notes（实现结论）:**
- 决策 47 的 PATH 扫描逻辑整体迁入 `skills::path_tool_names`，语义未变——`rtk` 这类工具型
  技能的行为与旧版逐字相同（`- rtk` 子弹），保证向后兼容。
- 工具型与知识型**同名时让位给知识型**（有正文的更具体）；当前内嵌两个名字（`grilling` /
  `to-spec`）在 PATH 中不存在同名可执行文件，故该分支暂无现实触发路径，但返回值是确定的。
- `strip_frontmatter` 最初用 `.leak()` 返回 `&str`，改为返回 `String`——避免每次读技能泄漏内存。
