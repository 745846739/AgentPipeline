# 02: frontmatter 四键解析

**What to build:** 技能 frontmatter 从「只做文本剥离」升级为解析四个键：`description`、
`disable-model-invocation`、`license`、`allowed-tools`。前两者进入语义（描述进技能目录、
`disable-model-invocation: true` 的技能不被自动注入）；后两者只解析不生效——`allowed-tools` 在
Agent Skills 规范里标记为实验性，本系统没有「工具权限授予」这一层。

实现为逐行 `key: value` 的轻量解析，**不引入 YAML 依赖**（沿用决策 170 的姿态）。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] 逐行 `key: value` 解析，无 YAML 依赖；无 frontmatter 块时四键全按缺省处理
- [x] `description` 可被读取（供技能目录展示）
- [x] `disable-model-invocation: true` 被识别并作为「不自动注入」的判据（消费方在后续票落地）
- [x] **`name` 若写在 frontmatter 且与目录名不一致 → 启动 fail fast**（对齐 Agent Skills 规范
      「name 必须与父目录同名」；名字是唯一身份，这条不变量不能被 frontmatter 悄悄覆盖）
- [x] frontmatter 解析失败（畸形、未闭合）**不** fail fast，按缺省处理，正文仍可正常读取
- [x] 单测：四键各一例 / 键缺失 / frontmatter 未闭合 / `name` 与目录名不符被拒
- [x] 既有 frontmatter 剥离行为的测试保持通过（正文内容不受解析影响）

**Notes（实现提示）:**
- 解析结果的载体应是 `Skill` 结构的一部分（`description` / `disable_model_invocation` 等字段），
  而不是在渲染处现读文件——目录态与 `disable-model-invocation` 判定都要用它。
- 值只做最朴素的 trim 与去引号处理；布尔只认 `true` / `false`，其余按缺省。

## Comments

**实现（2026-09-14）**

- 新增 `SkillFrontmatter`（五个字段：四键 + `name`）并挂到 `Skill` 上；`parse_frontmatter(raw)`
  返回（解析结果，正文），逐行 `key: value`、无 YAML 依赖，值只 trim + 去成对引号。
- `name` 一致性校验落在 `skills::validate_names`，由 `validate_startup` 在提供 `skills_root`
  时调用——报错同时点出目录名与 frontmatter name。
- **三处判定口径值得记一笔（都写进了文档注释）：**
  1. 布尔只认字面 `true`；`yes` / `1` / `True` 一律按缺省。
  2. 未闭合 / 畸形 frontmatter → 全缺省，正文照读（不 fail fast）。
  3. **正文剥离沿用历史的宽松前缀判定（只看开头 `---`），但键的解释与 `name` 校验只认
     「开头 `---` 自成一行」的规范块。** 否则一段以水平线开头、正文里含 `name:` 字面文本的
     正常 Markdown 会被误判为名字不符而拒绝启动——把一次无害的宽松解析升级成启动失败。
     `non_standalone_dashes_are_not_treated_as_frontmatter` 钉住这条。
- `disable_model_invocation` / `license` / `allowed_tools` 三个字段目前**只解析、无消费方**
  （判据消费在票 05，另两项按决策 172 明确不生效）——结构就位即本票范围，票 05 接上。
