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

### 票 01 · 详情页 480–819 中间档折行与 hero 溢出 —— **已落地（2026-10-01 用户裁决推翻 wontfix）**

- **落码（2026-10-01 本轮）:**
  - `frontend/src/components/pipeline/PipelineRail.svelte:340-350` — `.rail.hero` 加
    `overflow-x: auto`：**容器内横滚**，不裁切（站点坐标写死在 `lib/pipeline.ts`，
    裁掉等于「后面的工位不存在」）、不传给文档；`.rail.spine` 的裁切语义一字不动（决策 215③）。
  - `frontend/src/routes/TaskDetail.svelte:725-728` — `@media (min-width: 480px) and
    (max-width: 819px) { .detail.split { display: block } }`：主栏拿满容器宽（原
    `minmax(0,1fr) 320px` 在 480px 只剩 102px 那个洞），档案盒按 DOM 顺序落到主栏下方，
    sticky 与动作行照旧（决策 215①「`<820px` 折成一列」）；≤479 的移动款形态一字不动。
- **审计当轮读数（原证保留）:** 溢出列 `820→12、768→64、600→232、520→312、480→352`
  与票面完全同值；hero 轨道 `scrollWidth=812 / overflowX=visible` 同值；480 档
  `main=102`（票面记的 102px 洞）同值。
- **截图:** `r3-detail-768.png`、`r3-detail-480.png`（复跑现生成，与票面同探针）。
- **差异:** 票面（`.scratch/ux-audit-3/issues/01-*.md`）冻结在审计当轮的「有意不做」，
  一字未改（`FROZEN` 守着），本节是落地记档；`ux2-geometry.spec.ts` 的
  「819 仍是 320px」双保险随之改为断言 819 走单列档。

### 票 02 · 对讲台中间档折行（已修） —— **复核即关**

- **依据（复跑 ①.2 与票面同值）:** `1100 → 702px 340px`、`900 → 562px 280px`、
  `820 起 → minmax(420px, 1fr) 280px`（sideW=0，单列）逐格同值；**14 passed**。
- **截图:** `r3-talk-768.png`、`r3-talk-480.png`。
- **差异:** 无（对讲台侧本轮零代码改动）。

### 票 03 · 破坏性动作无确认步 —— **已落地（2026-10-01 用户裁决推翻 wontfix）**

- **落码（2026-10-01 本轮）:**
  - `frontend/src/lib/actions.ts:117` — `actionTier(action, pendingType)` 四档纯函数
    （`advance` / `gate-skip` / `destructive` / `quiet`，按判据不靠标签文字匹配）+
    `confirmSentence()` 后果句逐条（决策 216①③：`确认合入到 …？` / `确认终止？…` /
    `确认重置？…` / `确认跳过评审闸门？`）。判据张力按①与⑥末句互斥律收口：`merge` 归
    `destructive`（不取⑥「推进=实心」的举例）。
  - `frontend/src/components/board/PendingActions.svelte` — 三档量级类（`btn solid` /
    `btn gate` / `btn danger` / `btn quiet`，量级样式全站 `app.css` 一处）+ 内联两步确认
    （第一颗只亮后果句、同一颗再点才提交）、Escape 退回、焦点不移动、动作集换了清确认态
    （决策 216②④⑥）。
  - `frontend/src/components/task/DiffReviewPanel.svelte` — 合入 approve 走同口径两步确认
    （`destructive` 红描边 + 后果句），`return` 落 `quiet`；「合入后 push」随第二颗提交。
  - `frontend/src/components/task/ReviewForm.svelte` — 打回落 `quiet`，通过为 `advance`
    实心无确认步。
- **审计当轮读数（原证保留）:** 点击前 `bodyHasConfirm=false`；**第一次点击后**仍直接提交、
  无确认步——与票面同值（当轮为「有意不做」）；`②.2 终止任务按钮 found=false` 同值。
- **截图:** `r3-merge-first-click.png`。
- **差异:** 票面（`.scratch/ux-audit-3/issues/03-*.md`）冻结在审计当轮的「有意不做」，
  一字未改（`FROZEN` 守着），本节是落地记档；`终止任务` 的红描边量级由 `.btn.danger` 给出
  （原「量级一处未验证」随本轮补上）。

### 票 04 · 中流状态持久化余三件 —— **已落地（2026-10-01 用户裁决推翻 wontfix）**

