# IMPLEMENTATION · ux-audit-3 的 13 条票落实与复核记录

**日期:** 2026-10-06　**落树:** worktree `01M47RQG4M9533F5TMF1AGJXC8`（基于 HEAD `82dfdf1`）
**三档处理:** 动代码 3 票（05 / 08 / 13）；复核即关 7 票（02 / 06 / 11 / 12 已修，07 / 09 / 10 新开无缺陷）；
不改代码 3 票（01 / 03 / 04，wontfix 维持、不重开）。

**冻结文件（一字未改，`git status` 可证）:** `frontend/e2e/ux-audit-3.spec.ts`、
`issues/*`（13 张票面 + 00-INDEX）、`README.md`——只复跑、不修改。

**复跑命令与结果:**

```sh
bash scripts/e2e-artifacts.sh     # dist 已最新 → cargo 增量重编被测二进制
cd frontend && UX_AUDIT3=1 AGENTPIPELINE_E2E_BIN=/root/.agentpipeline/shared-target/debug/agent-pipeline \
  npx playwright test --project=chromium e2e/ux-audit-3.spec.ts --reporter=line
# → 14 passed (53.2s)，exit 0；31 行 [r3] 探针 stdout 全数留档于下文
```

> 环境注：本机 `CARGO_TARGET_DIR=/root/.agentpipeline/shared-target`，harness 默认找
> worktree 内 `target/debug/`，故经其自带出口 `AGENTPIPELINE_E2E_BIN` 指向共享目录的
> 当前构建（内嵌本轮 `npm run build` 的 dist）。

**闸门汇总（全绿）:**

| 闸门 | 结果 |
|---|---|
| `make check-lint`（fmt + clippy） | 通过（Rust 零改动） |
| `cd frontend && npm test` | 84 文件 / **1171 例全通过**（含新增 ToastStack.test.ts 6 例、copy-discipline 新 4 例） |
| `cd frontend && npm run check` | 0 errors / 0 warnings |
| `cd frontend && npm run build` | 通过 |
| e2e 窄跑 `ux2-geometry.spec.ts` + `ux2-resilience.spec.ts` | **11 passed / 0 failed**（两条新用例定点复跑亦 PASS） |
| 复跑 `ux-audit-3.spec.ts`（证据，非门） | **14 passed** |

---

## 逐票记录（一票一节：结论 + 依据 + 差异）

### 票 01 · 详情页 480–819 中间档折行与 hero 溢出 —— **维持 wontfix（有意不做），无回归**

- **依据（复跑 ①.1 逐格同值）:** 溢出列 `820→12、768→64、600→232、520→312、480→352`
  与票面完全同值；hero 轨道 `scrollWidth=812 / overflowX=visible` 同值；480 档
  `main=102`（票面记的 102px 洞）同值；`768/600/520/480` 列串仍 `…px 320px` 同值。
- **截图:** `r3-detail-768.png`、`r3-detail-480.png`（复跑现生成，与票面同探针）。
- **差异:** 唯一有意变化是 `900 / 820` 两格的 grid 列串（归票 13，见文末差异清单）；
  wontfix 面的每一格证据原样保住——`min-width: 820` 钉下界 + `ux2-geometry` 新用例
  断言 `819 → 320px` 双保险。

### 票 02 · 对讲台中间档折行（已修） —— **复核即关**

- **依据（复跑 ①.2 与票面同值）:** `1100 → 702px 340px`、`900 → 562px 280px`、
  `820 起 → minmax(420px, 1fr) 280px`（sideW=0，单列）逐格同值；**14 passed**。
- **截图:** `r3-talk-768.png`、`r3-talk-480.png`。
- **差异:** 无（对讲台侧本轮零代码改动）。

### 票 03 · 破坏性动作无确认步（wontfix） —— **维持 wontfix，不重开**

- **依据（复跑 ②.1 / ②.2）:** 点击前 `bodyHasConfirm=false`（按钮 `返回修改 / 合入`）；
  **第一次点击后** `bodyHasConfirm=false、confirmTexts=[]、buttons=[]`——仍直接提交、
  无确认步，与票面同值；`②.2 终止任务按钮 found=false`（票面即为「量级一处未验证」，
  现状未变，不据此重开）。
