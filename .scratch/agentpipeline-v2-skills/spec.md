# v2 技能运行时与技能市场（agentpipeline-v2-skills）

**Status:** done（票 01–03 全部落地）

> 来源：2026-09-14 用户诉求「给架构设计阶段配置一个 grill-me 和 to-spec 的 skill」经决策 170
> 落地后，暴露真实目标不是「配两个技能」，而是「把 Agent Skills 生态（ZCode / Claude Code
> 的 markdown 技能）整体接进来用」。本 spec 把该目标定形为 v2 范围：**技能运行时 + 技能市场**，
> 并让内嵌技能整体退场。经 20 轮拷问（Q1–Q20）逐项裁决。

## Problem Statement

决策 170 让技能能携带 markdown 正文并注入 prompt，但技能池**封顶在 2 个内嵌技能**，扩容只能
手工 `cp` 到 `~/.agentpipeline/skills/`。用户手上已有的 27 个 ZCode 技能一个都用不上，而其中
多数**在本系统里根本跑不起来**：

- **没有 Skill 工具。** 7 个技能的正文写着 `Call the Skill tool with "grilling"`，照装进来
  是一句无法执行的指令——`grill-me` 全文就这一句。
- **没有子代理。** 决策 154 把 `spawn_sub_agent` 定为「v2 候选能力，v1 无实现」，于是
  `code-review`（两轴并行）、`research`（后台 agent）、`codebase-design`（DESIGN-IT-TWICE）
  这批以子代理为前提的技能全部断腿。
- **加载器只读单个 `SKILL.md`。** `tdd/tests.md`、`prototype/UI.md`、`codebase-design/DEEPENING.md`
  这类兄弟文件既不进 prompt，agent 也读不到——文件工具被 `FileToolPolicy` 锁在 worktree +
  任务目录内。这些引用是**双向死指针**。

同时「推荐默认」无处投递、正文全量注入不可扩展：技能正文是**每轮常驻** system prompt 的，
27 个技能约 128KB，全量铺开既吃窗口又反复失效 prompt cache（决策 137）。

## Solution

把技能从「内嵌的两个改写版」升级为**可安装、可推荐的生态**：

1. **技能运行时**——新增 `Skill` 工具（按需拉取正文）、只读子代理、渐进披露（目录只放
   name + description）、兄弟文件一级展开。命名沿用上游的 `Skill`，使上游技能正文里的
   `Call the Skill tool with "..."` **原样变成可执行指令**，不必再改写技能内容。
2. **技能市场**——本地导入（含扫描 `~/.zcode/skills`）+ 远程 registry（`sha256` 校验 +
   来源白名单 + 装前预览），技能为用户级，可在各 Stage / Node 配置是否启用。
3. **配置界面推荐与一键安装**——按 Stage 推荐适合的技能，一键装好并写入该阶段配置；
   用户可停用推荐、可换成自己想用的任何技能。**不要求技能适配本系统**。
4. **内嵌技能退场**——`EMBEDDED_SKILLS` 与两个流水线原生改写版整体移除，规避上游内容
   再分发的授权问题与陈旧问题。

配套三项使「市场下载的技能」不至于立刻变成攻击面：注入模式二档（全文 / 仅名字）与信任
标记、`run_command` 的工具层出口控制、以及装前预览。

## 已确认的选型（本 spec 的边界）

| # | 决策 | 含义 |
|---|---|---|
| A | **不内嵌** | `EMBEDDED_SKILLS` 与内嵌改写版整体移除；技能只来自用户目录与市场。推荐默认由配置界面承载，不在二进制里 |
| B | **不要求适配** | 加载器不检查正文语义、不因内容拒绝任何合法 markdown；内容能否跑通由运行时能力决定 |
| C | **补齐运行时** | `Skill` 工具 + 只读子代理 + 渐进披露 + 兄弟文件展开。重开决策 154 的「v1 无实现」 |
| D | **二档注入 + 信任标记** | 启用技能时可配「注入全文」或「仅注入名字」；解析 `disable-model-invocation` 并默认标记为不可自动注入；未信任技能不得全文注入 |
| E | **市场与出口控制同批** | 市场准入自带可见性（装前预览 + 摘要 + 来源白名单）；`run_command` 加工具层网络策略，残余风险显式记账 |
| F | **会话续接参数化** | pending → resume 时是否续接上一 attempt 的对话由参数决定，**默认关** |

