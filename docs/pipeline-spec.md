# 流程规范

> 拆分自 agent-pipeline.md（原 §6–§9）。章节编号与决策编号保持拆分前不变，导读地图见 [README.md](README.md)。

## 6. 详细流程

### init（无 validate_input）

| 节点 | 内容 |
|---|---|
| **validate_input** | 跳过 |
| **execute** | (1) **创建 git worktree 隔离工作区**（`git worktree add <worktree_path> -b <branch_name> <base>`，`<base>` 为创建时 `project.default_branch` 的 HEAD；有 remote 时先 `git fetch` 并以 `origin/{default_branch}` 为基准，见决策 41）；(2) 更新 Task 记录 `status → running`，`current_stage → init`。Task 记录与 main 游标由 `POST /tasks` 在 API 层创建（决策 42 / 90），init 不重复创建 |
| **validate_output** | 跳过 |
| **next** | `[architect-design]` |

> **准入与 worktree（决策 98）：** `POST /tasks` 一律以 `queued`（有依赖时先 `waiting`）落库，**不直接启动**；由 `KanbanScheduler.start_task` 按 `max_concurrent_tasks` 放行后才进入 `running` 并执行 init.execute。这样并发上限是唯一闸门，worktree 也不会在名额之外被创建。

**Worktree 约定：**

| 项 | 值 |
|---|---|
| 分支名 | `kanban/{task_id}` |
| 工作目录 | `~/.agentpipeline/worktrees/{task_id}/` |
| 基础分支 | 创建时 `project.default_branch` 的 HEAD（有 remote 时为 `origin/{default_branch}`） |
| 任务目录（设计文档等） | `~/.agentpipeline/tasks/{task_id}/`（不在 worktree 内） |

设计阶段的产出（`design.md`、`dev-plan.md`、`test-scenarios.md`）写入任务目录；代码变更（业务代码、测试代码）写入 worktree。两者分离，避免设计文档污染 git 历史。

**边界情况：** 项目 `local_path` 不是 git 仓库 → 项目创建时即拒绝；仓库为 unborn HEAD（零提交）→ init 明确报错，不抛出底层 git 错误；目标仓库有未提交改动 → 不阻塞，仅在任务详情记录警告（决策 61）。

### architect-design

| 节点 | 内容 |
|---|---|
| **validate_input** | architect agent 分析任务信息是否充足。充足 → execute；不充足 → pending(info_insufficient)，用户补充后恢复到 validate_input |
| **execute** | architect agent 读取任务描述，生成 `design.md` 写入任务目录（含「验收标准」编号清单节，决策 136），并提交 `affected_files`、`new_symbols` 与 `acceptance_criteria`（决策 136）。**两层冲突检测**（决策 53 / 60）：**第一层**对比本任务与其他活跃任务的 `affected_files` 路径集合交集（任一交集即命中，无阈值可调——`conflict_overlap_threshold` 已由决策 256 删除）以及 `new_symbols[].name` 交集，任一命中 → `pending(conflict_wait)`，**`context.conflict_task_ids` 记录全部冲突任务 id**（决策 102），等待冲突任务终态后自动恢复（worktree 已隔离，此处是**串行化策略**而非防写覆盖）。**第二层**（`semantic_conflict_check = true` 时）在"模块路径重叠但符号名无交集"时，同步调用 `conflict_check` 伪阶段比对两边设计意图与新增符号，`duplicate_risk = high` → `pending(user_decision)`（`context.kind = duplicate_risk`），UI 并排展示两份设计供用户裁决。跨模块语义重复（各自模块内实现同类功能）为已知残余风险，不检测。**判定细节（决策 71）**：①"活跃任务"= `status ∈ {running, pending, waiting, queued}`（queued/waiting 尚无 architect 产出，参与比对自然为空，决策 120），已 `done`/`failed`/`cancelled` 或已归档的任务不参与比对；②符号判重用 `(module_path, name)` 组合，纯 `name` 重合（如两个模块各加一个 `new()`）只产生 warning，不触发 `conflict_wait`；③**环消除**：为避免两个任务互相等待造成死锁，只有 `created_at` 较晚者进入 `conflict_wait`（同一秒时以 `id` 字典序大者让步），较早者的任务不因对方回退 |
| **validate_output** | architect agent 读取 `design.md` 验证是否符合输入要求。通过 → next_stage；不通过 → 重新进入 execute（prompt 追加反馈）。validate_attempts +1，超过 → pending(retry_exhausted)。`cross_family_judge = true` 时首判不合格先经异族复判（决策 134 / 135） |
| **next** | `[develop-design, test-design]`（并行）→ **游标分裂点（决策 80 / 90）**：main 游标**就地改写**为 `branch = "develop-design"`（`stage` 置为 develop-design、`node` 置为入口节点，`validate_attempts` 归零），并**插入**第二条游标 `branch = "test-design"`。`UNIQUE(task_id, branch)` 保证分裂幂等，重复进入不会产生重复游标。分裂后**并行区间不存在 main 游标**，因此该区间内任何回退动作都必须显式携带 `cursor_id`（决策 91） |

