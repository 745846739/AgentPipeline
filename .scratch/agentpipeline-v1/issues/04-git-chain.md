# 04: Git 链（worktree 隔离与合并）

**What to build:** 全部 git 操作原语：任务 worktree 创建（origin 优先基底、unborn HEAD 显式报错、幂等复用）、rebase（冲突检测 + 系统侧 abort）、merge 链（ff / --no-ff + update-ref 写回、脏工作区拒绝）、diff patch、retry 重置、取消清理。

**Blocked by:** None（can start immediately）

**Status:** done（已实现）

- [x] init_worktree：origin 优先（决策 41）、unborn HEAD 显式错误（决策 61）、幂等
- [x] rebase_onto 返回 Clean/Conflict，冲突系统侧 abort（决策 74）
- [x] merge_into_default_branch：ff 或 --no-ff，detached worktree 合入后 update-ref 写回（决策 73/97）
- [x] retry 重置 base + clean untracked（决策 125）；清理幂等（决策 3）
- [x] git_chain 测试套件全覆盖（含回归点：update-ref 写回）