## User Stories

1. 作为在自己机器上跑流水线的开发者，我想在设置页按阶段看到「推荐技能」清单，所以我不必先知道有哪些技能可用就能用上它们。
2. 作为开发者，我想点一次「安装」就把推荐技能装好并写进该阶段配置，所以启用技能不是一串手工操作。
3. 作为开发者，我想停用任何推荐技能，所以「推荐」不是强加。
4. 作为开发者，我想把推荐技能换成我自己写的同名技能，所以我的版本能覆盖任何默认。
5. 作为开发者，我想把 `~/.zcode/skills` 里的技能一次导入，所以现有生态不用重打一遍。
6. 作为开发者，我想从远程 registry 搜索并安装技能，所以不必手工下载再拷贝。
7. 作为开发者，我想在安装前看到该技能会进哪些阶段与节点的 prompt，所以我能在装之前判断它是否合适。
8. 作为开发者，我想在安装前看到该技能正文里是否出现 `run_command`、网络调用、密钥路径字样，所以我能识别有风险的技能。
9. 作为开发者，我想每个技能带 `sha256` 摘要并在安装时校验，所以传输被篡改能被拦下。
10. 作为开发者，我想只允许从我在配置里放行的来源安装，所以不会误装陌生来源。
11. 作为开发者，我想新装技能默认标记为未信任且不得全文注入，所以恶意技能不能立刻进入我的 agent 上下文。
12. 作为开发者，我想在确认后把某个技能标记为已信任，所以我能显式承担这个决定。
13. 作为对成本敏感的开发者，我想每个启用技能单独选择「注入全文」还是「只注入名字」，所以常驻上下文的花费由我控制。
14. 作为开发者，我想用「只注入名字」的技能在真正需要时才拉取正文，所以几十个技能不会把窗口挤满。
15. 作为开发者，我想目录里只有 `name` 和 `description`，所以模型知道有哪些能力可用而不必预载全部内容。
16. 作为开发者，我想带 `disable-model-invocation` 的技能默认不进目录、不被自动注入，所以那 14 个「手动触发」技能不会静默变成常驻知识。
17. 作为开发者，我想在需要时显式把某个这类技能设为可自动注入，所以默认保守但不封死。
18. 作为技能作者，我想让 agent 能调用 `Skill` 工具按名字加载我的技能，所以我的技能正文里 `Call the Skill tool` 这类话在本系统里能照常执行。
19. 作为技能作者，我想技能目录里的 `tests.md`、`UI.md` 等兄弟文件在被加载时一并可用，所以我的技能不会是断腿的指针。
20. 作为技能作者，我想兄弟文件缺失时报错并指出缺哪个文件，所以我的技能包不会静默残缺。
21. 作为技能作者，我想兄弟文件引用只展开一级，所以我不必担心深层链式加载失控。
22. 作为开发者，我想 agent 能派一个只读子代理去读一批文件并只回摘要，所以 `research`、`code-review` 这类技能能真正跑起来。
23. 作为开发者，我想子代理**不能**执行 `run_command`、不能写文件，所以它不会成为注入攻击的加速通道。
24. 作为开发者，我想子代理的每次调用都落 run 行并带父 run 关联，所以我在会话审计里能看清谁派生了谁。
25. 作为开发者，我想子代理的 token 计入任务总量但不重复计数，所以成本账目准确。
26. 作为开发者，我想子代理不继承阶段声明的工具，所以它不能靠阶段配置扩权。
27. 作为开发者，我想子代理不能再派子代理，所以派发深度可控。
28. 作为开发者，我想 agent 对 `run_command` 的联网行为受工具层策略约束，所以直白的「把数据发出去」指令会被拦。
29. 作为开发者，我想文档明确写出「工具层策略约束不了子进程自行联网」这条残余风险，所以我不误以为它是安全边界。
30. 作为开发者，我想为某阶段配置该阶段的技能，也想为同一阶段的单个节点配置它专属的技能，所以「提问型」节点和「写文件型」节点不会拿到互相打架的指引。
31. 作为开发者，我想节点级技能只增不减地并入阶段级，所以局部配置不会意外削减已有能力。
32. 作为开发者，我想在技能正文为空或技能名不存在时启动就被拒绝并报出是哪个阶段哪个节点，所以坏配置不会延迟到运行时才暴露。
33. 作为开发者，我想伪阶段（`conflict_check` / `validator_cross_check` / `project_analysis`）不参与技能声明，所以它们的确定性姿态不被搅动。
34. 作为运维者，我想给某节点开启「pending 后续接上一轮对话」，所以信息补充型 pending 不必让 agent 从零重读一遍仓库。
35. 作为运维者，我想该开关默认关闭，所以默认行为与今天逐字相同。
36. 作为运维者，我想开启续接后 token 不被重复计费，所以成本视图可信。
37. 作为运维者，我想续接与上下文压缩协同正确（本轮提问不被当成历史锚点），所以压缩不会保错东西。
38. 作为运维者，我想续接对 `context_overflow` 这类 pending 也有明确行为，所以不存在「开了却无效」的静默路径。
39. 作为开发者，我想重试时仍然是干净对话，所以续接开关不会污染既有的重试语义。
40. 作为开发者，我想技能的启用状态在每个阶段（乃至节点）独立，所以同一技能可以只用在需要它的地方。
41. 作为开发者，我想远程安装失败时看到失败原因（摘要不符 / 来源未放行 / 格式非法），所以我能判断该改什么。
42. 作为开发者，我想本机没网时仍能安装本地技能，所以市场不是唯一入口。
43. 作为开发者，我想卸载技能后引用它的阶段配置被明确报错而不是静默降级，所以「技能名是唯一身份」这条不变量不被悄悄破坏。
44. 作为维护者，我想内嵌技能移除后相关测试改为对真实的用户目录技能断言，所以测试不再钉住已删的机制。
45. 作为维护者，我想技能相关的 prompt 变化反映在 `prompt_template_hash` 上，所以 prompt 版本对照仍然成立。
46. 作为维护者，我想这次改动对决策 170 / 154 / 47 的修订关系显式登记，所以后来者能读懂为什么语义变了。
47. 作为维护者，我想新接缝只有一个（市场客户端），所以可测试性成本不随功能数线性增长。
48. 作为使用中文项目的开发者，我想技能目录与推荐清单的展示用项目词汇表的说法，所以界面语言与文档一致。

