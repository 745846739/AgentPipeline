# test 阶段产出不落提交，merge 的 rebase 必卡

## 根因（2026-10-09 实证，任务 `01M4CD59Y977ZQ0GMY9MPSFFMX`）

三段各自都「对」，接起来是一条死路：

1. **test.execute 把集成测试写进 worktree**（`docs/pipeline-spec.md` §test execute：
   「编写集成测试代码写入 worktree」），而 `TEST_EX_SYSTEM`（`crates/core/src/agent/templates.rs`）
   的**输出步骤只有 1–6**：读场景 → 读变更文件 → 写测试 → 跑测试 → 写报告 →
   `submit_metadata`。**没有「落提交」这一步**（对比 develop 的提交契约，决策 391 有落提交步）。
2. **merge 阶段 A 第 (2) 步是 `git rebase`**（§merge execute），要求工作区干净。
3. 于是只要 test 阶段真写了文件，merge 就报
   `git 错误：unstaged changes exist in workdir; class=Rebase (29)` → `retry_exhausted`，
   而该状态的 `allowed_actions` **只有「重试合并」与「终止任务」**——重试必然同样失败，
   **这是一个没有出口的死锁**。

**为什么历史上没炸穿**：`run_command` 在 `test.execute` 是广告出来的（决策 396 只禁了
review / test_design），所以**会不会卡住取决于那次的 agent 自己有没有顺手 commit**——
非确定。已完成的 4 个任务的 worktree 与 `kanban/*` 分支都已删除，无法取证它们走的是哪条路。

## 本次的人工解除（非解法，只是把这单放过去）

worktree 内补提交 `96b095f`（5 个文件 / +840）→ 触发 `POST /tasks/{id}/resume`
（`goto merge.execute`）→ run 405。**补提交时 pre-commit 拦下一次真缺陷**：
`copy_payloads.rs` 有未使用的 `use …::storage::Store`（`-D warnings`）——test agent 的产出
本身不干净，删掉该行才提交成功。**这说明「跳过钩子直接合」是错的出路**：merge 阶段 A 跑的
`lint_command`（`cargo clippy --all-targets -D warnings`）会以同样方式失败，只是换了个地方烧一轮。

## 三个候选形态（开工时裁决）

**A · 给 `TEST_EX_SYSTEM` 加第 7 步「落提交」**（与决策 391 的提交契约同构）
最贴既有形状：develop 有落提交步，test 同样是「测试代码是硬产出」（决策 37）。
配套要给 `test.validate_output` 或 merge 加一条**未提交产出即红**的校验，否则模型漏做一步
照样回到今天这个坑。代价：动 system prompt（prompt cache 前缀，决策 380/381）+ 模板用例。

**B · merge rebase 前把本任务的未提交产出自动提交**
一次 `git add -A && git commit`，并把清单写进 `merge-proposal.diff` 说明区。
必须与决策 61 / 132「**不自动 stash**」划清：stash 是把改动藏起来（评审与审批都看不见它），
commit 是留痕（diff、审批面板、`base_commit` 都认它）。代价：合并审批看到的 diff 里
混进「不是 agent 声明的」提交，需要申报机制（决策 397）跟上。

**C · 至少别给死锁**
merge 撞上 dirty workdir 时给**一句说得清的报文 + 对的可用动作**（而不是 `retry_exhausted`
配一颗必然重败的钮）。这条是纯止损，不解决产出没人提交。

**验收（任一形态落地时）**

- [x] 用例：test 阶段写出文件后直接进 merge，**不再**出现 `unstaged changes exist`
      （三层全落，决策 416：L2 两条守卫集成测试 + merge 自动留痕兜底）
- [x] 反向用例：dirty workdir 场景下给出的报文 / 动作是可用的（不会指向一条必然失败的路）
      （`environment_blocked`：修复后重试执行 + 终止任务，无 skip）
- [ ] 106 上真跑一单「test 阶段有产出 → merge → 审批」，全程无人 ssh 上去手动 commit
      （待部署：重启 `agent-pipeline.service` 会杀掉 01M4CD59 的 run 405，等它跑完）

## 边界

- 不改「test 代码不进评审」那条已知风险记录（`pipeline-spec.md` 已记，决策 37，
  由 merge 全量测试兜底）。
- 不动决策 61 / 132 的「目标分支工作区不自动 stash」——本票管的是**任务 worktree**，
  两处对象不同，但措辞要互相对照写，别让后人以为可以互相援引。