### develop-design

| 节点 | 内容 |
|---|---|
| **validate_input** | develop agent 读取 `design.md`，分析是否支撑开发。充足 → execute；不充足 → pending(user_decision)，用户选择重回 architect-design 或跳过 |
| **execute** | develop agent 读取 `design.md`，生成 `dev-plan.md` 写入任务目录 |
| **validate_output** | develop agent 读取 `dev-plan.md` 验证是否符合要求。通过 → next_stage；不通过 → 重新进入 execute。validate_attempts +1，超过 → pending(retry_exhausted)。`cross_family_judge = true` 时首判不合格先经异族复判（决策 134 / 135） |
| **next** | `[sync-check]`。因 sync-check 是 join，`advance_cursor` 把本游标置为 **`waiting_join`**（决策 107）——这是 `next` 路由在"下一阶段是 join 节点"时的变体，不是独立状态机（落地实现见 `crates/core/src/pipeline/advance.rs`，决策 245） |

### test-design（业务测试用例设计）

| 节点 | 内容 |
|---|---|
| **validate_input** | test agent 读取 `design.md`，分析是否支撑测试场景设计。充足 → execute；不充足 → pending(user_decision)，用户选择重回 architect-design 或跳过 |
| **execute** | test agent 读取 `design.md`，生成 `test-scenarios.md` 写入任务目录。文档描述业务测试场景（测什么、怎么测、预期结果），**不产出测试代码**；`test_scenarios[]` 元数据每项携带 `design_refs`（引用 design.md 验收标准编号，决策 136） |
| **validate_output** | test agent 读取 `test-scenarios.md` 验证场景完整性。通过 → next_stage；不通过 → 重新进入 execute。validate_attempts +1，超过 → pending(retry_exhausted)。`cross_family_judge = true` 时首判不合格先经异族复判（决策 134 / 135） |
| **next** | `[sync-check]`。同 develop-design，`advance_cursor` 把本游标置为 **`waiting_join`**（决策 107）（落地实现见 `crates/core/src/pipeline/advance.rs`，决策 245） |

### sync-check（纯代码逻辑，**不占游标行**）

