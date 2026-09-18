# 06: 全站语义：`<main>`、看板的 `<h1>`、页签的 tablist、aria-current

**叠:** A（不动规格）

**来源:** R2-19 / R2-20（实测）

**What to build:** 实测 11 条路由的结构：

| 缺什么 | 现状 |
|---|---|
| `<main>` | 只有看板有；**另外 10 条路由为 0** |
| `<h1>` | 看板 **0**、404 **0**；任务详情 **h1 直接跳到 h4** |
| `tablist` / `tabpanel` | 全站 0（任务详情五个页签是裸 `<button>`，选中态只有 CSS class） |
| `aria-current` | 全站 0（顶栏当前项只靠 CSS） |
| `document.title` | 11 条路由**完全相同**（`AgentPipeline · 像素机房`） |

外加：`▶` 从 `::before` 进了可访问名（`.btn.solid`、`.chip.on`、详情页签），
读屏会念「▶ 合入」；选择态只有 class 的地方还有对讲台班次芯片、会话 run 选择、
产出文件、手机访问地址（无 `aria-pressed`/`aria-selected`）。

**Blocked by:** None（can start immediately）

**Status:** open

- [ ] 11 条路由**每条**一个 `<main>`（看板已有的保持）
- [ ] 看板补 `<h1>`；404 补标题（视觉可不变，语义要有）
- [ ] 任务详情标题层级不断级（h1 → 下一级，不跳过 h2/h3）
- [ ] 详情页签 `role="tablist"`+`role="tab"`+`aria-selected`+`aria-controls`，面板 `role="tabpanel"`，方向键切换
- [ ] 顶栏当前项 `aria-current="page"`
- [ ] 班次芯片 / run 选择 / 产出文件 / 地址选择补 `aria-pressed` 或 `aria-selected`
- [ ] 装饰性的 `▶` 不进可访问名（`aria-hidden` 化或移出 `::before`）
- [ ] 每路由一个 `document.title`（任务详情含任务标题）
- [ ] e2e：逐路由断言 `main === 1`、看板与 404 的 `h1 !== 0`、详情 `tablist === 1`、
      顶栏当前项 `[aria-current="page"] === 1`
- [ ] e2e：`getByRole('button', {name: '合入', exact: true})` 能命中（名字里没有 `▶`）

**边界.** 这些是纯语义改动，**不要顺手改视觉**（`▶` 可以照画，只要不进可访问名）。
