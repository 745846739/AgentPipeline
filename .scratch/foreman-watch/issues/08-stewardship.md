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

**Status:** done

- [x] **先改断言**：D 层恒提议的断言按新契约改写（`crates/core/tests/foreman.rs`），
      并新增「`retry` / `merge` / `review` / `cancel` 即便在托管下仍为 `Propose`」的反向断言
- [x] `kanban_tasks` 加托管标记（一列），迁移 + `types.rs::Task` 同步
- [x] `POST /tasks/{id}/stewardship`（开关）+ 董事会回读；仅 `foreman` 阶段可配（比照 `stage_may_use_ask`）
- [x] `gate_decision` 的 D 层分支加例外：**只有** `task` + `resume` + `continue` + 该任务托管中 → `Execute`
- [x] 判据只有一处实现：`tool_defs()` 与执行点白名单同源（不许两处各写一份）
- [x] **N = 2 的次数上限**：同一任务自动 `resume` 满 2 次即停手、转 pending 等你；
      计数落库（不许放内存——票 05 已经吃过内存状态的亏）
- [x] **同一诊断指纹不重复动手**：复用决策 207 的 `situation_fingerprint` 思路，
      指纹相同就不动手（单靠次数挡不住「同一件事被反复触发」，单靠指纹挡不住「每次指纹都不同
      但都没用」，两个一起才封住）
- [x] 每次自动动手**必须在班次里留一条可追溯的账**（这是硬要求，不是可选）
- [x] `docs/decisions.md` 的 206 / 207 标注被修订；persona 的权限段按托管状态改写
- [x] 新增用例：未托管 → `resume` 仍 `Propose`；托管中 → `Execute`；托管中但动作是 `retry` → 仍 `Propose`
- [x] 新增用例：第 3 次自动 `resume` 被拒且转 pending
- [x] 新增用例：同一指纹第二次不触发

**实施收尾（2026-09-18）:**

- **`resume` 抽成了 core 的**唯一实现**（`pipeline/resume.rs::apply_resume`）**：原来它整段长在
  `POST /tasks/{id}/resume` 里，而托管动作必须走**同一份**——抄一份就是两套「resume 到底做了什么」，
  迟早漂移成「界面按得动、它按不动」。端点现在只剩 HTTP 那一层（解析 / 错误映射 / 响应）。
- **执行者由 app 注入**（`StewardActionRunner` 接缝 + `runtime::StewardResume`）：core 不认识 HTTP
  那一层，而 resume 的另一半住在端点里。`serve.rs` 与 `api_contract` 的 harness 都按同一形状接线
  ——测试里少这一句，这条路就只会在生产里跑。
- **例外判据只有一处**（`agent::tools::is_stewardable_resume` + `StewardActionRunner`）：三道闸
  一起判——形状（`task`+`resume`+`continue`）、状态（托管中）、止损（次数与指纹）。
  **不注入执行者 = 不放行**（D 层照旧恒提议）。
- **一列 `stewardship_json` 而非三列**（迁移 0021）：`enabled` / `auto_resumes` /
  `last_fingerprint` 同生同死、也要一起读（放行判据三条同时成立），拆三列只会让「半边更新」成为可能。
  关掉 = **清空那一列**（不写 `enabled: false`）：留一个「关着的托管」会让「从没开过」与
  「开过又关了」在库里长得一样，而复盘时这两件事不是一回事。
- **触顶与同指纹不报错**：它们让这条路**回到提议**——任务仍停在 pending，按钮照旧给得出来。
  这就是票面「停手、转 pending 等你」的落法：停手之后人按，而不是由后端替人拍板。
- **留账两处**：会话里一条 `【托管】自动 resume：任务 t1（第 1/2 次，动作 continue）。依据指纹 …`
  的 `system` 行（操作台记的，人第二天早上读得到），加上任务行上的计数与指纹（止损线要落库）。
- **persona 的托管段是条件说的**：只有真开着托管的任务才在人格里说「你可以直接 resume」——
  一段笼统的「你可以直接动手」会立刻变成一句假话。
- **`206` / `207` 两行就地加了被修订标注**（保留原文不删，这是本仓的既有做法）。
- **票面那句「仅 `foreman` 阶段可配（比照 `stage_may_use_ask`）」的落法**：托管开关的守卫是
  「值班长没接线就 503」——`stage_may_use_ask` 守的是「谁能配 `ask`」，而托管的等价问题是
  「有没有人会用到这份授权」。终态任务另有一条 400（托管随任务自限）。
- **未做（如实记）**：界面上的托管开关（决策 210① 的「+ 界面开关」）本票没做，只有端点与回读
  （`GET /tasks` / `/tasks/{id}` 的 `task.stewardship` 已经带得出来）。它不挡任何一条链路
  （`curl` / 值班长提议的确认钮都可开），但界面上暂时只能看到状态、不能拨。留给下一轮补。

## 备注

**08 排在 03 之后**：托管要以诊断包能给出可归因的结论为前提。否则等于让一个盲诊的值班长动手——
那是这批里最不该发生的一种错。

**注意这条与调度器既有自动推进的关系**：调度器本来就在自动推进任务（超时重试
`scheduler/mod.rs:255-287`、冲突释放 `:293-343`、依赖放行 `:347-408`、准入重试 `:412-436`），
都不需要人按键。所以这条墙真正拦的不是「自动」，而是「**判断来自 LLM**」。托管放开的正是这一点，
放开的范围因此必须窄到可审计。