| 节点 | 内容 |
|---|---|
| **validate_input / validate_output** | 跳过 |
| **execute** | **join 节点**：在两条游标都到达边界（`waiting_join`）且均无 pending 时执行**一次**（决策 83 / 107）。它**不占游标行**——是游标无关的屏障，由 `advance_join` 在"没有可继续的游标"时调用。读取 `dev_doc` 和 `test_design` 的 readiness 和 blockers；某分支带 `skipped_to_join = true` 时视其 readiness=true（决策 93）。**并做验收标准引用完整性校验（决策 136）**：high 优先级场景 `design_refs` 为空或引用不存在的 criterion id → 视为该分支 blocker；medium/low → 仅 warning（记 sync-check 自身 run 行的 `metadata_json`）。两者都通过 → proceed；任一方有 blocker → backtrack |
| **next** | `[develop]`（proceed）→ **游标合并点（决策 90 / 113）**：**一个事务内**把两条分支游标置 `archived`、插入单条 `main` 游标指向 `develop.execute`，随后落 sync-check 自身的 system run 行（`cursor_id` = 该 main，决策 107 / 113；幂等由 partial `UNIQUE(task_id, branch)` 保证）；`[architect-design]`（backtrack）→ **同一事务内**归档两条分支游标、插入单条 `main` 游标指向 `architect-design.validate_input`，并把 `dev-plan.md` / `test-scenarios.md` 标记为过期（决策 83）；双方 blockers 写入任务目录 `backtrack-feedback.md`（决策 126） |

### develop

| 节点 | 内容 |
|---|---|
| **validate_input** | 跳过（sync-check 已确保输入充分） |
| **execute** | develop agent 在 **worktree 内**读取 `dev-plan.md`，编写业务代码和单元测试。文件写入"先清后写"保证幂等 |
| **validate_output** | **纯代码**（决策 62 / 139）：系统先执行 `lint_command`（如已配置，决策 139），再按 `test_framework` 执行单元测试命令（记录到 `kanban_node_commands`，超时用 `test_command_timeout_sec`）。**两者 exit code 均为 0 → next_stage**；任一非 0 → 重新进入 execute（prompt 追加失败输出，修复代码或用例）。validate_attempts +1，超过 → pending(retry_exhausted)，动作集含"带失败摘要回架构设计"（决策 138）。带空格的 `test_framework` 值按**原始命令**执行（决策 419：本仓注册 `make check-test`，闸门因此同时跑 cargo 全量与前端单测） |
| **next** | `[review]` |

### review

| 节点 | 内容 |
|---|---|
| **validate_input** | 跳过 |
| **execute** | review agent 读取变更文件和单元测试，以及系统注入的 `design.md` / `test-scenarios.md` **路径**（agent 经 read_file 自行读取；文件不存在时按决策 115 降级为"本任务跳过该设计阶段"，决策 133），生成 `review-report.md`（含「设计符合性」「测试质量」两节，决策 133）写入任务目录。`review_mode = "human"` 时仅做预审，完成后直接进入 validate_output 分支处理（见 §12.5） |
| **validate_output** | 代码逻辑判断 `approved`（读 execute 提交的元数据）。`review_mode = "agent"`：true → next_stage（进入 test）；false → pending(user_decision)，用户选择 goto develop.execute（修复代码后重新 review）或 skip（强制通过进入 test）。`review_mode = "human"`：无论预审结论如何，均 → pending(human_review) 等待用户提交评审结果 |
| **next** | `[test]` |

**review 打回循环：** `review → develop（修复）→ review → ...`，直到通过或用户强制放行。每次循环 develop.execute 的 prompt 追加 review 的必须修改项。

**已知风险记录：** review 发生在 test 之前，无法评审 test 阶段生成的集成测试代码。`review-report.md` 需记录"集成测试未做语义评审"这一已知风险（决策 37），由 `merge.execute` 的全量测试重跑作为兜底闸门。

### test（写集成测试 + 执行）

| 节点 | 内容 |
|---|---|
| **validate_input** | 跳过 |
| **execute** | test agent 读取 `test-scenarios.md`（测试场景）+ 变更代码文件，**编写集成测试代码**写入 worktree（路径按 `test_framework` 惯例，见决策 52），然后**执行集成测试**（根据 `KanbanProject.test_framework` 动态构建测试命令，模板变量 `{test_command}`）。生成 `test-report.md` 写入任务目录，并在元数据中为每个失败用例标注 `failure_cause`（`test_issue` = 用例问题 / `code_issue` = 业务代码问题）。测试代码是本阶段硬产出，**必须全部通过**（决策 37） |
| **validate_output** | **纯代码**（决策 62）：读 `test_result.passed` 与 `failures[].failure_cause` 路由。true → next_stage；false → 全部为 `test_issue` → 重新进入 execute（修复用例）；存在 `code_issue` → pending(user_decision)，用户选择 goto execute（修复用例）或 goto develop.execute（改业务代码）。validate_retry_max 耗尽 → pending(retry_exhausted)，动作集含"带失败摘要回架构设计"（决策 138） |
| **next** | `[merge]` |