- **截图:** `r3-merge-first-click.png`。
- **差异:** 无。

### 票 04 · 中流状态持久化余三件（wontfix） —— **维持 wontfix，不重开**

- **依据（复跑 ③.1–③.3 全部不存活，与票面同值）:**
  ③.1 页签 `Diff → 时间线`（hash 同值）；③.2 过滤 `已完成 0 → 全部 1`（hash 同值）；
  ③.3 草稿 `survived=false、afterValue=""`；三处 localStorage 键集合均同值
  （`agentpipeline.theme` / `agentpipeline.talk_seen`，无 `talk_draft` 键出现）。
- **截图:** `r3-tab-after-reload.png`、`r3-board-filter-after-reload.png`、
  `r3-talk-draft-after-reload.png`。
- **差异:** 无。

### 票 05 · toast 的 Escape 键盘关闭路径 —— **关（动代码）**

- **改动（只动 `ToastStack.svelte` 一个组件）:**
  - `frontend/src/components/layout/ToastStack.svelte:13-26` — `stack` 容器引用 +
    `handleKey` 焦点归属守卫（`Escape` 且有 toast → 焦点须在 `.toasts` 内 →
    `closest('[data-toast-id]')` 取整数 id → `preventDefault` + `dismiss`）；
  - `:29` — `<svelte:window onkeydown=*** />`（本仓既有形状，
    `Modal.svelte:129` / `TopBar.svelte:145` 同款，避开静态元素 a11y lint）；
  - `:35` — `.toasts` 容器 `bind:this={stack}`（归属判据，不靠 class 名猜）；
  - `:41` — 每条 toast `data-toast-id={toast.id}`（单测/e2e 稳定定位器）。
  - **不做:** 全局 Escape、`stopPropagation`、任何 store / 全局状态改动——与
    `Modal`（open 才关）、`menuTrap`（开着才关）、决策 216④ 同源，焦点不在 toast 里
    则一个字节不动（设计 §0 裁决 ②）。
- **依据（测试三层）:**
  - 单测 `frontend/src/components/layout/ToastStack.test.ts` **6 例全绿**（焦点在关闭钮
    Escape 关 / 焦点在 body 不动 / 两条只关焦点那条 / 无 toast 不抛 / 关闭钮点击回归 /
    Enter 不误伤）；
  - e2e `frontend/e2e/ux2-resilience.spec.ts:271-279`（⑤ 尾部）真页面
    聚焦关闭钮 → Escape → `.toast[data-toast-id]` count 0，**定点复跑 PASS**——
    正好闭合票面与 README 标的「运行时未验证」（`[r3] ⑤.1 count=0`）；
  - 复跑 ⑤.1 `{"count":0,"closeBtns":0}` 与票面同值（harness 那一刻仍无 toast，
    探针读数本身不变）。
- **行为不变面:** 关闭钮点击、Tab+Enter/Space、悬停/聚焦暂停、TTL 自动消解——全部未动
  （`npm test` 全量 1171 例通过即其回归证据）。
- **差异:** 见文末 ⑤ 结论标注一条。

### 票 06 · 第一轮 B 叠实现票是否真落（已修） —— **复核即关**

- **依据:** 四文件在且其用例在 `npm test` 全绿——`frontend/src/theme/contrast.test.ts`、
  `frontend/src/lib/pipeline.geometry.test.ts`、`frontend/src/lib/copy-discipline.test.ts`
  （本轮另扩了新规则，见票 08）、`frontend/src/lib/behavior-map.test.ts`；
  复跑 ④.3 七槽形态与票面同值（`slotCount=7`、第 3 槽「待处理」`hasCount=null` 不画数）。
- **差异:** 无。

### 票 07 · 命令执行设置页（新开无缺陷） —— **记录即关**