## Implementation Decisions

### 1. 技能来源与发现（修订决策 170 / 47）

- **移除** `EMBEDDED_SKILLS` 常量与 `SkillSource::Embedded` 变体。技能来源收敛为三类：
  用户 markdown（`{home}/skills/{name}/SKILL.md`）、可覆盖的技能根（新增 `[skills] dir`，
  照 `PromptsConfig::resolved_dir` 先例，为指到 `~/.zcode/skills` 而设）、PATH 可执行文件
  （决策 47 原语义不变，仍是只见名字的工具型技能）。
- **frontmatter 从「只做文本剥离」升级为「读四个键」**：`description`、
  `disable-model-invocation`、`license`、`allowed-tools`。实现为逐行 `key: value` 的轻量解析，
  **不引入 YAML 依赖**（沿用决策 170 的姿态）；解析失败或键缺失时按缺省值处理，不 fail fast。
- **`name` 仍取目录名**，并新增校验：若 frontmatter 里写了 `name` 且与目录名不一致，启动
  fail fast（对齐 Agent Skills 规范「name 必须与父目录同名」）。
- 正文「存在且非空」的 fail fast 口径不变；`resolve` 的兜底路径仍在。

### 2. 渐进披露与二档注入

- `build_system_prompt` 的 `[技能清单]` 段落位置不变（golden 顺序仍为
  `[基线前言][工作目录][AGENTS.md][persona][技能清单][格式规则]`），但**渲染内容分三态**：
  - **目录态**（未被声明、仅在可用池）：`- {name}: {description}`，`disable-model-invocation: true`
    的技能**不进目录**（选型 D）。
  - **名字态**（已启用，模式 `name`）：只列 `- {name}`，正文由 `Skill` 工具按需拉取。
  - **全文态**（已启用，模式 `full`）：`### {name}` + 正文，与今天一致。