> **被 merge 闸门打回后的复检（决策 85 / 109）：** test 游标从 merge 闸门失败重入时（`test_result.gate_recheck = true`），execute 的 prompt 追加闸门失败输出（`kanban_node_commands` 的完整日志 + 失败用例），让 agent 重新给出 `failure_cause`。注意两个计数分属两侧：**闸门失败次数** `gate_failures` 记在 merge 的 metadata 上且不重置（决策 108）；test 游标自己的 `validate_attempts` 仍按原语义——跨阶段跳转时归零，只统计"本次进入 test 后 validate_output 打回 execute 的次数"。

### merge

| 节点 | 内容 |
|---|---|
| **validate_input** | 跳过 |
| **execute** | **入口判定（决策 72 / 95）：** 读 `merge_result.approval` —— `none` / `returned` 走阶段 A；`approved` 走阶段 B；`pending` 说明仍在等审批，**不得重入、不得推进**（路由层显式不推进，见 §11.2 `route_merge`）。**阶段 A（生成 proposal，幂等可重入）**：(1) 有 remote 时 `git fetch`；基准分支 `{base_ref}` = 有 remote 时 `origin/{default_branch}`，无 remote 时 `{default_branch}`（决策 41）；(2) 在 worktree 内 rebase 到 `{base_ref}`，并把当时 `{base_ref}` 的 commit SHA 记入 `merge_result.base_commit`（决策 96）；(3) **有冲突 → 尝试自动合并**：可自动解决 → 把自动解决的冲突记录追加到 `merge-proposal.diff` 的说明区（沿用 §4.2 的目录约定，不引入额外文件）继续；**无法自动解决 → 打回 develop 阶段**（先由系统执行 `git rebase --abort` 恢复干净状态，再把冲突文件列表与冲突内容作为反馈传入，决策 74）；(4) 无冲突或自动合并成功 → 运行 **lint（如已配置）+ 单元测试 + 集成测试**（合入前强制闸门，超时用 `test_command_timeout_sec`）：**通过 → 写 `gate = "pass"`，生成 diff（`{base_ref}..kanban/{task_id}`），写 `merge_result.approval = pending`，pending(merge_approval)**；**失败 → 写 `gate = "fail"` 并按失败类型分流（决策 139）：测试失败 → 回到 `test.execute` 重新分析（决策 85，见下）；lint 失败 → 直接打回 `develop.execute`（prompt 追加 lint 输出，`validate_attempts` 归零，确定性错误不绕道 test.execute）**。注意闸门结果用独立的 `gate` 字段承载，**不与 `approval` 混用**（决策 95），失败类型记 `gate_failure_kind`（决策 139） |
| | **阶段 B（`approval = approved` 后执行合入，决策 59 / 72 / 73 / 96 / 97）**：resume 重新进入 `merge.execute`，检测到已批准 → (0) **基准校验（决策 96）**：重新 `git fetch` 并比对 `{base_ref}` 的当前 commit 与 `merge_result.base_commit`；**不一致说明 diff 已过期 → `approval` 重置为 `none`，回到阶段 A 重新 rebase + 重跑闸门 + 重新审批**（`gate_failures` 保留不重置，决策 108）；(1) 检查目标分支工作区是否干净：被某 worktree 检出且不干净 → `pending(user_decision)`，动作"我已手动处理，继续合入 / 取消任务"（决策 132），**不自动 stash**（`allow_dirty_worktree_merge=false`）；(2) 执行合入（2026-09-12 起为 git2 **内存合入**，决策 12 / 73）：**fast-forward 优先（主干未前移时直接把引用移到任务分支 tip），不可 ff 则 `merge_commits` 三方合并 → 写树 → 双亲 merge commit，绝不 force**；**合入结果必须显式写回分支**——双亲 commit 以 `refs/heads/{default_branch}` 为目标创建，等价于原 CLI 方案的 `git update-ref` 写回（ff 与非 ff 都适用，决策 97）；(3) 写 `merge_result.status = merged`；(4) 推送 SSE（有 remote 时可选 push）；(5) 进入 done 阶段清理任务 worktree 与分支 |
| **validate_output** | 跳过（流转判断在 execute 内完成） |
| **next** | `[done]`（批准并合入）/ `[develop]`（冲突打回、返回修改、闸门 lint 失败——决策 139）/ `[test]`（测试闸门失败，决策 85） |