- **依据（复跑 ④.1 tools 与票面同值）:** `main=1`、恰一个 `h1`（`设置 · 命令执行`）、
  `tablist=0、roleAlert=0、hScroll=0`，title 同值。
- **差异:** 无（本页唯一改动是票 08 的 160 行字面星号 → `<b>`，语义探针不受影响，
  复跑同值即证）。

### 票 08 · 离线通知设置页字面 `**…**` 星号 —— **关（动代码）**

- **改动（四处一并修，形态照本页既有 `<b>` 先例，不引 Markdown 管道）:**
  | 位置 | 改后 |
  |---|---|
  | `SettingsNotify.svelte:577` | `保存的是<b>整体覆盖</b>：四件…` |
  | `SettingsNotify.svelte:589` | `…（iOS 走 APNs），<b>同一条通知每台订阅设备各收一份</b>；` |
  | `SettingsNotify.svelte:676` | `清单只显示 endpoint 的<b>摘要</b>（…）；` |
  | `SettingsTools.svelte:160` | `闸门命令（跑测试 / lint）<b>不改写</b>——…` |
- **机器门（防回潮）:** `frontend/src/lib/copy-discipline.test.ts:72-96` 新增
  `LITERAL_BOLD = /\*\*[^*\n]+\*\*/g` + `findLiteralStars`（剥注释、行号可对回原文）；
  `:173-211` 新 describe 四例——全站扫描 `hits === []` / 注释与测试文件边界沿用 199
  （SettingsNotify 头注释仍有 `**` 但 0 命中的实证）/ `***` 掩码不计（⑤.2 订正）/
  正例带行号与 snippet。**四例全绿。**
- **依据（复跑 ⑤.2）:** 离线通知页 `{"count":0,"hits":[]}`、命令执行页
  `{"count":0,"hits":[]}`——两页归零，与票面修前（4 处）对照成立。
- **截图:** `r3-literal-asterisks.png`（复跑现生成，**修后**版本）、`r3-settings-notify.png`。
- **差异:** 修前截图备份未能落，见文末环境差异。

### 票 09 · 值守轮设置页（新开无缺陷） —— **记录即关**

- **依据（复跑 ④.1 foreman 与票面同值）:** `main=1`、恰一个 `h1`（`设置 · 值守轮`）、
  `tablist=0、roleAlert=0、hScroll=0`。
- **差异:** 无。

### 票 10 · 看板顶部道具栏（新开无缺陷） —— **记录即关**

- **依据（复跑 ④.3 与票面同值）:** 7 槽、第 3 槽「待处理」`hasCount=null` 不画数、
  `pendingChip="待处理 1"`、首槽 `pressed=true`，逐格同值。
- **差异:** 无。

### 票 11 · 决策 240 入口归一 wordmark 去链（已修） —— **验无回归后关**

- **依据（复跑 ④.4 与票面同值）:** `{"tag":"SPAN","isLink":false,"href":null,
  "text":"AGENTPIPELINE","navItems":["对讲台","看板","指标","设置"]}`——SPAN 不可导航、
  导航恰四项。
- **差异:** 无。

### 票 12 · 决策 243/300 窄档底栏与状态条（已修） —— **验无回归后关**

- **依据（复跑 ④.5 与票面同值）:** 430 宽下 `statusline display:none、h=0`、
  底栏 `display:flex、h=58`、`--sbar-h = calc(58px + 0px)`、`--topbar-h = 52px`。
- **截图:** `r3-narrow-bottom-430.png`。
- **差异:** 无。

### 票 13 · 决策 215/218 折行档（详情页 820–1099 档未落） —— **关（动代码，仅详情页侧本档）**

- **改动（只此一段，紧接 `.detail.split` 基础规则之后）:**
  `frontend/src/routes/TaskDetail.svelte:669-681` —
  `@media (min-width: 820px) and (max-width: 1099px) { .detail.split {
  grid-template-columns: minmax(480px, 1fr) 280px; } }`，注释写明决策 215 三档表、
  算式 `820 − 40 − 18 − 280 = 482 ≥ 480`、以及 `min-width: 820` 是为钉住票 01 面。