- 阶段/节点配置里技能字段的形态由 `string[]` 扩展为 `string | {name, mode, trusted}` 混合数组，
  裸字符串按 `{mode: "full", trusted: false}` 解释（**向后兼容今天的配置行**）。
- **只增不减**不变量保留：有效集 = 基线 ∪ 阶段级 ∪ 节点级。选型 A 之后基线为空，故实际
  等于阶段级 ∪ 节点级。

### 3. `Skill` 工具（新增第 8 个内置工具）

- 工具名就叫 **`Skill`**——这不是命名偏好而是功能性决定：上游技能的正文里写着
  `Call the Skill tool with "grilling"`，工具同名使这些正文**无需改写即可执行**（选型 B 的
  直接兑现）。
- 入参 `{name}`，行为：解析该技能正文（含兄弟文件展开）并作为 tool result 进入 `messages`。
  不进 system prompt，因此**不影响** `prompt_template_hash`。
- 落点：`BUILTIN_TOOLS` 由 7 项扩为 8 项（当前 `tool_defs` 对非 `BUILTIN_TOOLS` 的声明只
  warn 后忽略，故此常量是唯一放行闸）；`MANDATORY_TOOLS` **不加入**它，由阶段声明启用。
- 未知技能名 → 工具返回错误文本（不 fail fast，让模型自行纠正）。
- 该工具的可用性是**全局的**：未声明的技能也能通过目录被模型发现并加载（这是渐进披露的
  自动触发路径），但 `disable-model-invocation` 与未信任技能除外。

### 4. 兄弟文件展开

- `Skill` 工具返回正文前，解析**一级**相对引用（`[text](file.md)` 形态）并内联，深度不递归
  （对齐 Agent Skills 规范「Keep file references one level deep」）。
- 引用的目标必须是该技能目录**之内**的路径（拒绝 `../` 穿越与绝对路径）。
- 目标文件不存在 → `Error::Config`，报文须指出**技能名 + 缺失文件名**。
- 非 `.md` 引用（如 `scripts/*.py`）**不展开**，只保留原样文本：本系统没有脚本执行语义，
  假装支持会让技能作者误判。

### 5. 只读子代理（重开决策 154）

- 新增工具 `spawn_sub_agent`，复用 `agent_attempt` 的循环子集，**独立 context**（这是它的
  全部价值：把「读 20 个文件」的原文挡在父上下文之外，只回摘要）。
- **工具集固定为只读**：`read_file` / `list_dir`。**不含** `run_command` / `write_file` /
  `edit_file` / `delete_file` / `submit_metadata`；**不继承**阶段声明的工具（防阶段配置扩权）。
- 深度限制**一层**：子代理不再获得 `spawn_sub_agent`。
- 落 run 行：`agent_type = "subagent"`、`parent_run_id` 指向父 run（两列均已存在于
  `kanban_node_runs`，无需迁移）。
- 计量：子代理 token 记在**自己的 run 行**上并计入任务总量；父 run 不重复累加子代理的用量
  （与 §8 的续接双算防护共用同一条「不盲求和」原则）。
- 超时：沿用节点级 `node_idle_timeout_sec` / `node_max_duration_sec` 作为该子代理调用的上限；
  并发上限取 `max_concurrent_tasks` 之外的一个独立小常量。
- 审计：子代理的会话落 `kanban_node_conversations`，`agent_type = "subagent"`、带 `parent_run_id`。

### 6. 技能市场

