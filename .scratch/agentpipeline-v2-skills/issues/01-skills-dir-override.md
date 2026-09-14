# 01: `[skills] dir` 覆盖技能根

**What to build:** 用户可在 `config.toml` 用 `[skills] dir` 把技能根指到别处（如 `~/.zcode/skills`），
该目录下的技能被发现、校验、声明、注入——全链路可用。未配置时回落 `{home}/skills`，行为逐字不变。
照 `PromptsConfig::resolved_dir` 的既有先例（票 16）实现：相对路径按 home 解析、空白 → `None`。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] `Config` 增 `[skills] dir` 字段与 `resolved_dir(home_root)`；绝对路径原样、相对路径接在 home 下、空白值 → `None`
- [x] 未配置时技能根仍是 `{home}/skills`，既有行为零变化（现有测试全绿）
- [x] 配置后该目录的技能进入可用集——`discover_available_skills` 与 `skills::discover` 走同一入口，不新增第二个发现路径
- [x] 启动校验与 `PUT /stage-configs` 对外部目录的技能同样 fail fast（名字不存在、知识型正文为空/不可读）
- [x] 单测：路径解析三态（绝对 / 相对 / 空白）；指到临时外部目录后技能可被声明并出现在 system prompt 的技能段
- [x] `Home` 的默认技能目录（`skills_dir`）语义不变，仍纳入 `ensure_dirs`

**Notes（实现提示）:**
- 不引入第二个可覆盖根之外的概念：技能根只有一个，`[skills] dir` 是它的唯一覆盖点。
- 校验口径与 `persona_path` 一致（存在且非空），沿用 `StartupInputs.skills_root` 的 `Option<PathBuf>`
  + `None` 跳过模式，老调用点零改动。

## Comments

**实现（2026-09-14）**

- `skills.rs` 的每个入口参数由「home 根」改为「技能根**本身**」（`discover` / `skill_names` /
  `resolve` / `skill_file_path`），根不再自己拼 `skills` 目录名——所以 `[skills] dir` 是唯一覆盖点。
  四个生产调用点全部改走 `Home::skills_dir()`：`executor.rs`（正文注入）、`storage/catalog.rs`
  与 `routes/stage_configs.rs`（发现 + 启动校验）。
- `Config` 增 `SkillsConfig`（照 `PromptsConfig` 先例，含 `deny_unknown_fields`）；
  `Home` 增 `skills_override` 与 `with_skills_dir`；`serve.rs` 接线。
- **一处与直觉相反的取舍：`ensure_dirs` 仍建默认 `{root}/skills`，而非 `skills_dir()`。**
  覆盖目录通常是用户自己维护的技能生态目录（`~/.zcode/skills`），本系统不该新建它、更不该把它
  `chmod 0700`。已用 `ensure_dirs_never_touches_overridden_skills_dir` 钉住。
- 未配置时行为逐字不变的证据：核心库 302 项测试全绿；`prompt_template_hash` 所依赖的正文
  行尾口径与旧 `strip_frontmatter` 完全一致（`crlf_body_line_endings_are_normalized` 钉住）。
