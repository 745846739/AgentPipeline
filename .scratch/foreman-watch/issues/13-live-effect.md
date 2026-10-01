# 13: 修复「当场生效」——清存量 → 落补丁 → 闸门 → 托管自动 resume

**What to build:** 决策 358 授权的那一路：目标项目的修复不再只走「等合入」——把修复补丁
落进**任务 worktree**，闸门过了单独成 `[repair]` commit，然后经**托管自动 resume** 让任务
带着修复继续跑。票 11 已交付「等合入」路需要的全部件（worktree → 闸门 → commit → diff，
用例钉住）；本票只做授权落地后缺的那一段。

**Blocked by:** None（两个授权裁决已随决策 358 落定，2026-10-01）

**Status:** ready-for-agent

## 设计要点（裁决已定，不再开放）

1. **先清后落**：落补丁**之前**对任务 worktree 执行 `Git::reset_hard_clean(worktree, 任务分支 HEAD)`
   （原语在 `crates/core/src/git.rs:874`，`reset --hard` + `clean -fdx`）。
   - 与决策 125 的差别：那边的 `base_ref` 是阶段 base、语义是「重试 = 从头走」；这边 base 取
     **任务分支当前 HEAD**——只清**未提交**的半成品（上一轮失败尝试的残留），已提交进度不动。
   - 清的动作照 `crates/app/src/routes/tasks.rs:400` 重试重置的先例落 system 命令台账
     （`kanban_node_commands`，记清前的脏态读数一句话，供审计看出「清掉了什么」）。
   - `add_all(["*"])` 卷存量的担忧随先清消失（票 11 收口点 2 的关闭方式）。
2. **修复链复用票 11/12 的已交付件**：闸门命令映射同 `test_command_for`（11.1：不过不落补丁
   不 resume、失败原因播报）；commit 单独成 `[repair]` 带 id 与诊断一句话；diff 口径
   `{base}..{branch}` 不变。
3. **托管第三成员**：修复后的自动 resume 进托管可自动集——与决策 210② `resume(continue)`、
   210⑧ `unstick` 并列的第三个成员。**止损不新设**，全吃 210⑨ 既有线：
   - `STEWARDSHIP_MAX_AUTO_RESUMES = 2`（`crates/core/src/types.rs:1092`）；
   - 态势指纹：同一指纹不重复动手（`types.rs:1083`）；
   - 计数落库（`Stewardship.auto_resumes`，`types.rs:1078`——重启不清零）。
   - 执行走既有注入点（`crates/core/src/pipeline/foreman/runner.rs:404` 托管动作执行者），
     resume 状态机共用 `resume.rs::apply_resume` 唯一事实源（不许抄第二份）。
   - **托管没开 / 次数满 / 指纹同** → 不自动 resume，回落「等合入」（合进默认分支 + 人按），
     并在班次播报回落原因——回落是显式行为，不是静默不动作。
4. **本仓不变**：本仓的修复仍走「等合入」（不许热修、不许自己重启的既有纪律，票 11 用例
   钉着），本票全部新增行为只对目标项目生效。

## 验收

- [ ] 清后再落：带未提交半成品的任务 worktree，修复落地后 `[repair]` commit 不含半成品
      （半成品被清，diff 只含修复）——反向：去掉先清那步，用例红
- [ ] 已提交进度保留：分支上有 commit 的任务，清后那些 commit 原样在
- [ ] 清的动作落 system 命令台账（含清前脏态读数）
- [ ] 托管开且次数未满 → 修复收口后任务自动 resume，`auto_resumes` +1、指纹更新
- [ ] 次数满（=2）→ 不自动、回落「等合入」+ 播报原因
- [ ] 同一态势指纹 → 不重复动手
- [ ] 托管未开 → 不自动、回落「等合入」
- [ ] 闸门不过 → 不落补丁、不 resume、播报失败原因（票 11 既有用例不红）
- [ ] 本仓修复路径行为不变（票 11 的「不含重启调用、不改工作区」反向断言照旧绿）
