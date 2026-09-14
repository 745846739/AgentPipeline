# markdown 知识型技能 + 节点级技能（agentpipeline-markdown-skills）

来源：2026-09-14 用户诉求「给架构设计阶段配置一个 grill-me 和 to-spec 的 skill」。
裁决落 **决策 170**（修订决策 47）。本目录承接该裁决的执行面。

**Status:** done（2026-09-14）

## 背景：为什么不是一条配置就能解决

本系统的 skill 原语义（决策 47）是「用户机器上装了对应的外部工具」——扫 PATH 找可执行
文件，**只把名字**列进 system prompt。而 `grill-me` / `to-spec` 是 ZCode 的 markdown 技能
（非可执行文件），照字面写进 `skills_json` 会被启动校验 fail fast 拒绝；即使放过，agent
也只会看到一个空名字，正文永不进 prompt。

另有两条必须处理的事实：

- **`grill-me` 在 ZCode 里只是存根**——内容是「Call the Skill tool with "grilling"」，
  而本系统没有 Skill 工具，照搬进来是一句无法执行的指令。真正要改写的是 `grilling` 协议。
- **`skills_json` 是阶段级的**，无法区分节点；而 architect-design 的三个节点职责互斥
  （validate_input 提问 / execute 写 `design.md` / validate_output 校验）。阶段级注入
  会把「不断向用户提问」塞给写文件的节点。

## 范围一句话

把 skill 从「PATH 外部工具的名字」扩展为**三类来源、名字唯一身份、可携带 markdown 正文
并注入 prompt**，并新增**节点级声明**（`node_overrides_json[node].skills`，只增不减）；
两个内嵌技能 `grilling` / `to-spec` 是**流水线原生改写**（适配异步 pending 回路、
守住决策 136 的验收标准），配到 architect-design 的 validate_input / execute 上。

## 明确不立票的项

- **`GET /skills` 端点与设置页技能选择器**：当前设置页 `skills_json` 是自由文本输入，
  `SettingsProviders.svelte` 仅显示「已配置 / —」。发现可用技能不做 UI 呈现不影响本次目标。
- **`[skills] dir` 配置覆盖**：`[prompts] dir` 已有先例（票 16），但本次技能根固定为
  `{home}/skills`，够用；引入第二个可覆盖根会扩大配置面。
- **伪阶段技能注入**：`call_pseudo_stage` / `project_analysis` 两处调用点保持传 `&[]`，
  伪阶段（conflict_check / validator_cross_check / project_analysis）不参与技能声明。

## 票

| 票 | 内容 | 状态 |
|---|---|---|
| [01](issues/01-skills-module-and-validation.md) | `skills.rs` 模块（三类发现 + resolve + 内嵌正文）与校验扩展 | done |
| [02](issues/02-node-scoped-skills-and-injection.md) | 节点级技能声明 + executor 注入 | done |
| [03](issues/03-config-landing-docs-and-tests.md) | 配置落地 + 文档 + 测试锚点 | done |

## 关键约束

- 技能**只增不减**：有效集 = `mandatory ∪ 阶段级 ∪ 节点级`，与 §10.6.4 并集语义一致。
  任何让节点级配置能**削减**阶段级技能的实现都是错的。
- 正文「存在且非空」fail fast，口径与 `persona_path` 一致；`StartupInputs.skills_root`
  照抄 `home_root` 的 `Option<PathBuf>` + `None` 跳过模式，老调用点零改动。
- `to-spec` 的正文必须显式要求保留 §10.3 必需节与决策 136 的「验收标准」编号清单——
  否则 validate_output 的检查项与下游 traceability 会断。
- 与决策冲突时必须显式标注决策编号（AGENTS.md）。本批次的权威裁决是决策 170（修订 47）。
- 质量闸门：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`；涉及前端加跑 vitest / svelte-check / build。
