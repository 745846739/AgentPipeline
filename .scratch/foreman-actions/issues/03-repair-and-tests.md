# 03: 搬 repair 族，并给它补上第一条测试

**What to build:** 把 `run_repair_proposal`（`crates/app/src/routes/foreman.rs:534-599`）搬进
`pipeline/foreman_actions.rs` 成为 `run_repair(store, proposal) -> Result<Option<String>>`，
**并补上这条链的契约测试**——它今天在 app 与 core 两侧都零测试（全仓 grep 确认）。

搬进去的语义逐字不变：

1. 载荷读不出来 → 拒（`payload` 为 `None` 或反序列化失败）；
2. `!outcome.gate_passed` → 拒，并带上 `gate_failure_note(&outcome.gate)`；
3. `get_project(args.project_id)` 取不到 → 拒（「修复所属的项目不存在」）；
4. `rebase_onto_with_auto_resolve` **冲突 → 拒执并列出冲突文件**；
5. 干净 → `merge_into_default_branch` + `finish_repair(repo, &session, true)`。

**顺带**：`discard_repair_worktree`（`:1047-1080`）的回收逻辑与上面共用 `RepairSession` 的重建
（现在三处各建一遍：`:539-545`、`:1044-1050`、`repair.rs:470-476`）。搬迁时**收成一个 core 构造函数**
（如 `RepairSession::from_outcome(&outcome, session_id)`），让「从载荷重建会话」只有一处。

**Blocked by:** 01（module 与模式）、02（建议先落，repair 最重）。

**Status:** done（2026-09-23）

- [x] `run_repair` 落地；`foreman.rs` 的 `"repair"` 臂改成一行 core 调用；报文逐字不变
- [x] `RepairSession` 的「从 `RepairOutcome` 重建」收成一处（三处调用改调它）
- [x] `seed_proposal` 的**带载荷兄弟助手**：现有那份（`api_contract.rs:5062`）硬编码
      `payload: None` + `kind: ApiCall`，repair 用不了（`run_repair` 第一个检查就是「缺载荷」）。
      加一个可传 `kind` / `payload` 的变体，或给它加两个形参
- [x] **契约用例一（干净路径）**：真 git 仓上造一个修复分支 + worktree（`api_with_foreman` 已持有
      `api._repo`）、落一条带 `RepairOutcome` 载荷的提议、执行 → 断言分支已合入默认分支、
      worktree 已回收、分支已删（报文说「已合入 … 并回收修复 worktree（分支已删）」）
- [x] **契约用例二（冲突路径）**：让修复分支与前进后的基准冲突 → 断言 **拒执**、报文列出冲突
      文件数、**且没有发生合入**（这是决策 212① 的「指纹换义」落点）
- [x] `make check` 绿

## Comments

- **为什么这条链最值得有测试**：它是提议执行里唯一**真的动 git 仓库**的一族，也是唯一
  「执行 = 合入一个分支」而不是「执行 = 一次工具调用」的那一族（决策 212①）。而它的
  rebase 冲突拒执路径**今天从没被执行过**——`run_repair_proposal` 只在 `:546` 有一条
  `gate_passed` 的早退测试覆盖之外的分支。
- **载荷形状**：`RepairOutcome`（`pipeline/repair.rs:65`）字段为
  `repair_id` / `worktree_path` / `branch` / `base_ref` / `gate_passed` / `gate`。
  造 fixture 时照它逐字段写，别照报文猜。
- **`finish_repair(…, true)` 的 true**：合入成功才传 `true`（回收分支）；拒绝路径那头传 `false`
  （保留分支、只删 worktree，决策 212③）。两条路径的差异是**故意的**，搬迁时不要统一。
