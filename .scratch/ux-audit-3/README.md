# AgentPipeline 前端 UI/UX 审计（第三轮 · 复核 + 漂移面）

**Status:** done（骨架 13 条候选全部核实并回填；C 组现场追加 0 条——走查当场发现的新问题
（设置页字面星号）并入票 [08](issues/08-settings-notify-page.md) 记「回归」，未另立编号）

**日期:** 2026-10-04　**被测:** 本工作树代码 commit `1f3db53`
（二进制 `/root/.agentpipeline/shared-target/debug/agent-pipeline`，2026-10-05 11:25 构建；
内嵌 `frontend/dist/assets/index-BCWtBXQM.js`，2026-10-05 11:11 构建）
**复现:** 见下「六、复现命令」四行，逐条可跑
**证据:** 本目录 12 张 PNG + 三份 spec stdout 上逐条可引的 `[r1]/[r2]/[r3]` 数字。
本轮 spec 默认 skip（`UX_AUDIT3` 未设时 14 例全跳），不进 `make check-e2e`。
**产品代码零改动:** `git diff --stat -- frontend/src crates` 为空；变更白名单只有
`.scratch/ux-audit-3/**`、`frontend/e2e/ux-audit-3.spec.ts`、`.gitignore`（+2 行）。

---

## 〇、这一轮和第一、二轮差在哪

前两轮已经横着扫过两遍：

- **第一轮**（`.scratch/ux-audit/`，决策 195–203，27 票）审**静息态的画面与文案**——色值、措辞、IA、对比度；
- **第二轮**（`.scratch/ux-audit-2/`，决策 215–217，22 票）审**边界态**——语义/键盘、失败/断线、
  480–1240 中间档、不可逆动作与重入。

第三轮**不再新增视角**，改做两件收口的事（用户描述逐条）：

1. **复核**：把前两轮遗留的 **open** 条目拿到**当前**源码与真页面上对一次账——已修 / 未修 / 回归 / 有意不做。
  点名最小集 ux-audit-2 票 **18 / 19 / 21 / 22**，另加票 17 未做子项、第一轮 B 叠实现票兜底。
2. **漂移面**：第二轮收口（2026-09-18）**之后**才新增的界面（`SettingsTools` / `SettingsNotify` /
   `SettingsForeman` 三页、看板道具栏、决策 240 wordmark 去链、决策 243/300 窄档底栏与状态条、
   决策 215/218 折行档）——前两轮的地图没覆盖它们。**以当前源码/真页面为准**，不以旧报告描述为准。

一句话与上两轮的差别：**第一轮看「静下来什么样」，第二轮看「坏起来什么样」，
第三轮看「修过的东西现在还有效吗，新长的东西长歪了没有」。**

---

## 一、口径（读这份报告前先认这三条）

### 1. 证据等级三档（沿用第二轮，不冒充实测）

- **`实测`**：真二进制 + 真后端 + 真浏览器上量到的数字（附 `[r3]` stdout 行或截图名）；
- **`代码`**：读源码到 `文件:行号` 可推、未在浏览器里跑；
- **`未验证`**：有意留待（harness 造不出该状态，或本轮没走到）。**跑不动的场景一律标 `未验证`，不硬凑成 `实测`。**

### 2. `wontfix` 条目的特别规则

票 **18 / 21 / 22 余项**是 2026-10-01 经用户**裁决收掉**的（`wontfix`），票 **19** 是 `superseded`。
本轮对这些条目**只记「现状核实」**，不重开（R3）；结论档一律落 **`有意不做`**（设计 §4 特别规则：
「wontfix 收掉的按『有意不做』另记，**不算未修**」），**只有现状比 wontfix 时更坏**才按「回归」新开。
判据：第二轮取证点（`[r2]` 数字）与本轮（`[r3]` 数字）逐格对比——同值即「未回归」。

### 3. 票号不一致按并集处理（一处需正面处理）

`design.md` 概述第 1 条写的点名最小集是「票 18/19/22」，而 AC-3 写的是「票 18/21/22」
（概述里的编号少写了一位，「破坏性确认」实为票 21）。本方案取**并集**：
**18、19、21、22 四条逐条给结论**；超出 AC-3 最小集的 19 本就是 `superseded`，复核即一条现状记录。

---

## 二、逐条清单（13 条候选，全部已核实）