**测试闸门失败后的处置（决策 85）：**

闸门由系统执行，但**根因判断需要 agent**——直接打回 develop 会让"其实是用例自己写错了"的情况错误地改动业务代码。因此失败时不做人工 pending，而是把失败输出交给 `test.execute` 重新分析，复用决策 62 已有的 `failure_cause` 分类机制：

```
merge.execute 阶段 A: 单元 + 集成测试闸门失败
  │
  ├─ gate = "fail"；gate_failures += 1（记在 merge metadata 的 kanban_stage_outputs 行，决策 108）
  ├─ gate_failures >= validate_retry_max → pending(retry_exhausted)
  │    用户选择：重试 merge.execute / 终止任务（**无 skip**，决策 86）
  │
  └─ 未超限 → 跳回 test 阶段重新分析：
       current_stage = test, current_node = execute
       test_result.gate_recheck = true（prompt 追加闸门失败上下文，决策 109）
       test.execute 的 agent 读取闸门失败输出（kanban_node_commands 里的完整日志 + 失败用例），
       为每个失败用例标注 failure_cause：
         - 全部 test_issue → 自己修用例（test.execute 幂等覆盖写入）→ test.validate_output → merge 重跑闸门
         - 存在 code_issue  → test.validate_output 转 pending(user_decision)
                             用户选择：goto test.execute（修用例）或 goto develop.execute（改业务代码）
```

> **为什么是回到 test 而不是 develop：** 闸门在 merge 时失败，最常见的原因有两种——rebase 引入了新的上游代码（`code_issue`），或集成测试本身对上游改动敏感（`test_issue`）。两者只有 agent 读日志才能区分。让 test.execute 做这次分析，既符合"先分析用例问题，需要改就自己改，改代码才回开发"的顺序，也不必给纯代码的 merge 阶段新增 agent 调用和 prompt。

> **闸门失败分流（决策 139）：** 上图为**测试失败**路径。lint 失败是确定性错误（工具直接给出文件与行号），无需 agent 根因分析——直接打回 `develop.execute`（prompt 追加 lint 输出，`validate_attempts` 归零），不绕道 test.execute。`gate_failures` 对两类失败统一累加（决策 108），失败类型记 `merge_result.gate_failure_kind`。

**冲突打回策略：**

```
merge.execute 阶段 A: rebase 冲突
  │
  ├─ 尝试自动合并（无冲突标记、纯新增文件、可三方合并）
  │   ├─ 成功 → 继续跑测试 → 生成 diff → pending(merge_approval)
  │   └─ 失败 → 进入下一步
  │
  └─ 打回 develop：
       (0) 系统先执行 git rebase --abort，把 worktree 恢复到干净状态（决策 74）
       current_stage = develop
       current_node = execute
       prompt 追加："rebase {base_ref} 时发生冲突，冲突文件：{conflict_files}，
                    请基于最新 {base_ref} 修改代码解决冲突，然后重新提交。"
       validate_attempts 重置为 0
```

