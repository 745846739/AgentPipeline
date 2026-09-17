# 08: 任务级托管开关 + D 层例外（含冻结断言改写）

**What to build:** 让值班长**在指定任务上**可以免按键 `resume(continue)`，默认仍然恒提议。

**这是本批唯一的破坏性改动。** `gate_decision` 今天对 `task` / `config` / `skills` 三个工具
**不读档位、恒返回 `Propose`**（`crates/core/src/agent/tools.rs:148-165`，决策 206 的原话是
「D 层不读档位」）。托管要生效，必须在那一支上开一个**恰好一个动作**的例外。

**为什么是任务级而不是全局档位**：托管是**例外**而不是**模式**。一个全局开关一旦拨过去就再也
回不来（你不会记得自己什么时候拨的、也不知道今晚它是开着的）；任务级托管天然自限（任务一 done
就失效），且与「一个任务 + N 次」的次数上限是同一个东西的两面。

**允许的**：`task` 工具的 `resume`，且**仅限 `continue`**（不是 `skip` / `goto`）。

**永远不允许**：

- `retry`——它会 `git reset --hard {base_ref}` + `git clean -fdx`（`crates/app/src/routes/tasks.rs:321-370`），
  会洗掉工作区
- `merge` / `review`——写回主干或替人拍板
- `cancel` / `create`——一个丢掉工作、一个花钱
- `config` / `skills` 两族——全局或装代码

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] **先改断言**：D 层恒提议的断言按新契约改写（`crates/core/tests/foreman.rs`），
      并新增「`retry` / `merge` / `review` / `cancel` 即便在托管下仍为 `Propose`」的反向断言
- [ ] `kanban_tasks` 加托管标记（一列），迁移 + `types.rs::Task` 同步
- [ ] `POST /tasks/{id}/stewardship`（开关）+ 董事会回读；仅 `foreman` 阶段可配（比照 `stage_may_use_ask`）
- [ ] `gate_decision` 的 D 层分支加例外：**只有** `task` + `resume` + `continue` + 该任务托管中 → `Execute`
- [ ] 判据只有一处实现：`tool_defs()` 与执行点白名单同源（不许两处各写一份）
- [ ] **N = 2 的次数上限**：同一任务自动 `resume` 满 2 次即停手、转 pending 等你；
      计数落库（不许放内存——票 05 已经吃过内存状态的亏）
- [ ] **同一诊断指纹不重复动手**：复用决策 207 的 `situation_fingerprint` 思路，
      指纹相同就不动手（单靠次数挡不住「同一件事被反复触发」，单靠指纹挡不住「每次指纹都不同
      但都没用」，两个一起才封住）
- [ ] 每次自动动手**必须在班次里留一条可追溯的账**（这是硬要求，不是可选）
- [ ] `docs/decisions.md` 的 206 / 207 标注被修订；persona 的权限段按托管状态改写
- [ ] 新增用例：未托管 → `resume` 仍 `Propose`；托管中 → `Execute`；托管中但动作是 `retry` → 仍 `Propose`
- [ ] 新增用例：第 3 次自动 `resume` 被拒且转 pending
- [ ] 新增用例：同一指纹第二次不触发

## 备注

**08 排在 03 之后**：托管要以诊断包能给出可归因的结论为前提。否则等于让一个盲诊的值班长动手——
那是这批里最不该发生的一种错。

**注意这条与调度器既有自动推进的关系**：调度器本来就在自动推进任务（超时重试
`scheduler/mod.rs:255-287`、冲突释放 `:293-343`、依赖放行 `:347-408`、准入重试 `:412-436`），
都不需要人按键。所以这条墙真正拦的不是「自动」，而是「**判断来自 LLM**」。托管放开的正是这一点，
放开的范围因此必须窄到可审计。
