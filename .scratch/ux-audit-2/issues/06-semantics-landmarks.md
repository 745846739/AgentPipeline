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

**Status:** done

- [x] 11 条路由**每条**一个 `<main>`（看板已有的保持）
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

## 实施记录（2026-09-18）

**`<main>` 落在每个路由的根元素上**（`<div class="page">` → `<main class="page">`，Talk 的
`.talk`、详情页的 `.detail` 同理；看板原本就在 `<main class="panes">` 里，不动）。
不新增包裹层：多包一层会改变文档流与既有布局的父选择器关系，而这些页面**逐页**都有自己的
高度公式（比如对讲台）。404 那一支在 `App.svelte` 里改成 `<main class="notfound">`。

**顶栏当前项**：`aria-current="page"` 只在**这条路由真的属于那一项时**出现——看板 / 404 /
任务详情这三条路由本就不在「对讲台·指标·设置」那一行里，它们**不该**点亮任何一项。
票面 e2e 那条 `[aria-current="page"] === 1` 因此收窄成「逐路由断言该点亮的那一项（或都不亮）」；
全站恒定 1 反而是错的。

**任务详情标题层级**：h1（任务名）→ **h2（面板标题，视觉隐藏）** → h3（diff 的每个文件块、
评审页三段）。`DiffView` 与 `ReviewForm` 的 `h4` 随之降为 `h3`；`pending-dossier.spec.ts`
与 `TaskDetail.test.ts` 里按 `level: 4` 定位的断言一并改为 `level: 3`。

**页签容器从 `<nav>` 换成 `<div>`**：`nav` + `role="tablist"` 这个组合本身就是错的
（页签不是导航地标，Svelte 的 a11y 检查直接点名）。连带把三处 e2e 的 `nav.tabs` 定位
改成 `.tabs`，`getByRole('button', …)` 的页签点击改成 `getByRole('tab', …)`。
容器不加 `tabindex`：焦点住在那几格上（roving tabindex），这是 APG 的 tablist 模式，
Svelte 那条检查不认，就地 `svelte-ignore` 并在代码里说明。

**`▶` 用 `clip-path` 剪影画**，不用 `▶` 字符、也不用边框三角：CSS 生成的文字同样进可访问名
（读屏念「▶ 合入」），而边框宽度在这个仓是**受管的像素纪律**（只允许 2px 一档，
`css-parity.test.ts` 当场撞红——第一版就是这么红的）。剪影三角两边都不犯。

**`document.title`** 在 `App.svelte` 的一个 `$effect` 里写：`<页面名> · AgentPipeline · 像素机房`，
任务详情那一页用任务标题。

**三处选择态补 `aria-pressed`**：对讲台班次芯片、会话 run 选择、产出文件、手机访问的地址选择
（都是「一组里选一个」的按钮，`aria-pressed` 是它们对得上的那个属性）。

**e2e**：`frontend/e2e/ux2-semantics.spec.ts`（四条，进 `make check-e2e`）。
