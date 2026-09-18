# 10: 新建任务按服务端返回的 id 跳转

**叠:** A（不动规格）

**来源:** R2-12（代码）

**What to build:** 看板当前在项目 A，打开「新建任务」把项目改成 B 并创建：任务**建在 B 里**，
但页面跳到 **A 的某个任务**；如果 A 一个任务都没有，就哪儿也不去、对话框关掉、人留在看板——
新任务在 B 里，当前视图看不到，**看起来像没创建成功**。

根因：`stores/board.svelte.ts:159-169` 的 `createTask` 丢掉 `POST /tasks` 的返回值，
改从 `loadTasks()` 的结果里取 `this.tasks[0]`，而 `loadTasks` 按 **`board.projectId`** 过滤
（`board.svelte.ts:60-63`）；对话框的项目是它自己的局部 `projectId`
（`NewTaskDialog.svelte:39-60`）——两个 id 可以不同。`POST /tasks` 是返回 `{task}` 的
（`crates/app/src/routes/tasks.rs:114`）。

**Blocked by:** None（can start immediately）

**Status:** done

- [x] `createTask` 返回服务端创建的**那一个** task（用响应里的 id），不再靠 `tasks[0]` 猜
- [x] 对话框跳转到该 task 的详情；项目不同也不跳错
- [x] 若确实取不到返回值：明确报错/留在原处并提示，不做静默失败
- [x] 单测：`createTask` 在项目不同 / 看板当前项目无任务时仍返回正确 id
- [x] e2e：看板在项目 A（有任务）→ 对话框选项目 B → 创建 → 断言 URL 是 **B 的那条任务**、
      且 B 的看板上出现它

**边界.** 不改对话框的项目选择交互本身。

## 实施记录（2026-09-18）

**落点**

| 处 | 改动 |
|---|---|
| `src/api/client.ts` | `createTask` 的返回类型从「丢弃」改成显式 `{ task: Task }`（后端本来就返回它） |
| `src/stores/board.svelte.ts` | `createTask` 返回**服务端创建的那一个** `Task \| null`（取 `body.task`）；**不再**从 `loadTasks()` 的 `tasks[0]` 里猜 |
| `src/components/board/NewTaskDialog.svelte` | 用返回的 task id 跳转；拿不到就**说一句**（不静默关框），并保持对话框打开 |

**为什么保留 `null` 一态**：服务端返回体缺 `task` 是「契约被破坏」，此时既不该跳转也不该
沉默——`null` 让调用点必须处理，而处理方式就是票面要的「明确报错 / 留在原处并提示」。

**证据**：
- 单测 `src/stores/board.test.ts`：项目不同、看板当前项目无任务两种情形下仍返回正确 id；
  返回体缺 `task` 时返回 `null`。
- e2e `frontend/e2e/ux2-flows-and-copy.spec.ts` ①②：走 UI 铺出项目甲（有任务）与项目乙 →
  看板停在甲 → 对话框选乙并创建 → 断言 URL 是**乙的那一条**任务，且乙的看板上有它。
