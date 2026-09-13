# 09: 并发第二任务在浏览器层的覆盖

**What to build:** 单任务 happy path 之外，用户最常见的真实用法是**同时跑多个任务**。
Rust 层覆盖充分（E2E-09 基准前移使审批失效、E2E-12 并行互不阻塞、E2E-17 conflict_wait、
E2E-20 并发准入），但**浏览器层零覆盖**，而并发恰恰是「状态投影到界面」最容易出错的地方。

**Blocked by:** 01

**Status:** done

- [x] 用例：建**两个**任务（同一项目）→ 断言看板同时出现两张卡，各自状态独立正确
- [x] 断言**互不阻塞**：任务 A 停在某个 pending 时，任务 B 照常推进到 merge_approval
      （对应决策 89/82 的浏览器侧证据）
- [x] 断言**顶栏待办计数**在双 pending 时正确（不是只看单任务）
- [x] 断言**多游标分支归属**：让一个任务同时有两个 pending 分支（并行区间内），
      断言界面按分支分组渲染、且动作带正确的 `cursor_id`
- [x] 断言**基准前移**的界面表现：任务 A 停在 merge_approval → 任务 B 合入使基准前进 →
      对 A 点「合入」→ 断言界面未错误地显示 done，最终状态与后端真值一致
- [x] 若上述任一条暴露缺陷，按缺陷修复后重跑并在票面记录
- [x] 全量闸门绿 + `just frontend-e2e` 全过

## 交付

新增 `frontend/e2e/concurrent.spec.ts`（E2E-⑧，3 条用例），harness 增
`additionalTasks`（同 home / 同项目的附加任务，各自脚本）与 `App.taskIds`；
`scripts.ts` 增 `archBlockerRounds` / `parallelBlockerRounds` / `siblingDesignRounds` /
`siblingPassScript`。

| 用例 | 覆盖 | 断言要点 |
|---|---|---|
| ① 双任务互不阻塞 | 票面第 1、2、3 项 | 甲挂 `info_insufficient`、乙推到 `merge_approval` 后甲**仍在原处**；看板两卡各自 `.reason` 文案与动作按钮正确；`.pending-count .c` == `*2` |
| ② 并行双 pending 分支 | 票面第 4 项（决策 91 的 UI 契约） | 两分支同时 `pending`；卡片 `.head-label` == `['[dev]','[test]']`；点 [dev] 组动作 → `POST /resume` 体里的 `cursor_id` == develop-design 那条游标；resume 后 dev → `waiting_join`、test **原地不动** |
| ③ 基准前移 | 票面第 5 项（决策 96） | 乙先合入抬基准；对甲点「合入」→ 收敛判定为 `pending(merge_approval)`（**非 done**）且提案 diff 重算为只剩 `tests/acceptance.js`；二次审批 → done；主干同时含两个任务的产物 |

用例③ 的断言有个陷阱值得记下来：点「合入」**之前**甲就已经是 `merge_approval`，
所以「等到 pending(merge_approval) 就断言」会在旧状态上立刻返回、把缺陷放行成绿。
故改成等到**终局信号**（`done`，或「pending 且提案已按新基准重算」）再裁定。

## 本轮暴露并修复的缺陷（3 个，全部串行测试不可见）

1. **看板卡动作按钮被整卡导航链接覆盖**（→ 决策 164）。`TaskCard.svelte` 的
   `.card > :not(.card-link){z-index:0}` 特异性 (0,2,0) 压过 `.actions` 的 (0,1,0)，
   动作区被 `inset:0` 的链接盖住——**用户点卡上按钮只跳详情**。Playwright 报
   `element intercepts pointer events`。修复：`.card > .actions` 取同等特异性 + `z-index:2`。
2. **SQLite 快照升级失败**（→ 决策 163①）。`Store` 多步事务用 deferred `BEGIN`，
   读-写之间另一连接提交 → `SQLITE_BUSY_SNAPSHOT`（517），`busy_timeout` 救不了
   （非锁等待），节点报「database is locked」→ 重试耗尽。修复：`begin_write()`
   发 `BEGIN IMMEDIATE`，5 处多步事务改走它。钉住：`cursor_lifecycle.rs::
   concurrent_writers_do_not_fail_with_busy_snapshot`（修复前 0.05s 复现）。
3. **libgit2 建 worktree 的 TOCTOU**（→ 决策 163②）。两任务同时启动，对共享的
   `.git/worktrees` 先查后建各来一次 → 后者 `EEXIST`，init 直接挂
   `failed to make directory '.../.git/worktrees': directory exists`。修复：
   `git.rs::worktree_creation_lock` 按仓库路径串行化该窗口。钉住：`git_chain.rs::
   concurrent_worktree_creation_in_same_repo_does_not_race`（修复前复现原文）。

三个缺陷都是「只有真并发才现形」的形态——这也正是本票的存在理由。前两个是**产品缺陷**
（用户并发跑任务必然踩到），第三个是**测试基建**修正（决策 165：mock 附加任务改按任务 id
路由，因任务标题只出现在 architect prompt 里）。

## 验证

- `concurrent.spec.ts` 连跑 4 轮：3/3 稳定通过（修复前 ①/③ 必红、③ 有 1/3 概率红）。
- 全量 `npx playwright test --project=chromium`：**17 passed**。
- `cargo test --workspace`：**532 passed, 0 failed**（2 ignored 为真 LLM 冒烟）。
- `cargo fmt --check` / `clippy --workspace --all-targets -- -D warnings` 干净；
  vitest **88 passed**、`svelte-check` 0 error / 0 warning、`npm run build` 成功。

## 注记

- 用例② 顺带钉住一个「安全方向」的事实：后端下发的动作**总是**带 `cursor_id`，
  故决策 91 的「多游标选择器」兜底（`.picker`）不渲染——用户不可能把分支选错。
- 用例③ 的 fixture 需要「设计声明错开 + 实现文件错开」两层错开，原因写在
  `scripts.ts` 的 `siblingDesignRounds` / `siblingImplementationRounds` 注释里：
  声明相同会触发**合法**的 `conflict_wait`（那是冲突检测在正确工作，不是本票要测的路径），
  实现文件相同会让主任务 rebase 后 diff 退化成空差异、观察不到「重走阶段 A」。
- executor 注册表以 task_id 为进程全局键（`docs/testing.md` §11 注记）：本票两任务
  id 天然不同（各自 post 创建），未触发该坑。