- **本地导入**：上传 zip / 目录 → 校验结构（须含 `SKILL.md`）与 frontmatter → 落到
  `{home}/skills/{name}/`。同名冲突**默认拒绝**并报出已存在的来源，覆盖需显式确认。
- **目录扫描导入**：把一个本地技能根（如 `~/.zcode/skills`）下的技能批量导入，逐个显示
  装前预览。
- **远程 registry**：索引格式须定义（技能名、版本、摘要、来源、描述）；下载后按 `sha256`
  校验，不符即拒绝安装；**来源白名单**默认只放行配置内的源（照 Claude Code
  `strictKnownMarketplaces` / Codex `allowed_sources` 的姿态）。
- **装前预览**：安装前列出 ① 该技能会被推荐到哪些 Stage / Node；② 注入模式与信任态；
  ③ 正文中是否出现 `run_command`、网络调用、密钥路径字样。第 ③ 项是选型 E 里
  「市场准入自带可见性」的落点。
- **信任标记**：新装技能默认 `trusted: false`，**不得以 `full` 模式注入**；用户显式确认后可
  转为已信任。未信任技能可以是 `name` 模式，正文只由 `Skill` 工具按需拉取。
- **卸载与引用完整性**：卸载一个仍被阶段配置引用的技能 → 启动校验与 `PUT /stage-configs`
  的 fail fast 照旧生效（技能名是唯一身份，这条不变量不变）。

### 7. 出口控制（工具层）

- 给 `run_command` 增加**网络策略**：默认按 allowlist 放行，未放行的目标需显式批准
  （具体形态在票内定：命令级特征识别或网络层拦截）。
- **诚实标注残余风险**：工具层策略**只能约束 agent 主动经由 `run_command` 发起的调用**，
  约束不了被启动子进程后续自行联网。本项的真实价值是「拦截直白的 exfiltrate 指令」，
  **不是安全边界**。
- **不引入 OS 级沙箱**：决策 19（修订）/ 104 的「不做系统级 confinement」保持不变；
  OS 级沙箱作为**后续决策的正式候选**登记在案，不在本批实现（选型 E 的边界）。
- 该项与决策 112（provider 密钥明文存储）的关联须在文档里点明：`{home}/data/` 与技能目录
  同父，**技能根不得被放宽为 agent 可读**（因此兄弟文件走加载器展开而非放宽
  `FileToolPolicy`）——这是选型 C 与决策 104 的交界。

### 8. 会话续接（参数化）

- 新增阶段级 / 节点级参数（默认 **false**：每次 attempt 干净对话，与今天逐字相同）。开启后，
  `agent_attempt` 在 pending → resume 重入时从 `kanban_node_conversations.messages_json`
  读回上一 attempt 的 messages 作为起点。
- **三项必要条件**（缺一即为错，不是加固项）：
  1. **`context_overflow` 退出路径补写会话行**——该 pending 在会话写入之前返回，今天没有行可读。
  2. **token 双算防护**——`refresh_task_totals` 今天对所有 run 行盲求和，续接会把历史对话
     的输入 token 在新 run 里再报一遍；须给续接的 run 打标记并在汇总时排除被续接的历史。
  3. **压缩锚点修正**——`compact_messages` 的 `must_keep` 保留「第一条 user 消息」；载入历史
     后那条是**上一轮**的提问，会占掉 keep 预算。载入的 messages 不得被当作本轮锚点。
- **wire 顺序不变**：system 恒为 `messages[0]`、user 恒为 `messages[1]`（有测试钉住）。
- 与重试分层的关系：`agent_retry_max` 的干净对话重试语义（决策 33）**保持不变**；续接只作用于
  pending → resume 这一条边界。

### 9. 配置界面与推荐

- `StageConfigForm.svelte` 的 `skills_json` / `node_overrides_json` 自由文本输入，改为结构化
  控件：已启用技能列表（名字 + 注入模式 + 信任态）、按节点独立勾选、可用技能目录（带
  `description`）、每阶段的**推荐技能**清单与「一键安装」按钮。
