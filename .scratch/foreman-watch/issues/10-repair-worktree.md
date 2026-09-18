# 10: 修复 worktree 的生命周期

**What to build:** 一次「修复」的载体——一个**独立于任务工作区**的 git worktree。

**两份仓都可修，按诊断结论分派。** 本仓（AgentPipeline 自己）**已经注册成了 project**
（`kanban_projects`：`local_path = /Users/lazyking/Documents/AgentPipeline`，`default_branch = main`，
`test_framework = cargo`，`lint_command = cargo clippy`），所以「让流水线修自己」在机制上端到端现成。

**本仓的补丁写在独立 worktree 里，不是直接改工作区。** 三条理由都是硬的：

- **可达性**：值班长的文件域是 `workdir_bound = vec![home.root()]`，`FileToolPolicy::check` 的第③条
  对**读和写都强制**「解析后的路径必须落在允许根之内」（`crates/core/src/agent/file_policy.rs:119-136`）。
  本仓在 `~/Documents/AgentPipeline` → `write_file` / `edit_file` **写不了它任何一个文件**。
  唯一能碰到它的是 `run_command`（命令不受文件策略管，`file_policy.rs:70-75` 自己写着
  「这是补偿，不是边界」），而那条路是 shell heredoc，**没有账**。
  worktree 落在 `{home}/worktrees/` 下 → 可达。
- **隔离**：不污染你正在开发的工作区。
- **diff 天然**：产物天然是 `{base}..{branch}`。

**不新建 `kanban_tasks` 行**（用户裁决「修复要快」）。代价如实记：这条修复不在看板上、
不进 `kanban_node_cursors` 那套账——所以票 12 的提议表要承担它的账。

**Blocked by:** 09

**Status:** done

- [x] 修复 worktree 的创建与清理生命周期独立于 init 阶段
      （`Git.init_worktree` 现签名是 `(project_path, task_id, worktree_path, default_branch)`，
      `crates/core/src/git.rs:312-318`，且只被 init 调用）
- [x] 分支命名：与流水线任务的分支可区分（`Git::branch_for` 是 `kanban/{task_id}`，
      修复分支要有自己的前缀，否则 `git branch --list` 里分不出哪些是修复）
- [x] worktree 落在 `{home}/worktrees/` 下 → 在值班长的写域内（**有断言钉住**
      `FileToolPolicy::check_write` 对它返回 `Ok`）
- [x] **base 的取法**：与 merge 阶段一致——有 `origin` 用 `origin/{default_branch}`，否则本地分支
      （`Git::base_ref`，`git.rs:292-305`；本仓无 remote，故走本地）
- [x] 修复轮里允许的写工具与命令域**明确列出**（不靠「碰巧域够大」）
- [x] 回收：合入成功 → 删分支 + 删 worktree；被拒 / 年龄清理 → **保留分支、删 worktree**
      （分支是唯一的证据，与决策 207「过期只让按钮变灰、那一轮留在时间线」同一理由）
- [x] 复用既有的按仓库串行的 `with_worktree_lock`（`git.rs:88-105`）——
      libgit2 建 worktree 对 `{repo}/.git/worktrees` 是「先查后建」，跨任务并发会撞 `EEXIST`
- [x] 所有 git 调用走 `blocking`/`blocking_within` 的兜底上限（不许新增无界阻塞点）
- [x] 新增用例：从零拉起一个修复 worktree，断言分支从正确 base 分出、目录在 home 下、
      且 `FileToolPolicy` 允许写它
- [x] 新增用例：两个修复并发拉同一仓库的 worktree，**都成功**（`with_worktree_lock` 的牙齿）
- [x] 新增用例：回收后 worktree 目录消失、分支按规则留存或删除

**实施收尾（2026-09-18）:**

- **创建走 `Git::init_worktree_named`**（`init_worktree` 也改成调它）：base 的取法、幂等复用、
  `with_worktree_lock` 的串行化、unborn HEAD 的明确报错**都只有一份实现**——修复与 init
  在这几件事上必须逐字相同。
- **路径 `{home}/worktrees/repair-{id}`**（`Home::repair_worktree_path`）：前缀 `repair-`
  让它与任务 worktree 一眼可分；落在家目录下是**可达性的硬约束**，用例正面断言
  `FilePolicy::check_write` 通过、反面断言项目工作区写不进去。
- **分支 `repair/{id}`**（`Git::repair_branch_for`）：可检索是唯一理由——`git branch --list`
  里要一眼看出哪些是修复产物。
- **回收规则**在 `finish_repair(merged)`：合入删分支、被拒留分支（分支是唯一证据）。
- **「修复轮里允许的写工具与命令域」如实记**：域不是新划的——就是决策 206 给值班长的
  `home.root()`，而 worktree 落在它下面。写工具仍是 `write_file` / `edit_file` / `run_command`
  那三件（命令域走既有的出口策略），**没有为修复新开任何域或工具**。
- **`blocking` 与 `gerr` 从 private 提为 `pub(crate)`**：修复模块要用它们，而新增第二份
  「阻塞 + 超时兜底」的包装正是这一票要避免的东西。
## 备注

**这是本批最实的一块工程量。** 前面几票都能用既有接缝拼出来，这一票要新管一套生命周期。
它也是唯一一票会**扩大值班长实际能碰的文件范围**（到 `{home}/worktrees/` 下的修复分支）——
那正是决策 206 划的写域之内，不需要再扩一次域。
