# 06: 人工评审分支 + 合并「返回修改」的浏览器覆盖

**What to build:** 主流程有两条正常分支在浏览器侧零覆盖：

**① 人工评审**：新建任务时可选「人工评审」（`NewTaskDialog.svelte` 的 `review_mode`），
选后任务会停在 `pending(human_review)` 等人操作（`executor.rs:2202`）。Rust 层有 L4 用例
（E2E-04 approve → test / reject → develop），但**前端渲染从未验过**：`PendingDossier.svelte`
在 `pendingType === 'human_review'` 时渲染「三件套」（评审报告 / 单测报告 / diff，
见 `PendingDossier.svelte:101-111`、`156-166`），approve / reject 动作经
`endpointFor` 映射到 `POST /review`（`actions.ts:28-34`）——这条「人工评审的专用面板 +
双动作端点」的链路没有一条端到端用例。

**② 合并「返回修改」**：`merge_approval` 的动作集是 `approve`（合入）+ `return`（返回修改）
（`actions.rs:246-249`）。现有 happy path 只点了「合入」（`happy-path.spec.ts:64`）；
`return` → 打回 develop 这条路径在浏览器侧未验，而它与 approve 共用 `pendingType` 分支、
**端点相同但语义相反**（`actions.ts:28-32` 按 pendingType 解析到 `/merge/decision`）——
正是最容易接错的一处。

**Blocked by:** 01

**Status:** done（2026-09-13）

**实现期暴露的真实缺陷**：merge「返回修改」与人工评审决策落库后任务投影不同步——
`GET /tasks/{id}` 在执行器下次写库前继续报旧 pending，用户对着陈旧状态再次提交决策
（实测二次合入 404）。修复见票 13 与决策 161；浏览器用例 ②③ 的中间态断言因此改为
流转时间线 + 修好的投影。

- [x] **人工评审 — approve 路径**：UI 建任务时选 `review_mode=human` → 推进到
      `pending(human_review)` → 断言 dossier 出现人工评审面板（三件套中至少断言评审报告与 diff
      可见）→ 点「评审通过」→ 断言离开 human_review 并向 test 推进
- [x] **人工评审 — reject 路径**：点「评审不通过，打回开发」→ 断言任务回到 develop
      （可通过 `current_stage` / 后续重新进入 review 断言）
- [x] 断言两个动作**发往同一端点但结论相反**（用 `page.waitForResponse` 抓 `/review` 请求体，
      断言 `approved` 字段分别为 true / false）——这是防接错的关键断言
- [x] **合并「返回修改」**：推进到 `pending(merge_approval)` → 点「返回修改」→
      断言 approval 重置、任务回到 develop.execute；随后再次推进到 merge_approval 并「合入」→ done
- [x] 断言「返回修改」请求打到 `/merge/decision` 且 decision 为 return（区别于 approve）
- [x] 断言人工评审的**意见输入**（若有）随结论进流转原因（对应 L3 已覆盖的
      「comments 进流转原因」，此处补浏览器侧）
- [x] `harness.ts` 的 mock 脚本支持 human 模式与 return 后的二次推进（按轮投喂，
      注意 `scripts.ts` 头注的「按轮」语义）
- [x] 全量闸门绿 + `just frontend-e2e` 全过

**不做什么：** 其余 7 种 pending 的在浏览器逐条渲染（非主流程，见 spec「不做什么」）。