- **推荐的投递载体是界面，不是二进制**（选型 A）：推荐清单以代码内常量形式存在（沿用决策 7
  的内嵌 persona 先例的「内嵌默认值」姿态），界面据此展示；一键安装成功后写入该阶段配置行。
- **停用** = 从配置行移除；**替换** = 装同名技能到用户目录（同名覆盖）。
- 界面语言用 `docs/glossary.md` 的词汇表说法（技能 / 节点级技能 / 阶段）。

### 10. 测试契约迁移

- 移除内嵌技能后，现存 50 处按名字钉住 `grilling` / `to-spec` 的断言（分布：
  `crates/core/tests/executor.rs` 13、`crates/core/tests/...` 与 `api_contract.rs` 3、
  `config.rs` 7、`prompts.rs` 6、`skills.rs` 21）须改为**对真实的用户目录技能**断言
  （临时 home 写入 `skills/{name}/SKILL.md`），保留原有的注入行为断言意图。
- `skill_body_changes_prompt_hash` 一类的 hash 敏感性用例改用用户技能验证。

## Testing Decisions

**接缝（决策 143）：优先复用既有五条，本 effort 只新增一条。**

| 用途 | 接缝 | 说明 |
|---|---|---|
| 技能目录隔离 | `AGENTPIPELINE_HOME`（既有） | 每测试独占临时 home，直接作为技能根；**不新造目录接缝** |
| 技能拉取与 LLM 交互 | `LlmClient` / FakeAgent（既有） | FakeAgent 只替换 LLM 响应流，工具层真跑（决策 148）——`Skill` 工具与 `spawn_sub_agent` 由此可端到端测 |
| 子代理超时 | `Clock` + 进程组终止器（既有） | 子代理只用只读工具，无需新终止器 |
| 市场/registry 网络 | **新增唯一接缝：市场客户端 trait** | 返回 `(bytes, sha256, source)`；测试用 fake 提供固定字节与摘要，**不打真网络**；摘要不符等错误路径由 fake 驱动 |
| 技能根覆盖 | `prompts_root` 式路径解析（既有先例） | `[skills] dir` 照 `PromptsConfig::resolved_dir` 做，复用其测试体例 |

**什么算好测试**：只断言外部可观察行为——启用了什么技能、注入进 system prompt 的文本长什么样、
`Skill` 工具调用后**下一轮 `LlmRequest.messages` 里出现了正文**、子代理拿到哪些工具、账目数字
对不对。不断言内部结构（不测 `BTreeMap` 顺序、不测解析中间态）。

**模块与先例**：

- `skills.rs`（frontmatter 四键解析、目录列举、二档渲染、兄弟展开与路径穿越拒绝）：沿用
  `crates/core/src/agent/skills.rs` 现有单测体例（`tmp()` + `write_skill()` 辅助）。
- `Skill` 工具与子代理：`crates/core/tests/executor.rs` 已有的「FakeAgent 脚本 → 断言
  system/user prompt 内容」体例（如 `node_scoped_skills_inject_different_bodies_per_node`）；
  子代理另需一条「工具集为只读」的断言。
- 市场：`crates/app/tests/api_contract.rs` 的端点契约体例（安装 / 卸载 / 预览 / 摘要不符 400）。
- 出口控制：`file_policy.rs` 的策略单测体例（allow / deny / 边界样本）。
- 会话续接：`tests/e2e/tests/pending.rs`（E2E-21）是现成的最近先例——它已经在测
  `info_insufficient` 后重入的 prompt 内容，续接只需再加「第二轮 `messages` 是否含上一轮」的断言。
- 前端：`frontend/src/lib/stageConfigs.test.ts` 的纯数据 + 解析体例（技能字段新形态的解析与
  回填、旧裸字符串的兼容）。

**必须补的缺口**：今天**没有任何测试钉住**「每次 attempt 对话为空」——该性质只由构造和散文
（决策 33 的注释）保证。改动前须先补一条断言锁住当前行为，否则续接开关会改掉既有语义而无
安全网。

## Out of Scope

