# 03: 拆分 / 换模型进「提交中」态，杜绝双击重入

**叠:** A（不动规格）

**来源:** R2-03（代码）

**What to build:** 点「确认拆分」不会进入任何提交中态——按钮不 disabled、没有转圈。慢网络下
再点一次，会发出两次 `POST /tasks/{id}/split`：**原任务取消两次、子任务建两套**。同理「更换模型」。

根因：`stores/taskDetail.svelte.ts:290-313` 的 `submitSplit` / `submitModelOverride`
**从不设 `busyKey`**（`runAllowedAction:262` 与 `submitReview:318` 都设了），
而两个对话框拿的正是 `submitting={taskDetail.busyKey !== null}` → 恒为 false。

**Blocked by:** None（can start immediately）

**Status:** open

- [ ] `submitSplit` / `submitModelOverride` 设置 `busyKey`（键含动作种类）
- [ ] 成功路径照既有口径：等 SSE 回执 + safety timeout 兜底，不立即复位
- [ ] 失败路径：`actionError` 可见 + 清除 busy
- [ ] 对话框的 `submitting` 真的生效：按钮禁用 + 转圈
- [ ] 单测：`submitSplit` 在 in-flight 期间 `busyKey !== null`；两次调用只有一次落在 client
- [ ] e2e：`page.route` 延迟 split 响应 → 连点两次「确认拆分」→ 断言只发出一次请求

**边界.** 先补能跑的用例再改（本轮是代码级结论，未在浏览器复现——
`context_overflow` 这个 pending 态前端 harness 造不出来，实现票要想办法让它可测，
例如按动作种类给一个可注入的 pending 剧本）。