**审批动作端点（决策 119）：** merge_approval 的两个动作都要先写 `merge_result.approval` 再推进，不是纯 resume，均为 side_effect，配对端点 `POST /tasks/{id}/merge/decision {decision: "approve" | "return"}`：单事务内写 `approval`（approved / returned）+ 清除 `merge_approval` pending + 置游标——`approve` → 重入 `merge.execute` 走阶段 B；`return` → `develop.execute`，`validate_attempts` 重置为 0。worktree 保留。注意串行阶段只有一条 main 游标，所以此处直接改写该行即可（决策 90）。

**打回与 worktree：** 打回后 develop 在同一个 worktree 内工作，不需要重建。worktree 的 rebase 中断状态已由 merge.execute 打回前用 `git rebase --abort` 清除（决策 74），develop agent 直接在干净工作区上基于最新 `{base_ref}` 修改代码即可。

### done（无 validate_output）

| 节点 | 内容 |
|---|---|
| **validate_input** | 跳过 |
| **execute** | 根据 `merge_result.status` 设置终态：merged → status=done。清理 worktree 与分支（决策 3） |
| **validate_output** | 跳过 |
| **next** | 无（终态） |

**终态处理：**
- **成功（done）：** 任务归档，清理 worktree，保留任务目录产出文件供查看。
- **失败（failed）：** 任务保留在看板，用户可点击"重试" → 重置到 init（复用 worktree，重置到分支起点——决策 125）并置回 `queued` 重新走准入（决策 117），或点击"归档"移出看板（清理 worktree）。
- **取消（cancelled）：** 用户主动取消，清理 worktree 和分支，保留任务目录产出文件供审计。

---

## 7. 并发分支同步机制

develop-design 和 test-design 并行执行，汇聚于 **sync-check** 节点统一决策：

```
┌──────────────────┐          ┌──────────────────┐
│  develop-design   │          │   test-design     │
│                  │          │                  │
│  读 design.md    │          │  读 design.md    │
│  产出 dev-plan.md│          │  产出 test-      │
│                  │          │  scenarios.md    │
│  validate_input  │          │  validate_input  │
│       ↓          │          │       ↓          │
│  execute         │          │  execute         │
│       ↓          │          │       ↓          │
│  validate_output │          │  validate_output │
└────────┬─────────┘          └────────┬─────────┘
         │                             │
         └──────────┬──────────────────┘
                    ▼
           ┌────────────────┐
           │   sync-check    │
           │  （纯代码逻辑） │
           └────────┬───────┘
                    │
          ┌─────────┼─────────┐
          ▼         ▼
       proceed   backtrack
       进入develop 回退arch
```

**同步规则：**

1. **无交叉依赖：** 两者都只读 `design.md`，develop-design 不依赖 test-design 的产出，反之亦然。
2. **汇聚判断：** sync-check 读取 `dev_doc.readiness` + `test_design.readiness`，根据 blockers 做统一决策；并对 `test_design` 的场景清单做**验收标准引用完整性校验**（决策 136：high 场景 `design_refs` 缺失/悬空 → blocker；medium/low → warning）。
3. **决策矩阵：**
   - 双方都通过（且引用校验无 blocker）→ proceed
   - 任一方有 blocker（含引用完整性校验失败）→ backtrack，双方 blocker 写入任务目录 `backtrack-feedback.md` 传递（决策 126）
4. **死循环消除：** 决策集中在一个节点，不存在互相等待。
5. **自动恢复：** backtrack 后 architect-design 完成，自动重新进入并行流程。

**并行执行规则（决策 80–83）：**