- **OS 级沙箱**（macOS Seatbelt / Linux bubblewrap）与网络出口的系统级 confinement。决策 19
  （修订）/ 104 的「无系统级沙箱」保持；本批只做工具层策略，并把 OS 级方案登记为后续候选。
- **远程 registry 的签名与审核流程**（Sigstore 式签名、人工复核队列）。本批只做 `sha256` 摘要
  校验 + 来源白名单 + 装前预览。
- **`allowed-tools` 的权限授予语义**。该字段在 Agent Skills 规范里标记为实验性，本系统没有
  「工具权限授予」这一层（工具集由阶段配置决定），故只解析不生效。
- **技能内脚本的执行**。兄弟文件里的 `scripts/*.py` 不展开、不执行。
- **MCP 接入**（backlog §B.1）与**对话 agent**（§B.2）。
- **前端视觉**：主题六「像素机房 · 夜班流水线」已定（决策 169），本批只加控件不改视觉语言。
- **技能正文的静态安全扫描**（用正则拦恶意正文）。明确不做——既拦不住变形又会误伤合法技能，
  风险由信任标记 + 装前预览承担。

## Further Notes

- **修订关系（AGENTS.md 要求显式标注）**：本 spec **修订决策 170**（技能不再内嵌、二档注入、
  节点级字段形态扩展）、**重开决策 154**（子代理从「v1 无实现」改为只读实现）、
  **修订决策 47**（技能根可覆盖；`disable-model-invocation` 进入语义）。与决策 19（修订）/
  104 的关系是**部分承接**：不引入 OS 沙箱，但新增工具层网络策略。
- **规模与成本**：27 个技能的 `SKILL.md` 合计约 128KB（含兄弟文件约 201KB），单个最大
  `wayfinder`（11.9KB）。这是二档注入与渐进披露存在的量化理由——全量注入会让每个节点
  每轮都背上这个量级。
- **上游技能的兼容性事实**（决定「不要求适配」的实际可用面）：27 个技能中 **14 个**带
  `disable-model-invocation`（`ask-matt`、`grill-me`、`grill-with-docs`、`implement`、
  `improve-codebase-architecture`、`teach`、`to-questionnaire`、`triage`、
  `setup-matt-pocock-skills`、`wait-what`、`to-spec`、`handoff`、`wayfinder`、`to-tickets`）；
  7 个正文提到 `Skill tool`；6 个以子代理 / 后台 agent 为前提（`ask-matt`、`code-review`、
  `codebase-design`、`grilling`、`improve-codebase-architecture`、`research`）。三者有重叠，故「目录态默认不给自动注入」
  恰好挡掉了大部分「装进来却不该常驻」的技能。
- **授权**：27 个技能里只有 `frontend-design` 带许可声明（Apache-2.0），其余**无任何许可
  声明**（默认保留所有权利），而本仓是 MIT。选型 A 的「不内嵌」同时解掉了这个再分发问题——
  技能一律由用户从来源安装到本地，不经二进制分发。
- **`attempt` 语义的一个既存瑕疵**：`next_attempt` 统计 `(task, stage, node)` 的 run 行数，
  **不过滤 `agent_type`**，而伪阶段 run 行复用父节点的 stage/node——故 `attempt` 会被
  `conflict_check` 之类虚增。续接若依赖「读 attempt N-1 的会话」须先处理这个错位，否则可能
  读到伪阶段的会话。**建议在票内一并修正**（加 `agent_type = 'main'` 过滤），这属于 bug 修正
  而非本功能引入的问题。
- **伪阶段不参与技能**：`call_pseudo_stage` / `project_analysis` 两处调用点保持 `&[]`
  （决策 170 的「明确不做」延续到本批）。
- **票的拆分与依赖**：由 `to-tickets` 产出。建议顺序为「决策登记 → 技能来源与 frontmatter →
  二档注入与字段形态 → `Skill` 工具与兄弟展开 → 只读子代理 → 市场（本地 → registry → 预览与
  信任）→ 出口控制 → 会话续接 → 配置界面与推荐 → 契约迁移与收口」，其中**契约迁移**与
  **决策登记**各自独立可并行。