| 编号 | 标题 | 与前轮关联 | 证据等级 | 状态 |
|---|---|---|---|---|
| [01](issues/01-detail-midband-fold-status.md) | 详情页 480–819px 中间档主栏折行与 hero 横向溢出 | 现状核实（=18，wontfix） | 实测 + 代码 | **有意不做**（wontfix 维持，无回归） |
| [02](issues/02-talk-midband-fold-status.md) | 对讲台 480–899px 中间档对话列折行 | 现状核实（=19，superseded） | 实测 + 代码 | **已修** |
| [03](issues/03-destructive-confirm-status.md) | 不可逆动作单击即发、无确认步、量级反着来 | 现状核实（=21，wontfix） | 实测 + 代码（一处未验证） | **有意不做**（wontfix 维持，无回归） |
| [04](issues/04-midflow-persistence-status.md) | 中流状态持久化余三件（`?tab=` / `?filter=` / `talk_draft`） | 现状核实（=22，wontfix 余项） | 实测 + 代码 | **有意不做**（wontfix 余三件维持） |
| [05](issues/05-toast-keyboard-close-status.md) | toast 的键盘关闭路径（票 17 明写「没做」的子项） | 现状核实（=17 子项） | 代码（运行时未验证） | **部分已修**（有关闭钮、缺 Escape） |
| [06](issues/06-round1-bstack-status.md) | 第一轮 B 叠实现票是否真落（决策 201 类历史坑） | 现状核实（=第一轮 B 叠） | 代码 | **已修**（无悬空） |
| [07](issues/07-settings-tools-page.md) | 命令执行设置页（漂移面）可达性与语义 | 新开 | 实测 + 代码 | **新开（无缺陷）** |
| [08](issues/08-settings-notify-page.md) | 离线通知设置页（漂移面）——兼字面星号回归 | 回归（票 15 ③ 同型）+ 新开 | 实测 + 代码 | **回归** |
| [09](issues/09-settings-foreman-page.md) | 值守轮设置页（漂移面）可达性与语义 | 新开 | 实测 + 代码 | **新开（无缺陷）** |
| [10](issues/10-board-props-bar.md) | 看板顶部道具栏（过滤槽 / 待处理 / 新建）语义与计数 | 新开 | 实测 + 代码 | **新开（无缺陷）** |
| [11](issues/11-wordmark-and-entry-consolidation.md) | 决策 240 入口归一：wordmark 去链、看板回根路由 | 新开 | 实测 + 代码 | **已修** |
| [12](issues/12-narrow-dock-and-statusbar.md) | 决策 243/300 窄档底栏与状态条（≤479px 状态条退场） | 新开 | 实测 + 代码 | **已修** |
| [13](issues/13-fold-band-215-218.md) | 决策 215/218 折行档版面（详情页 820–1099 / 对讲台 900–1099） | 新开 | 实测 + 代码 | **对讲台侧已修 / 详情页侧未落** |

> C 组（`14` 起）**本轮 0 条**：走查当场看到的唯一新问题（设置页字面 `**` 星号）与票
> [08](issues/08-settings-notify-page.md) 同源，已并入该票记「回归」，不另立编号（R1：只证实/证伪，不扩挖）。

---

## 三、四条点名复核的结论（AC-3）

| 前轮票 | 本轮结论 | 落点 |
|---|---|---|
| 18 详情页中间档折行 | **有意不做**（2026-10-01 用户裁决收掉，与 wontfix 一致，无回归） | `[r3] ①.1` 与 `[r2] ①.4` 逐格同值（820→12、768→64、600→232、520→312、480→352）；`TaskDetail.svelte:662-666,948` |
| 19 对讲台中间档折行 | **已修**（superseded 后确实落地） | `[r3] ①.2`：900→`562px 280px`、820 起单列；`Talk.svelte:2353,3149-3153,3166` |
| 21 破坏性动作确认步 | **有意不做**（2026-10-01 用户裁决收掉，与 wontfix 一致，无回归） | `[r3] ②.1`：第一次点「合入」直接提交（按钮区整块消失），无确认步 |
| 22 中流状态持久化 | **有意不做**（wontfix 余三件维持，已落两处底座未回退） | `[r3] ③.1–③.3`：页签 `/` 过滤 `/` 草稿刷新后均不存活；`TaskDetail.svelte:40`、`board.svelte.ts:37,168`、`Talk.svelte:1089` |

---

## 四、漂移面结论（B 组）

- **设置三页**（tools / notify / foreman）：可达、单一 `<main>`、恰一个 `<h1>` + 若干 `<h2 class="sec-title">`、
  无横向溢出（`[r3] ④.1`）。离线通知页的通道是**真 `radiogroup`**（`role=radio` + `aria-checked`，`④.2`）；
  值守轮的「节奏」是**只读行**，不伪装成控件——语义诚实。
- **看板道具栏**：7 个过滤槽带 `aria-pressed` + 图标 + 词 + 计数，第 3 槽（`pending`）**不画重复数**，
  数由「待处理 N」芯片唯一承载（`[r3] ④.3`）——决策 201 的形态在漂移面之后仍在。
- **决策 240 入口归一**：`.wordmark` 是 `<span>`（非链接），页面导航行**恰四项**含看板（`[r3] ④.4`）——已修。
- **决策 243/300 窄档**：430 视口下 `footer.statusline` `display:none`、底部页签栏 `display:flex h:58`，
  `--sbar-h = calc(58px + 0px)`（收成单层）（`[r3] ④.5`）——已修。
- **决策 215/218 折行档**：对讲台侧两档（899/1099）源码与实测均成立；详情页侧**未落** 280/单列两档
  （与票 01 同源）。

