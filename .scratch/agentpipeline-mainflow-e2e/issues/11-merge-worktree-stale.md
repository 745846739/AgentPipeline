# 11: 合入后主仓库索引/工作区陈旧（主流程 e2e 暴露的真实缺陷）

**Status:** done（2026-09-13，实现过程中被票 01/02 的断言暴露并修复）

## 缺陷

`Git::merge_into_default_branch`（`crates/core/src/git.rs:586`）按决策 73 / 97 做**内存合并**：
写树 → 建双亲 commit → 直接写回 `refs/heads/{default_branch}`。这一步**只移动引用**，
不更新索引与工作区。当 `default_branch` 在某个工作区被检出时（**用户的项目主仓库正是这种
情形**），留下三方不一致：

| 位置 | 内容 |
|---|---|
| `refs/heads/main` / `HEAD` | 合入后的新提交（正确） |
| 索引（index） | 旧提交 |
| 工作区磁盘文件 | 旧代码 |

实测证据（真实二进制 + 真实仓库，2026-09-13）：

```
=== HEAD src/lib.js ===      合入后新内容（正确）
=== INDEX :src/lib.js ===    旧内容（"not implemented" 占位）
=== DISK src/lib.js ===      旧内容
=== status --porcelain ===
M  src/lib.js            ← 已暂存
D  tests/acceptance.js   ← 已暂存
=== npm test（工作区实况）===  exit=1（旧代码跑不过）
```

## 为什么这是「主流程不会出现 bug」必须堵住的一类

三条用户可见后果，全部在主流程上：

1. **用户查看合入结果时看到旧代码**——在项目目录里 `cat` / 编辑器打开，看到的不是刚合入的内容。
2. **用户一次 `git commit` 就把合入回滚掉**——`M`/`D` 是**已暂存**状态，提交它会写入旧内容，
   主干被倒退。这是最危险的一条：合入越「成功」，用户越容易踩。
3. **合入成功反而破坏下一次合入**——决策 61 的「目标分支工作区不干净」检查在下一个任务的
   merge 处会读到这些陈旧差异，判定为脏 → `pending(user_decision, dirty_worktree)`，
   用户被要求「手动处理」一个自己从未制造的问题。**串行跑两个任务是用户的正常用法。**

## 修复

`crates/core/src/git.rs` 新增 `sync_checked_out_worktree`：引用前移后，仅当 **HEAD 正指向
该 `default_branch`** 时执行 `checkout_head(force)` + 索引同步；分支未被检出（正在别处工作 /
裸仓库）时不触碰任何工作区。

- ff 与 no-ff 两条路径都调用。
- force 的安全性：合入前调用方已按决策 61 校验工作区干净（`allow_dirty_worktree_merge = false`
  为默认），因此这里覆盖的只是**本次合入自身造成的**索引滞后，不会丢弃用户未提交的改动。
- 未采用「整仓 `reset --hard`」：那会越过决策 61 的意图（用户可能有别的分支在被检出、
  或另有 worktree 在工作），只同步「当前 HEAD 所在的那份工作区」是最小且正确的范围。

## 钉住缺陷的用例

`crates/core/tests/git_chain.rs::merge_leaves_default_branch_worktree_consistent`：
合入后断言主仓库 `status` 干净、`HEAD:src/lib.rs` 与**磁盘文件**均为合入后内容。
修复前该用例红（`M src/lib.rs`），修复后绿。

另在浏览器层补了两条更强的断言（票 02）：
- happy path 断言 `main` 上代码内容正确 + 闸门**真的跑过** + 合入后 fixture 仓库里
  `npm test` 能独立通过（这条正是被本缺陷打红的——它证明合入的代码自身自洽，
  而不是靠陈旧工作区混过去）。

## 与决策的关系

- **修订决策 73 / 97 的实现范围**：两者规定「合入结果必须显式写回分支」——本缺陷说明
  「写回分支」之外还需**同步被检出的工作区**，否则「写回」在用户视角不成立。已追加决策 158。
- 不改决策 61 语义（脏工作区仍挂起），本缺陷正是让 61 不再被误触发。

## 验收

- [x] `git_chain` 新用例红 → 绿（修复前后各验证一次）
- [x] 全量 `cargo test --workspace` = 487 全过
- [x] `just frontend-e2e` 两条全过（含新断言）
- [x] 决策 158 已追加（只追加，不修改既有行）
