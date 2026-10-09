# 01: test 阶段产出没人提交 → merge 的 rebase 必卡（无出口死锁）

**来源:** 任务 `01M4CD59Y977ZQ0GMY9MPSFFMX`（文案纪律扩面）走到 merge 时实证——
`pending_reason = {"type":"retry_exhausted","message":"git 错误：unstaged changes exist in
workdir; class=Rebase (29)"}`，`allowed_actions` 只有「重试合并」与「终止任务」，
而重试必然同样失败。三段拆开看各自都对（test 写 worktree / 没有落提交步 / merge 要 rebase），
接起来没有出口。根因、历史成因与三形态见 `../spec.md`。

**What to build:** 从三个形态里选一个（A 加落提交步 + 校验 / B merge 前自动提交留痕 /
C 纯止损的报文与动作），落地时带：

- [x] test 阶段有产出 → 直接进 merge，**不再**出现 `unstaged changes exist`
      （A 落提交步 + develop/test 双侧工作区守卫在 validate_output 就打回，穿不到 merge；
      漏网的由 B 在 rebase 前自动留痕兜底）
- [x] dirty workdir 场景给的报文与可用动作是**真可用**的（不会指向一条必然失败的路）
      （C：`environment_blocked` 一等 pending，动作 = 修复后重试执行（goto 本阶段入口）+
      终止任务，无 skip；节点错误那一个出口改判，其余分类不动）
- [x] 选 A 则补「未提交产出即红」的校验用例（否则模型漏一步又回今天这个坑）；
      选 B 则与决策 61 / 132「不自动 stash」的分界写进决策条目，并让申报机制（决策 397）
      覆盖这笔自动提交
      （三层全落：**决策 416** 记了 stash/commit 分界与「未跟踪不动」对 397 口径的对齐；
      校验用例 = L2 两条守卫集成测试）
- [ ] 106 上真跑一单「test 有产出 → merge → 审批」，全程无人 ssh 手动 commit
      （**待部署**：部署重启 `agent-pipeline.service` 会杀掉 01M4CD59 的 run 405，等它跑完）

**Blocked by:** None（可立即开工；**建议在任务 `01M4CD59Y977ZQ0GMY9MPSFFMX` merge 之后**，
避免与那单的审批面混在一起）

**Status:** ready-for-human（本地落地完成，2026-10-09：三层全落 + 用例全绿；待部署 106 + 端到端验证）

**边界.** 不改「test 代码不进评审」的已知风险记录（`pipeline-spec.md` 已记，决策 37）；
不动决策 61 / 132 的「目标分支不自动 stash」，但两处措辞要互相对照，别让后人互相援引；
不顺带改 `TEST_EX_SYSTEM` 的其余步骤。

## Comments

### 2026-10-09 · triage 裁决 → ready-for-agent，选型 A（验收②止损并入）

**A · `TEST_EX_SYSTEM` 加第 7 步「落提交」+ 配套「未提交产出即红」校验**——与决策 391 的
develop 提交契约同构（test 产出是硬产出，决策 37），根治死锁；校验即验收③，否则模型漏一步
又回今天这个坑。**验收②（dirty workdir 给真可用的报文与动作）并入同票**：`retry_exhausted`
配一颗必然重败的钮本身就是缺陷，随 A 一起修。**排 B**：要跟决策 61/132 划清界线 + 申报机制
397 跟上，改动面与风险最大，且把非 agent 声明的提交混进审批 diff。
**开工序**：三票之首（死锁已实证咬过一次，96b095f 人工解除）。**开工约束**：`templates.rs`
与并行会话在飞改动重叠——独立 worktree 或等其收口。

### 2026-10-09 · 三层全落 → ready-for-human（决策 416）

triage 选 A，grilling 后**扩为 A+B+C 三层全落**（各堵一段：A 根治产出没人提交、B 兜底
漏网的、C 止损不给必然重败的钮）——票面「三选一」就此作废，裁决全文见**决策 416**。
落地：`TEST_EX_SYSTEM` 第 6 步落提交（fmt/lint 先自查）+ `worktree_clean_check` 挂
develop / test 双侧 validate_output（确定性 Retry、事实段 `worktree-dirty-facts.md`
进重入 prompt）；merge 阶段 A rebase 前 `commit_unstaged_tracked_changes`（`[autosave]`
留痕、只收已跟踪、清单进 `merge-proposal.diff` **头部**）；`PendingKind::EnvironmentBlocked`
（标签「环境受阻」、goto 本阶段入口「修复后重试执行」+ 终止任务、无 skip、通知 Failed、
5 条指纹判据）。验收前三条已勾（见上），**第四条（106 端到端）待部署**——部署重启
`agent-pipeline.service` 会杀掉 01M4CD59 的 run 405，等它跑完再推。
用例：L1 `routes` / `actions` / `git`×2 / `merge` / `templates` 各条 + 枚举镜像
（`enum_members.json` / notify pin / resume_cause 27 行）；L2 `executor.rs` 两条守卫
集成测试；前端 `diff.test.ts` 3 条。跑数：lib 822 通过、integration 539 通过 / 4 ignored、
vitest 全绿。