---

## 五、明确**未验证** / 有意留待

照 R4/R5：以下三条**不冒充实测**，已在对应票内标 `未验证`：

1. **`终止任务` 按钮量级**（票 [03](issues/03-destructive-confirm-status.md)）：本 harness 造出的
   `merge_approval` 态下该按钮**不存在**（`[r3] ②.2 {"found":false}`）——它属 `retry_exhausted` /
   `dependency_failed` 态的旁路动作，故红描边色未在真页面量到，结论来自源码（`crates/core/src/actions.rs:229,243,252,272,278`）。
2. **toast 的关闭钮 / Escape 运行时行为**（票 [05](issues/05-toast-keyboard-close-status.md)）：
   `[r3] ⑤.1 {"count":0,"closeBtns":0}`——本轮那一刻 harness **没弹出任何 toast**，关闭钮与 Escape
   只在源码层确认（`ToastStack.svelte:32-36`；全文件 `grep Escape` = 0）。
3. **非看板路由的 `--topbar-h` 取 0 那一档**（票 [12](issues/12-narrow-dock-and-statusbar.md)）：
   窄档探针只走了看板路由 `#/`（`--topbar-h = 52px`），非看板路由的 0 值未在真页面量到。

另：本轮**未**审桌面壳（Tauri/dmg）、手机真机浏览器（Safari / Chrome for Android 安全区）、
真实 provider 长任务体验（harness 全 mock LLM）——与前两轮同一留待口径。

---

## 六、复现命令（逐条可跑）

```sh
# 1) 先保证二进制内嵌的是当前工作树（本轮一次性构建，冷编约十几分钟）
bash scripts/e2e-artifacts.sh

# 2) 复跑前两轮的取证 spec（回归的直接判据）
cd frontend && UX_AUDIT=1  npx playwright test --project=chromium e2e/ux-audit.spec.ts
cd frontend && UX_AUDIT2=1 npx playwright test --project=chromium e2e/ux-audit-2.spec.ts

# 3) 跑本轮 spec（复核 + 漂移面）；不设开关时 14 例全 skipped
cd frontend && UX_AUDIT3=1 npx playwright test --project=chromium e2e/ux-audit-3.spec.ts --reporter=line
```

本轮实跑结果（2026-10-05）：`ux-audit.spec.ts` **13 passed (1.5m)**、
`ux-audit-2.spec.ts` **12 passed (3.1m)**、`ux-audit-3.spec.ts` **14 passed (53.0s)**；
`--list` 收集 14 例、退出码 0（`skipped (14)` 在未设开关时）。

---

## 七、票

一票一文件在 `issues/`，编号即依赖序（骨架先落、逐条回填）。全部沿用**叠 A**（不动规格的纯核实/修正票）。
候选池来源见 [00-INDEX.md](issues/00-INDEX.md)。

| # | 票 | 叠 | 结论 |
|---|---|---|---|
| [01](issues/01-detail-midband-fold-status.md) | 详情页中间档折行现状核实 | A | 有意不做（wontfix 维持） |
| [02](issues/02-talk-midband-fold-status.md) | 对讲台中间档折行现状核实 | A | 已修 |
| [03](issues/03-destructive-confirm-status.md) | 不可逆动作确认步现状核实 | A | 有意不做（wontfix 维持） |
| [04](issues/04-midflow-persistence-status.md) | 中流状态持久化余三件现状核实 | A | 有意不做（余项维持） |
| [05](issues/05-toast-keyboard-close-status.md) | toast 键盘关闭路径现状核实 | A | 部分已修 |
| [06](issues/06-round1-bstack-status.md) | 第一轮 B 叠实现票是否真落 | A | 已修（无悬空） |
| [07](issues/07-settings-tools-page.md) | 命令执行设置页（漂移面） | A | 新开（无缺陷） |
| [08](issues/08-settings-notify-page.md) | 离线通知设置页（漂移面）+ 字面星号回归 | A | 回归 |
| [09](issues/09-settings-foreman-page.md) | 值守轮设置页（漂移面） | A | 新开（无缺陷） |
| [10](issues/10-board-props-bar.md) | 看板顶部道具栏语义与计数 | A | 新开（无缺陷） |
| [11](issues/11-wordmark-and-entry-consolidation.md) | 决策 240 入口归一 | A | 已修 |
| [12](issues/12-narrow-dock-and-statusbar.md) | 决策 243/300 窄档底栏与状态条 | A | 已修 |
| [13](issues/13-fold-band-215-218.md) | 决策 215/218 折行档版面 | A | 对讲台侧已修 / 详情页侧未落 |

**本批落实的唯一「建议」**（其余是现状核实或「无缺陷」）：票 [08](issues/08-settings-notify-page.md) 的字面星号——
把面向用户的 `**…**` 改成真的强调元素（`<b>`）或走一处 Markdown 渲染管道。
**审计票不实现**（AC-7 / R2）：本轮对产品代码零改动。
