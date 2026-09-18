# 03: 拆分 / 换模型进「提交中」态，杜绝双击重入

**叠:** A（不动规格）

**来源:** R2-03（代码）

**What to build:** 点「确认拆分」不会进入任何提交中态——按钮不 disabled、没有转圈。慢网络下
再点一次，会发出两次 `POST /tasks/{id}/split`：**原任务取消两次、子任务建两套**。同理「更换模型」。

根因：`stores/taskDetail.svelte.ts:290-313` 的 `submitSplit` / `submitModelOverride`
**从不设 `busyKey`**（`runAllowedAction:262` 与 `submitReview:318` 都设了），
而两个对话框拿的正是 `submitting={taskDetail.busyKey !== null}` → 恒为 false。

**Blocked by:** None（can start immediately）

**Status:** done

- [x] `submitSplit` / `submitModelOverride` 设置 `busyKey`（键含动作种类）
- [x] 成功路径照既有口径：等 SSE 回执 + safety timeout 兜底，不立即复位
- [x] 失败路径：`actionError` 可见 + 清除 busy
- [x] 对话框的 `submitting` 真的生效：按钮禁用 + 转圈
- [x] 单测：`submitSplit` 在 in-flight 期间 `busyKey !== null`；两次调用只有一次落在 client
- [x] e2e：`page.route` 延迟 split 响应 → 连点两次「确认拆分」→ 断言只发出一次请求

**边界.** 先补能跑的用例再改（本轮是代码级结论，未在浏览器复现——
`context_overflow` 这个 pending 态前端 harness 造不出来，实现票要想办法让它可测，
例如按动作种类给一个可注入的 pending 剧本）。

## 实施记录（2026-09-18）

两个方法收进同一个私有口径 `submitDialogAction(key, send)`：进 `busyKey`、**重入时交回同一个
promise**（不是静默 no-op——no-op 会让调用方把「什么都没发生」当成功去关对话框）、
成功等回执 + 30s safety timeout、失败写 `actionError` 并复位。

**怎么让 `context_overflow` 可测**：没动生产代码，也没给 harness 加播种口——e2e 里用
`page.route` **只改写那一段详情响应**（`route.fetch()` 取回真实响应，把 `pending_reason`
换成 `context_overflow`、动作集换成「拆分任务 / 更换长上下文模型」，再 `fulfill`）。
任务在后端的真实状态一字未动，被测的是前端在这套输入下的行为——正是本票的对象。
这个手法写进了 `ux2-reentrancy.spec.ts` 的文件头。

**e2e**：`frontend/e2e/ux2-reentrancy.spec.ts`（两条，进 `make check-e2e`）；
store 单测见 `src/stores/taskDetail.test.ts` 的「对话框动作的提交中态与重入护栏」四条。

