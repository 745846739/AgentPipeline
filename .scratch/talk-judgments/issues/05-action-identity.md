# 05: 动作身份收成一种拼法——生产者与消费者的契约今天有两条

**What to build:** 「哪个动作正忙」这件事有**两种身份拼法**，而它们不该同时存在。

规范形在 `lib/actions.ts:115-123 actionKey()`：**四段** `action:cursorId:stage:node`
（`lib/actions.test.ts:104-153` 钉着它）。消费者照规范形走的有 `stores/taskDetail.svelte.ts`
（`:296` / `:315` / `:375` / `:384`）、`components/board/PendingActions.svelte`（6 处）、
`components/task/DiffReviewPanel.svelte`（2 处）。

但 `stores/board.svelte.ts:114` 写的是**另一种**：

```ts
const key = `${taskId}:${action.action}`;   // 两段，taskId 在前，无 cursor / 落点
this.actionBusy = key;
```

它的消费者跟着生产者的拼法走：`Talk.svelte:1530`
（`isBusy={(a) => board.actionBusy === `${task.id}:${a.action}`}`）与
`TaskCard.svelte:62`。而 `TaskCard.svelte:63` 还**手工去桥接规范形**
（`` `${action.action}:${cursorId ?? ''}` ``，两段且顺序相反）——那是 `actionKey(a)` 的**截断版**，
只在 `target` 缺席时才等于四段式。`§12.3` 有一行管这件事（「同名动作不是一个动作：
身份 = 动作名 + 游标 + 落点」），但它指的三个落点里**不含** `board.svelte.ts` 与 `Talk.svelte`
——第二个拼法长在映射表没盖到的地方。

**为什么单独立票**：这是 **store 契约**，面比候选 10 的三块判断宽（board store + TaskCard +
Talk + BoardColumn 的 prop 管道），且 `actionKey` 的语义（含游标与落点）与 board 那份
（不含）**不是同一个函数**——合并要先定「board 那条待办是否总有一个明确的游标与落点」。
塞进本批会把一次去重变成一次语义变更。

**Blocked by:** None（可立即开始）

**Status:** done（2026-09-23 实现；取证 + 收口 + 守卫，见交付说明）

- [x] 先取证：`board.svelte.ts:110-118` 的 `cursor` 解析（`action.cursor_id` → `cursors.find` →
  `cursors[0]` 兜底）说明它**手里有游标**，故它有条件产出四段式。逐条核对三个消费者
  （`Talk.svelte:1530` / `TaskCard.svelte:62-63` / `Board.svelte:214` 的 prop 管道）
  在四段式下是否仍能正确判忙
- [x] 定裁决：`actionBusy` 改为存 `actionKey(action, cursorId)` 的返回值；
  `TaskCard.svelte:63` 的手工桥接随之删除（它是为兼容两种拼法而写的）
- [x] 核对 `stores/taskDetail.svelte.ts:401` 那处**两段字面量**（`'review:approve'` / `'review:reject'`）
  ——它是第三种写法，一并收进裁决
- [x] 补测：`actionKey` 在 board 路径上的调用各一条；「同名动作、不同游标 → 不是同一个忙」一条
  （这正是 `§12.3` 那一行要守的语义）
- [x] `design/frontend-design.md` §12.3 那一行的落点补上 `stores/board.svelte.ts` 与
  `routes/Talk.svelte`（今天没指它们，第二个拼法因此在映射表外生长）

## 交付说明（2026-09-23）

**取证结论**（票面第一格的四件）：

1. **`board` 手里确实有游标**：`handleTaskAction` 本就解析 `action.cursor_id → cursors.find →
   cursors[0]`（那份解析另有用途：`pendingType`），且后端 `allowed_actions()` 把 `cursor_id`
   **用 `with_cursor` 应到全部动作**上——游标是 ULID、**全局唯一**，故四段式不带 `taskId`
   也跨任务分得开（旧两段式里的 taskId 防的正是跨任务相撞，四段式用游标达成了同一目的）。
2. **三个消费者在四段式下都能正确判忙**：Talk 与 TaskCard 把 `cursorId` 交给
   `PendingActions` 的 `fallbackCursorId`，而发动作时 emit 的也是**同一个** `fallbackCursorId`
   （emit 与 `busy()` 同源）——store 存 `actionKey(action, opts.cursorId)` 与消费端
   `actionKey(action, cursorId)` 对同一个动作恒等。残余边缘：`selectedCursor` 在提交途中被换掉
   会让比较短暂失配——这与 taskDetail 既有形态完全相同，是规范形本身的已知性质，不新增。
3. **`taskDetail:401` 的两段字面量是第三种拼法，但只写不读**：全仓没有一处拿
   `'review:approve'` / `'review:reject'` 的**值**做相等比较（评审按钮只查 `busyKey !== null`）
   ——故它不参与身份判定、不与四段式竞争。**裁定：不并**（评审表单不是流水线动作，没有游标与
   落点，硬塞只会造出一段恒空的伪身份），原地加注释记下这条取证。

**改动**：`board.svelte.ts` 忙键改存 `actionKey(action, opts.cursorId)`；`TaskCard.svelte`
的手工双桥**整段删除**（两段 taskId 式 + 截断版各一）；`Talk.svelte` 的 `isBusy` 改
`actionKey(a, cursorId)`。三处旧拼法固定串全仓 grep 清零（注释里的引用也改写成不带 `${}` 的
叙述，免得守卫误伤）。

**测试**：`board.test.ts` 新增 2 条（拖住提交在半路断言 `actionBusy === 'continue:c-main::'`；
同名不同游标不是同一个忙）+ `delegation-scan` 新 describe 2 条（三方 import 都指 `lib/actions`、
三处旧拼法清零）。`actions.test.ts` 已有的「同名不同落点 / 裸动作不同游标」覆盖纯函数侧，未重复。

**§12.3**：`:825` 行的落点补了 `stores/board.svelte.ts`、`routes/Talk.svelte`、
`components/board/TaskCard.svelte`（票面点名前两个；TaskCard 是该行管辖的**比较点**之一，
一并补上），备注列补「票 05」。

**不产生新决策**：这是把看板路径并回既有规范形（票 20 立的 `actionKey`），不是新语义。
