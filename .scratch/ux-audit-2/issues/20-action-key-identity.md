# 20: 同名动作撞 key，整块动作区停更

**叠:** A（不动规格）

**来源:** 本批实现过程中现场发现的**新缺陷**（不在第二轮审计的 23 条里）——写票 17 的
430px 用例时，`expectBundleHealthy` 在真浏览器里抓到未捕获的 Svelte 运行时错误
`each_key_duplicate`，顺着它剥出来的

**What to build:** 后端下发的动作集里，**同名动作可以合法地出现两次，两条的落点不同**：

- `retry_exhausted@develop|test`：「重试执行」+「带失败摘要回架构设计修订」（两个 `goto`，
  `crates/core/src/actions.rs` 的 `RetryExhausted` 分支）
- `user_decision@test_code_issue|gate_recheck`：「修改测试用例」+「修改业务代码」（同样是两个 `goto`）

而渲染层的 key 是 `action.action + (action.cursor_id ?? '')`（`PendingActions.svelte` 三处、
`DiffReviewPanel.svelte` 两处），**只看名字 + 游标**——两条 `goto` 撞 key，Svelte 抛
`each_key_duplicate`，于是：

> **整块动作区不再更新**：坞里留着**上一个 pending** 的按钮（实测 430px 详情页停在
> `重试耗尽` 时，坞里画的还是上一态 `信息不足` 的「补充信息并继续」/「取消任务」），
> 而用户点下去发的是那个停在屏上的旧动作。

同一个 key 还被 `busyKey` 用（`stores/taskDetail.svelte.ts` 的 `isBusy` / `runAllowedAction`），
所以即使不撞 key，点其中一颗 `goto` 会让**两颗一起**进入提交中转圈。

**取证（2026-09-18，真二进制 + 真浏览器 + Chromium）**：把 svelte 内部 `DEV` 打开后重跑，
错误报出重复的 key 与下标——

```
Keyed each block has duplicate key `goto01M2SPXCTSTW5M8YF1J22AF6W7` at indexes 0 and 2
```

`indexes 0 and 2` 正是 `retry_exhausted@develop` 的 resume 组 `[goto, skip, goto]`。
组件级复现（jsdom）在修复前 3 条用例全红、修复后全绿。

**Blocked by:** None（已修）

**Status:** done

- [x] `lib/actions.ts` 加 `actionKey(action, cursorId?)`：身份 = 动作名 + 游标 + **落点**
      （`target.stage` / `target.node`），并写清为什么必须有这把尺子
- [x] `PendingActions.svelte` 的三处 `{#each}` key 与 `inputs` 槽位改用 `actionKey`
- [x] `DiffReviewPanel.svelte` 的两处 `{#each}` key 改用同一把尺子
- [x] `stores/taskDetail.svelte.ts` 的本地 `actionKey` 删掉，改为 import——
      否则「提交中」态与渲染层的身份两套算法（`isBusy` 恒不中或一起中）
- [x] 单测 `lib/actions.test.ts`：两条 `goto` 的 key 互不相同、对象重造不改变身份、
      动作自带游标优先于显式传入
- [x] 组件级回归 `components/board/PendingActions.test.ts`：`retry_exhausted` 的完整动作集
      渲染出四颗钮、提交中态只落在被点的那颗、点哪颗发哪颗（**修复前三条全红**
      `each_key_duplicate`，修复后全绿）
- [x] 组件级的确定性回归（上面那条）；**e2e 只作旁证**：`ux2-resilience.spec.ts` ⑤ 第二次停靠
      落在 `retry_exhausted` 时会经过这一档（起初在那里就地断言了两颗 `goto`，后来**撤掉**——
      实测第二次停靠在 `retry_exhausted` 与 `conflict_wait` 之间随整份用例的次序摆动，
      把「必到某一档」写进断言就是给自己埋一根摇的钉子；见下面的实施记录）

**边界.** 不动动作集（后端下发什么画什么）；不动 `allowed_actions` 的语义；不改标签文字。

## 实施记录（2026-09-18）

| 处 | 改动 |
|---|---|
| `src/lib/actions.ts` | 新增 `actionKey(action, cursorId?)`（导出、带长注释解释 ①② 两种后果） |
| `src/components/board/PendingActions.svelte` | 三处 each key + `inputs` 记录 + `busy()` 全走 `actionKey`；删掉本地同名函数 |
| `src/components/task/DiffReviewPanel.svelte` | 两处 each key 走 `actionKey`（`cursorIdFor(action)` 兜底游标） |
| `src/stores/taskDetail.svelte.ts` | 删掉本地 `actionKey`，import 共享的那把 |

**顺带订正一处测试口径**：`ux2-resilience.spec.ts` ⑤ 的注释原写「resume 之后再到
`merge_approval`」，实测第二次停靠是 `retry_exhausted@test.execute`（本轮 mock 只给 test
节点喂了一轮，重入的那几轮拿不到结构化元数据）。注释已按实测改写，并把「这一档就是两条
`goto` 那一档」写进去——**用例的意图没变**（它要的是一次页面开着时发生的 pending 迁移），
但描述与事实对齐了。

**一处回退（2026-09-18，全量 e2e 复查之后）**：⑤ 里那两条「坞里两颗 `goto` 都在」的断言
已删。同一条流在全量跑里落到 `conflict_wait`（截图取证：坞里只有「取消任务」，标题行
`等待冲突任务`，冲突方是仍在册的兄弟任务）——两个任务都声明 `src/lib.rs`，谁还挂在册上
就决定第二次停靠在哪儿。断言里只留几何（toast 不压坞、`elementFromPoint` 命中坞按钮），
动作身份那条回归交给组件级用例：那里三颗钮的输入是写死的动作集，撞 key 必抛。

**一个流程教训值得记**：这条缺陷**第一轮的静态审查与第二轮的 `ux-audit-2.spec.ts` 都没抓到**，
它是在写 A 叠实现票的 e2e 时被 `expectBundleHealthy`（主流程票 01 那道「页面不许有未捕获
错误」的闸）当场抓住的。窄款 + 真 pending 迁移 + 健康闸三者缺一，它就会一直躺着——
而那三样恰好都是这一批新做的。