- **落码（2026-10-01 本轮）:**
  - **详情页签 `?tab=`** — `frontend/src/routes/TaskDetail.svelte:44-75`：`tabFromQuery()`
    从地址读（枚举外回落 `timeline`）、`userTab()` 走 `pushState`（用户点页签 / 方向键）、
    `programTab()` 走 `replaceState`（深链 `?run=` 直达现场、档案盒「去看对话」、打开产出文件）、
    另有一个 `$effect` 让后退 / 前进照地址恢复并抹掉脏值；缺省 `timeline` 不写进地址（决策 217②③④）。
  - **看板过滤 `?filter=` + `agentpipeline.board_filter`** — `frontend/src/stores/board.svelte.ts:34-80`
    （`FILTER_KEY` / `isStatusFilter` / `storedFilter` / `saveFilter` / `initialFilter()`：
    地址 → 本地 → 缺省，脏值就地删键）与 `:219-250`（`setFilter` 写地址 + 本地，
    `syncFilterFromQuery` 供后退 / 前进照地址恢复且不回写地址）；接线在
    `frontend/src/routes/Board.svelte` 的 `$effect`（决策 217①④⑤）。
  - **输入草稿 `agentpipeline.talk_draft`** — 新文件 `frontend/src/lib/talkDraft.ts`
    （JSON `{sessionId, text, at}`、7 天过期、形状不对 / JSON 坏就地删键，存储不可用不抛）
    + 单测 `frontend/src/lib/talkDraft.test.ts`（恢复 / 清零 / 7 天过期 / 脏键 / 存储不可用七例）；
    `frontend/src/routes/Talk.svelte` 装载回填（只认当前这班、框里有字不覆盖）、打字即存、
    清空即删——`send()` 清空输入那一趟当场把键删掉，失败回填的那句跟着下一次敲击存回去（决策 217⑤）。
- **审计当轮读数（原证保留）:** ③.1 页签 `Diff → 时间线`（hash 同值）；③.2 过滤
  `已完成 0 → 全部 1`（hash 同值）；③.3 草稿 `survived=false、afterValue=""`；三处
  localStorage 键集合均无 `talk_draft`。
- **截图:** `r3-tab-after-reload.png`、`r3-board-filter-after-reload.png`、
  `r3-talk-draft-after-reload.png`。
- **差异:** 票面（`.scratch/ux-audit-3/issues/04-*.md`）冻结在审计当轮的「未落 / 有意不做」，
  一字未改（`FROZEN` 守着），本节是落地记档。

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
    （819 那一格随后被票 01 落地（2026-10-01 裁决）改成「转单列」，见上方票 01 节；
    2026-10-08 起该格量 `display: block` 而非栅格轨道——`display:block` 不会把
    `grid-template-columns` 的 computed 值清成 `none`，量轨道是量错了仪器。）
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

## 落地形状读数（一次性验收，锚在「落地那次的分支 diff」）

这组读数判的是**审计那一轮的分支 diff 形状**。审计合入 main 之后已不存在任何检出能重现
它，故从常驻闸门套件摘出为 `#[ignore]`，读数留档于此（判据定位见 `docs/testing.md` §8）：

- `tests/e2e/tests/integration/ux_audit3_landing.rs::landing_shape_readings_once`
- `tests/e2e/tests/integration/ux_audit3_artifacts.rs::artifact_shape_readings_once`

| 判据 | 读数 | 出处 |
|---|---|---|
| 场景 7：SettingsNotify 恰改 3 行 | `(3, 3)` | 落地提交 `d7bfcc9`（票 08） |
| 场景 7：SettingsTools 恰改 1 行 | `(1, 1)` | 落地提交 `d7bfcc9`（票 08） |
| 场景 11：TaskDetail 只增 13 行、0 删除 | `(13, 0)` | 落地提交 `116745b`（票 13） |
| 场景 17①：改动面白名单 | `frontend/src/` ∪ `frontend/e2e/` ∪ `.scratch/ux-audit-3/IMPLEMENTATION.md` ∪ `tests/e2e/tests/integration/` | 落地那一段 |
| 场景 17④：四段 message 反查 | 含「票 05 / 08 / 13（ux-audit-3）」 | 落地那一段 |
| 场景 6：产品代码（`frontend/src` + `crates`）零 diff | 空 | 审计轮 |
| 场景 5：`docs/decisions.md` 本轮未动 | 空 | 审计轮 |
| 场景 9：前两轮 spec 本轮未动 | 空 | 审计轮 |

**为什么不能改成「锚在提交区间」**：审计落地的那几段提交（票 05 `a9562d5`、票 08 `d7bfcc9`、
票 13 `116745b`、用例与记录 `af241ff`）在 main 上**不连续**——其间夹着别的票的提交
（如 `a6cfd06`），没有任何 commit range 的 diff 等于上面这张白名单（实测 `a6cfd06..af241ff`
把 `crates/**` 也算进来）。按分支取数的判据描述的是**任务分支**的那一次 diff，不是持久事实，
故只能归位到台账。