- **不碰面（逐条零改动）:** `<479` 移动款 `display:block`、480–819 档、hero 轨道
  `overflow-x`、`.detail.split { max-width: 1240px }`、gap/padding——`git diff` 只有
  上述一段插入。
- **依据:**
  - 复跑 ①.1：`900 → 562px 280px`（main 562）、`820 → 482px 280px`（main 482 ≥ 480）、
    `1100 → 722px 320px` 桌面档不变、`768/600/520/480` 仍 `… 320px` 不变；
    溢出列逐格同值（820 仍 12，hero 的事一分没动）；
  - e2e `frontend/e2e/ux2-geometry.spec.ts:204-251` 新用例：`1099/1024/900/820 →
    末列 280px 且 .main ≥ 480`；`1100 / 819 → 320px`（钉住「只动本档」），
    **PASS**；不断言 820 处横向溢出 = 0（hero 12px 属票 01 wontfix，断它等于逼重开）。
- **差异:** 就是文末那两格有意变化本身。

---

## 差异清单

### 有意变了（全部归票，且仅这些）

| 格子 | 修前（票面/前轮） | 复跑（修后） | 归属 |
|---|---|---|---|
| ①.1 详情 w=900 列串 | `522px 320px` | `562px 280px` | 票 13 |
| ①.1 详情 w=820 列串 | `442px 320px` | `482px 280px` | 票 13 |
| ⑤.2 两页字面星号 | 修前 4 处（`r3-literal-asterisks.png` 原图） | `count=0, hits=[]` | 票 08 |
| ⑤.1 结论标注 | Escape「运行时未验证」 | 由 `ToastStack.test.ts` + `ux2-resilience` ⑤ 尾部闭合（探针读数仍 `count=0`，同值） | 票 05 |

### 必须不变（复跑逐格核对，全部同值 ✓）

- ①.1 溢出列 `820→12、768→64、600→232、520→312、480→352`；`768/600/520/480` 列串
  仍 `… 320px`；480 档 `main=102`；hero `812 / visible`；1100 档 `722px 320px`。
- ①.2 对讲台全部格子（`1100→702px 340px`、`900→562px 280px`、`820 起单列`）。
- ②.1/②.2、③.1–③.3、④.1–④.5 全部探针与票面同值（见逐票依据）。
- 819 及以下仍读 `320px`（`ux2-geometry` 新用例断言，票 01 面一格不动）。

### 环境差异（如实记录，不掩盖）

1. **修前截图备份无源文件:** `.scratch/ux-audit-3/*.png` 被 gitignore（`*.png` 规则
   `git check-ignore` 已核），审计原跑截图未随库——本工作树复跑前**不存在**
   `r3-literal-asterisks.png`，故 `cp …pre-fix.png` 一步无源可备。本次复跑生成的
   `r3-literal-asterisks.png` 是**修后**取证；修前形态的证据保留在票面 08 的判据记录
   （4 处行号）与本次 `git diff` 里。
2. **二进制路径:** harness 默认找 `<repo>/target/debug/agent-pipeline`，本机
   `CARGO_TARGET_DIR` 指向共享目录——经 `AGENTPIPELINE_E2E_BIN` 出口指向共享目录的
   当前构建（`scripts/e2e-artifacts.sh` 本轮现编，内嵌现 build 的 dist，新鲜度由脚本
   自己打印的判定证明）。
3. **行号漂移:** 票面行号对 `82dfdf1` 前的工作树仍准确（577/589/676/160、
   `TaskDetail:662-666`、`ToastStack:31-36` 均实读吻合），改动后的新行号见各票
   「改动」小节——改动前后两套行号都能反查回票面。

### 闸门与移交

- 本地闸门（lint / vitest / svelte-check / build / 目标 e2e 窄跑）全绿，见文首汇总表。
- 全量 `make check` / `check-e2e` 按决策 331 交 CI。
- 提交按票切四段：票 05 / 票 08 / 票 13 / 本记录文件，message 带「票 NN（ux-audit-3）」。
