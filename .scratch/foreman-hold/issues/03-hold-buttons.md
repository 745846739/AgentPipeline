# 03: 界面上那两颗钮（在跑的任务）

**What to build:** 任务详情页在 `running` 上摆两颗钮——**暂停** / **重跑本阶段**。按住之后
任务转 pending，动作坞换成后端下发的 `allowed_actions`（`continue` 续跑 / `goto` 重跑本阶段 /
`cancel`），**界面不给「续跑」写第二份逻辑**。

**Blocked by:** 01（端点）与 02（按住之后不被打扰的豁免）

**Status:** done（2026-09-25）

**要点：**

- 只在 `running` 上摆：`queued` / `waiting` 还没开跑（后端会拒），终态有自己的两颗
  （`bypassActions` 的 retry / archive）。「续跑」不在这里——其他每一种待办都是同一条路。
- 回执里后端那句**事实**（`notified`：有没有真的通知到在跑的执行体）照原样显示，界面不
  自己拼一句「已经停稳了」。
- 失败不静默：走页面既有的动作错误位（与终态旁路动作同一条纪律，票 02 / R2-08）。

- [x] `frontend/src/routes/TaskDetail.svelte`：`canHold` 派生 + `hold('pause' | 'rerun')`
      + `bypassActions` 的新分支
- [x] `frontend/src/api/client.ts`：`pauseTask` / `rerunTask`
- [x] `frontend/src/lib/pipeline.ts`：`pendingLabel` 的「已暂停」
- [x] 镜像契约两侧：`frontend/src/api/types.ts` 的 `PENDING_KIND_MEMBERS` + fixture
      （`lib/enumMembersFixture.test.ts` 与 §5 的表测试钉住）
- [x] `design/frontend-design.md` §12.3 行为映射加一行（`behavior-map.test.ts` 的悬空检查
      照旧绿）