| 规则 | 说明 |
|---|---|
| 游标独立 | 每个分支一条游标记录，`stage` / `node` / `validate_attempts` / `pending_reason` 全部独立（决策 82） |
| 相互不阻塞 | 一个分支进 pending 时，另一分支继续跑完自己的阶段，然后停在 join 边界（`waiting_join`），**不进入 sync-check 之后的阶段**（决策 82）。executor 通过"把 pending 游标移出可运行集合"实现，不是整体暂停（决策 89） |
| join 条件 | 两条游标都到达边界（`waiting_join`）且均无 pending，`sync-check.execute` 才执行，且**只执行一次**（决策 83，落实 G5）。`waiting_join` 由 `advance_cursor` 在 `next` 指向 join 时写入（决策 107）（落地实现见 `crates/core/src/pipeline/advance.rs`，决策 245） |
| pending 归属 | pending 挂在游标上；任务整体 `status = pending` 是"任一游标被阻塞"的投影。看板同时展示两个分支各自的状态与可用动作（决策 82） |
| backtrack 处置 | 两条游标**一起**重置回 `architect-design.validate_input`（一个事务内归档两条 + 插入单条 main，决策 90 / 113）；`dev-plan.md` / `test-scenarios.md` 标记为过期（文件保留供回溯，下次执行按 §8 覆盖写入）（决策 83） |
| 重试计数 | `validate_retry_max` 按游标各自判定；backtrack 属跨阶段跳转，两条游标都重置为 0（决策 43 / 82） |
| 本分支 skip | 用户对某分支执行 `skip` 时，该分支**不得越过 join**：`advance_cursor` 把它置为 `waiting_join` 并写 `skipped_to_join = true`（落地实现见 `crates/core/src/pipeline/advance.rs`，决策 245）；sync-check 读到该标志则视其 readiness=true（决策 93） |

> **模型一致性：** 本节规则取代了早先"单游标 + 待定稿"的表述。§4.1 的 `NodeCursor` 类型、§11.5 的 `kanban_node_cursors` 表、§11.2 的 executor 伪码是同一套模型的三个视图。

---

## 8. 幂等性保证

| 阶段 | 幂等策略 |
|---|---|
| init | worktree 创建前检查是否已存在（`git worktree list`），存在则复用 |
| architect-design | 覆盖写入 `design.md`，路径确定 |
| develop-design | 覆盖写入 `dev-plan.md`（各自游标，互不覆盖） |
| test-design | 覆盖写入 `test-scenarios.md`（各自游标，互不覆盖） |
| sync-check | 纯读取判断，天然幂等；只在所有前置游标就位后执行一次（决策 83） |
| develop | worktree 内文件先清后写，分支名确定性（`kanban/{task_id}`） |
| review | 覆盖写入 `review-report.md` |
| test | 覆盖写入 worktree 内测试文件（路径由 `test_framework` 决定）和任务目录 `test-report.md`；被 merge 闸门打回后重入时同样覆盖写入（决策 85） |
| merge | 入口按 `merge_result.approval` 判定阶段 A/B（决策 72）；rebase 中断后 abort 重置（打回前由系统执行，决策 74）；diff 生成前检查是否已存在；合入前检查目标分支工作区是否干净（决策 61）；git2 内存合入、双亲 commit 直接写回 `default_branch`（决策 97，2026-09-12 git2 重写）；`gate_failures` 存 `kanban_stage_outputs.metadata_json`，merge 的 upsert 路径**显式跳过该字段**（决策 108） |
| done | 状态 upsert，worktree 清理前检查存在性 |
| 打回反馈文件 | `backtrack-feedback.md`（决策 126）与 `retry-feedback.md`（决策 138）均覆盖写入、路径确定 |
| 游标 | `kanban_node_cursors` 按 partial `UNIQUE(task_id, branch)` upsert（仅约束非 archived 行，决策 113）；创建（`POST /tasks`）/ 分裂 / 合并 / 重置均幂等（决策 80 / 90）。分裂是"改写 main 行 + 插入第二行"，合并与 backtrack 是"单事务归档两条 + 插入单条 main"——游标行永不物理删除 |

---

## 9. 异常处理汇总

