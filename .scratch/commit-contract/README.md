# commit-contract：任务分支零提交缺口

## 事故实证（2026-10-06，任务 01M47RQG4M9533F5TMF1AGJXC8）

同一根因两轮 test 不通过：develop 轮从未把变更落进任务分支，merge 阶段 A
`files_changed == 0` 短路（`merge.rs:243`，`gate_failure_kind=Test`）踢回 test 复检，
test 全绿后只能分类 code_issue 上交用户；第一轮打回 develop 后 develop 未落提交
（attempt 3 元数据校验失败，attempt 4 只重交了元数据），原样再失败一轮。

根因链（四层各缺一块，已逐层取证）：

1. **提交契约无归属**：`DEV_EX_SYSTEM`（`crates/core/src/agent/templates.rs:235`）
   输出步骤只到 write_file + submit_metadata，无任何提交要求。历史任务成功全靠模型
   自觉（ux-audit-3 审计任务的 develop agent 自己 grep 文档后自行 `git commit`）。
2. **下游四道检查全漏放**：`develop_code_gate` 只跑 lint+单测；review 读工作区
   diff；test 用例本来就绿；直到 merge 才炸——发现点离制造点隔四个阶段约两小时。
3. **失败归类错位**：空 diff 记 `Test` 走决策 85 复检，但 test 永远无法自修
   （修用例造不出提交），确定性判定（`rev-list --count == 0`）耗掉一整轮复检+一次人工决策。
4. **打回后无自愈机制**：重入 prompt 没有一句话告诉 develop「工作区有 11 处未提交、
   分支零提交、必须落提交」。

次生发现：develop attempt 3 的「未找到结构化元数据」是模型正文声称已提交元数据但未
调用工具——救援逻辑按设计工作（重入后恢复），真缺口是「每轮最终动作必须是调用
submit_metadata」契约锚点只有三个 validate_input 模板有（决策 277①），execute 模板没有。
现场坑：worktree 内相对 `cwd` 解析到 `/opt/AgentPipeline`（另一份 checkout）。

## 止血记录（正式票落地前）

- **persona 追加**：106 `stage_configs.develop.persona_append` 已写入提交契约
  （2026-10-06 11:0x UTC，服务已重启生效）。**根修部署后必须撤掉**，避免双份契约漂移。
- **解卡**：过渡 285 `user_resume` 放行「修改业务代码」，裁决文本带落提交硬性要求。

### 止血实证（2026-10-06 11:2x–13:0x UTC）：prompt 级契约被证实不够

- run 333/334 的 system_prompt 确认含提交契约全文（`prompt_template_hash`
  `cef1dcad…` → `d91a75d6…` 佐证已生效），但 develop attempt 7（run 334，117 条消息的
  续跑会话）**一条 git 命令都没跑**（`kanban_node_commands` 里该 run 命令数 = 0），
  末轮正文还复述了「本轮最终动作即该 submit_metadata 调用」——契约锚点句被逐字复述、
  却没被执行。失败形态：长会话续跑轮的注意力收窄到「重交元数据解锁」，system prompt
  里的契约管不到这一步。**结论：守卫必须是代码（develop_code_gate 硬检查），prompt
  只配当第一道软防线。**
- **scene_17 取数陷阱（新增实证）**：`changed_paths` = `git status --porcelain` ∪
  `git diff HEAD`（`ux_audit3_landing.rs:56`）——变更一旦落提交两处全空，场景 17①
  「改动面不应为空」当场红；`numstat`（scene_7/11 的行数断言）同理。即：**提交这个
  动作本身会弄红按工作区取数的测试**。管线内无任何角色能同时走出「不提交 merge 空
  diff、提交则 scene_17 红」的死结。
- **处置**（2026-10-06 13:1x UTC，操作者代行并留痕）：任务 pause → 按测试报告自开的
  处方落五段提交（`03051bb` 票 05 / `96cdb44` 票 08 / `50cef3e` 票 13 /
  `fed8ccf` 场景用例+IMPLEMENTATION / `26039af` scene_7/11/17 取数补
  `origin/main..HEAD` 回退——保住场景意图「改动面白名单 + 冻结零触碰」在提交后时序
  下依然成立）→ `cargo test --no-run` 绿 → resume 复跑 review。
  五段 subject 均含 scene_17④ 要求的票号字面量。

## 票

- [01-commit-contract-and-empty-branch](issues/01-commit-contract-and-empty-branch.md)
  ——单票收口：模板根修 + 申报制 + develop 守卫 + merge 分类改道 + 注入段 + 决策 391。

### 落地状态（2026-10-06，本地直改完成）

形状 1–6 已落地并通过 lint / 全量测试，决策 391 已续写、testing.md 已补表行。

**部署批次已执行（2026-10-07）**：推送 → CI 全绿 → 任务 `01M47RQG4M9533F5TMF1AGJXC8`
经 106 闸门转绿后到终态（`done`）→ 部署 106（全量重建，`deploy-106` run 37558331087
成功，106 落 `00bafec`）→ **同批删除 `stage_configs.develop.persona_append`（形状 7）**：
`PUT /stage-configs/develop` 置空（原值备份 `/root/stage_configs.develop.persona_append.bak-20261007.*`，
含 `.restore.sql`），`skills_json` 与 `max_duration_sec` 原样保留。**不需要重启**——
阶段配置在每次模型调用时读库（`model_invoke.rs:777` 的 `get_stage_config`），
且两处 persona 追加都带 `trim().is_empty()` 守卫（`model_request.rs:988`、
`model_invoke.rs:1323`），空串等价于没有追加。

**dogfood 已执行（2026-10-07）**：真实小任务 `01M4A35GGJ53YDJRZ0R3GZTM06`（补术语表三条
词条，仅改 `docs/glossary.md`）。**两条验收目标都达成**：

- **提交契约生效 ✓**：develop execute 把改动落成提交 `c488d85`（message 合本仓惯例），
  分支 `kanban/01M4A35GGJ53YDJRZ0R3GZTM06` 相对基准 `rev-list --count` = 1、工作区干净。
- **零提交守卫不误伤 ✓**：任务目录里没有 `zero-commit-facts.md`，守卫一次都没响。

**但这轮 dogfood 顺带挖出并根治了一个更严重的隐患**：develop 代码闸门连红 4 轮
（`cargo clippy` 过，`cargo test` 红在 `-p e2e --test integration` 的 3 条、72 passed /
3 failed），红的是 ux-audit-3「落地验收」场景 scene_07/11/17——它们按 `origin/main...HEAD`
判「审计落地那次的分支 diff 形状」，在任何**别的**任务分支上必红。也就是：**develop 与
merge 的闸门对普通任务走不通**，任何任务都过不了。任务因此被人工取消。

根因与处置记为闸门套件的**分支无关性**不变量：判据必须持久、不能锚在瞬时状态
（`docs/testing.md` §8）；一次性落地验收摘成 `#[ignore]` 用例，读数留档于
`.scratch/ux-audit-3/IMPLEMENTATION.md`；并由 `scripts/gate-suite-branch-independence.sh`
（挂 `make check-lint`）机器强制。

共识裁决记录（2026-10-06 grill 会话）：解卡走 develop（不手工代提交，保场景 17 判据）；
契约归模板（message 语义属目标项目域，系统不接管）；merge 空分支直接打回 develop
（扩展决策 139「确定性失败不绕 test」原则，修订决策 85 适用范围）；零变更走显式申报制、
develop.validate_output 处提前收尾（cancelled 终态）；metadata 救援逻辑不动，只补契约锚点句。
