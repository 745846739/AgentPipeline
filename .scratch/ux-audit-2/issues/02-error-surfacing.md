# 02: 错误要说得出口，失败要有出路

**叠:** A（不动规格）

**来源:** R2-06（实测）+ R2-07a（实测）+ R2-07c / R2-08（代码 + 实测）

**What to build:** 全站错误既听不到也走不出去：

- **听不到**：11 条路由上 `aria-live` 恒为 1（就是完成横幅 toast）、`role="alert"` 恒为 0——
  加载失败横幅、动作提交失败横幅、表单校验错误，一个都不播报。
- **走不出去**：市场断网刷新后技能列表连同「刷新」「查看技能」两个钮一起消失（实测
  `刷新钮 1→0`、`技能行 0`）；设置五页的错误横幅没有重试钮；详情页首次加载失败是永久死页。
- **静默**：重试/归档没有 `catch`，失败后按钮静静复活（实测注入 500 后 `bannerErrors:[]`）。

**Blocked by:** None（can start immediately）

**Status:** done

- [x] 错误横幅用 `role="alert"`（加载/提交失败是打断级，不用 `polite`）
- [x] 表单错误给字段 `aria-invalid` + `aria-describedby`，并同样可播报
- [x] 市场刷新失败**保留已列出的 `list`**，错误态自带「重试钮」
      （`SettingsMarket.svelte:437-442` 的 `listError` 分支在 `list` 之前，是根因）
- [x] 设置四页 + 详情页的错误横幅都有可点的重试
- [x] `TaskDetail.svelte:181-190` 的 `bypass()` 补 `catch`，失败写进 `actionError`
- [x] e2e：注入失败后断言 `role=alert` 文本出现；市场断网刷新后断言技能仍在且有重试钮
- [x] e2e：`page.route` 让重试返回 500 → 断言出现可见的失败反馈

**边界.** 不新增「全站统一错误组件」这种抽象——各页现有的 `.banner.error` 就是落点。

## 实施记录（2026-09-18）

**落点（逐处就地改，零新抽象）**

| 处 | 改动 |
|---|---|
| `Board.svelte` | 两条错误横幅 `role=alert`；加载失败横幅下加「重试」→ `board.loadTasks()` |
| `TaskDetail.svelte` | 两条横幅 `role=alert`；后台 refetch 失败加「重试」；失败空态的原因行 `role=alert` + 「重新加载」 |
| `SettingsProjects / Providers / Stages / Share` | 横幅 `role=alert` + 「重试」（`load()` 从 `onMount` 里提出来，Share 也提了） |
| `SettingsMarket.svelte` | 横幅 `role=alert` + 「重试」；`listError` 分支**挪到 `list` 之后**并与列表并存、自带重试；`addError` / `saveError` / `installError` 各自 `role=alert` |
| `ProjectForm / ProviderForm / NewTaskDialog` | 校验结果带上**哪一格** → 该格 `aria-invalid` + `aria-describedby` 指向错误节点；错误节点 `role=alert`；`ProviderForm` 现在自己先校验（父页只留兜底） |
| `Metrics.svelte` | 任务指标错误 `role=alert` |
| `SplitDialog / ModelOverrideDialog` | 错误 `role=alert` |

`validateProjectDraft` / `validateProviderDraft` 的返回值从 `string | null` 变成
`{ field, message } | null`（字段级结果才有落点可用）；`providers.test.ts` 与
`TaskDetail.test.ts` 里钉旧形状的断言随之改写。

**e2e**：`frontend/e2e/ux2-failure-paths.spec.ts`（三组：详情失败 / 市场断网 / 设置页读不到）。

## 取证订正：R2-08 的 ②.1 是**假阳性**

报告里 ②.1「重试失败完全静默」的取证不成立：它注入的是 `page.route('**/retry**')`，而那个
界面上名为「重试」的钮是 `pending(retry_exhausted)` 的**「重试执行」**——一个 `goto`/resume 动作，
实际请求是 `POST /tasks/{id}/resume`，**不含 `/retry`**，所以注入的 500 从未生效；请求照常成功，
`bannerErrors: []` 是「本来就没失败」。

代码结论仍然成立并已修：`bypass()`（终态任务的「重试（回到 init）」/「归档」）确实没有 `catch`，
失败就是静默。新的取证走**真会失败的路径**：合入 → done → 终态旁路行出现 → 注入
`POST /tasks/{id}/archive` 500 → 点「归档」→ 断言 `.main [role=alert]` 里出现「动作提交失败」。

`frontend/e2e/ux-audit-2.spec.ts` 的 ②.1 未改（那是历史证据），但下一轮读它时按本条订正理解。