| 场景 | 处理方式 |
|---|---|
| agent 节点空闲超时（>`node_idle_timeout_sec` 无活动） | 杀死整个进程组，节点失败，按 `agent_retry_max` 干净对话重试；超过 → pending(timeout) |
| agent 节点绝对超时（>`node_max_duration_sec`） | 同上，防不收敛循环 |
| agent 崩溃或返回异常 | 节点失败，触发重试；超过 → pending(retry_exhausted) |
| validate_output 判定产出不合格 | 重新进入 execute，prompt 追加反馈；超过 → pending(retry_exhausted)。`cross_family_judge = true` 时 agent 型 validate_output 首判不合格先经异族复判（决策 134 / 135） |
| 首判不合格但复判合格（judge 分歧） | pending(user_decision, `context.kind=judge_disagreement`)，用户终审：continue 放行（特判直接 next_stage，不重跑校验）或 goto execute 打回（决策 135） |
| 进程中断/崩溃 | executor 从 checkpoint 恢复到中断节点入口，节点内重跑（幂等保证） |
| 冲突任务阻塞 / `(module_path, name)` 符号重复 | pending(conflict_wait)，冲突任务**全部**终态且复检无交集后自动恢复（决策 71 / 102；纯 name 重合仅 warning） |
| 语义重复风险（模块重叠） | pending(user_decision, `context.kind=duplicate_risk`)，用户裁决 |
| merge 无法自动解决冲突 | 系统先 `git rebase --abort` 恢复 worktree，再打回 develop.execute，传入冲突文件列表 |
| merge 测试闸门失败 | 跳回 `test.execute` 让 agent 重新分析根因（决策 85）：全部 `test_issue` → 修用例后重跑闸门；存在 `code_issue` → pending 由用户决定改用例还是回开发 |
| merge 闸门 lint 失败 | 直接打回 develop.execute（prompt 追加 lint 输出，`validate_attempts` 归零），确定性错误不走 test.execute 根因分析（决策 139） |
| develop / test 的 retry_exhausted | 动作集含"带失败摘要回架构设计"（goto architect-design；重试历史摘要写 `retry-feedback.md` 注入 architect prompt，决策 138） |
| 同一任务两个并行游标 | 各自独立推进；一个进 pending 不打断另一个，另一个跑完本阶段后停在 join 边界（决策 82） |
| 并行阶段某一分支超时 | 超时只作用于该分支的节点 run；该游标按 `agent_retry_max` 重试，耗尽后该游标 pending(timeout)，另一分支不受影响（决策 82） |
| merge 时目标分支工作区不干净 | pending(user_decision)，用户手动处理后继续（决策 61） |
| merge 阶段 B 发现基准分支已前移 | 比对的 `base_commit` 不一致 → `approval` 重置为 `none`，回阶段 A 重新 rebase + 重跑闸门 + 重新审批；`gate_failures` 保留（决策 96 / 108） |
| 上下文超限 | 四级压缩（§12.13）；兜底仍超限 → pending(context_overflow) |
| 并行分支间冲突 | 各游标独立推进，互不阻塞；join 由 sync-check 统一判定（决策 82/83） |
| 依赖任务失败 | pending(dependency_failed)（挂在 main 游标上，决策 90），用户决定继续/取消/等待依赖重试；"继续"= 忽略失败依赖置回 queued（决策 116） |
| review 打回 | develop.execute 追加修改项，修复后重新 review |
| 用户取消任务 | status → cancelled，清理 worktree，通知依赖任务 |
| 并发设计分支评估 | sync-check 汇聚判断，deadlock 已消除 |
| **已知残余风险：shell 可跨任务污染（决策 104）** | v1 **不做系统级沙箱**（决策 19 已修订）：文件工具受 `FileToolPolicy` 约束，但 `run_command` 的 shell 不受限，agent 理论上可读写其他任务的 worktree / 任务目录，甚至读取本机 provider 明文密钥。系统**不阻止**，只能靠 `kanban_node_commands` 全量命令日志事后审计 + `§12.14` 的文件权限控制。这是有意接受的风险，不是遗漏 |
