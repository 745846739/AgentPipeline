# UX 审计整备 · 交付记录（27 张票全部落地）

**规格:** [spec.md](spec.md)　**票:** [issues/](issues/)　**审计证据与判定:** [README.md](README.md)
**并行协作须知:** [parallel-brief.md](parallel-brief.md)　**裁决原文:** [.rows/](.rows/)
**开源就绪:** [oss-readiness.md](oss-readiness.md)

本文件是编排者的收口记录：每张票「做了什么、落在哪、验收点在哪、哪些没覆盖」。
逐票的细节汇报在各页文件与 [handoff/](handoff/)（跨文件交办）里，本表只做索引与对账。

---

## 一、逐票对账

| 票 | 叠 | 落地 | 验收点（机器门 / e2e） |
|---|---|---|---|
| 01 补拍证据与判定 | A | `.scratch/ux-audit/README.md` 〇.6–〇.8 + `e2e/ux-audit.spec.ts`（五处琥珀特写、空列、404 深浅两套；72 张 PNG） | 取证类用例**默认 skip**（`UX_AUDIT=1` 才跑），不进 `make check-e2e` |
| 02 三个模态框一起修 | A | 新增 `components/ui/Modal.svelte`（Escape 不依赖焦点、开框焦点进第一个输入框、Tab 环绕、`role="dialog"` + `aria-modal` + `aria-labelledby`）；三个调用点改用它 | `e2e/modal-keyboard.spec.ts` ①③④（真应用）+ `components/ui/Modal.test.ts` 等 4 个单测 |
| 03 404 给回看板的路 | A | `App.svelte` 的 not-found 分支改用 `<EmptyState>` + 回看板的 `href`；展示地址不带 `#` | `e2e/settings-landing.spec.ts`「404 给回看板的路」 |
| 04 删除理由挪到动手时 | A | `SettingsProjects.svelte`：删掉常驻红字，被拒那一刻给出下一步；本页把 `.btn.danger` 拉回与中性钮同档（描边 / 硬投影 / 实底） | `e2e/settings-empty-and-copy.spec.ts`「删除被拒的理由只在动手之后出现」「破坏性动作不比中性动作轻」 |
| 05 依赖任务 ID 的候选列表 | A | `NewTaskDialog.svelte` 加原生 `datalist`（候选来自看板已在用的任务列表，**零新端点**，不做完整选择器） | `e2e/modal-keyboard.spec.ts` ②（选了候选 → 提交 → `depends_on` 真的含它） |
| 06 指标页的任务级入口 | A | 跨流接口 `#/metrics?task=<id>`：`router` 解析 query（N）→ 指标页自动载入并高亮（Me）→ 任务详情放链接（D） | `e2e/metrics-entry.spec.ts` ①；`TaskDetail.test.ts` 的 href 逐字 |
| 07 项目分析入口 | A | 跨流接口 `#/settings/projects?project=<id>&analyze=1`：任务详情放链接（D）→ 项目页自动就位并触发分析（S） | `e2e/settings-empty-and-copy.spec.ts`「据 query 自动就位并触发分析」「读不到 query 时行为不变」 |
| 08 不再同屏两份 diff | A | `DiffReviewPanel` / `ReviewForm` / `PendingDossier` / `TaskDetail` 传 `diffInPane`：用户已在 Diff 页签时档案盒只留结论与**动作行** | `e2e/pending-dossier.spec.ts`（含移动款坞）；`TaskDetail.test.ts`；既有 6 个 e2e 的 8 处动作行定位器全绿 |
| 09 小节标题层级 | A | 设置各页小节标题落到 12px 档；阶段配置页在 N 的新页 | `e2e/settings-empty-and-copy.spec.ts`「小节标题明确小于页面标题」；`e2e/settings-landing.spec.ts`「阶段配置有自己的页」 |
| 10 并行站名标签内边距 | A | `PipelineRail.svelte` 补回原型里的 `padding: 0 4px`；层叠一律未动（裁决 ④） | `e2e/board-overflow.spec.ts`「侧站名标签带遮罩与 4px 内边距」 |
| 11 公开发布 | A | 新增 `LICENSE`（MIT，与 `Cargo.toml` 同口径）+ `.scratch/ux-audit/oss-readiness.md` 逐类核过记录；可见性判定：**发布规则里没有冲突**（`.gitignore` 因此未动——本次唯一的 `.gitignore` 改动是编排者早于本票加的两条审计截图排除）；`docs/README.md` 登记公开范围与许可 | 人工核对（`git check-ignore` 逐文件、链接可达性 45/0 缺失、敏感信息 4 条处置） |
| 12 琥珀收敛 | A | 按**票 01 的逐处判定**执行（决策 203 落清单）：指标重试率**灯色随值取**（Me）、推荐技能「未安装」与入口闸标题回中性档（S）、`.warnnote` / `.replace-note` / `.warn` 保留 | `e2e/metrics-entry.spec.ts`「琥珀随值取」；`e2e/settings-empty-and-copy.spec.ts`「入口闸标题与未安装回中性档」 |
| 13 空态语汇 | A/B→A | 七处空态 + 404 统一 `<EmptyState>`（形状唯一、提到别处可点、文字 `--text-3`）；形状规格落 `design/frontend-design.md` §5.3 + 视觉规格 §3（**决策 202**） | `e2e/settings-empty-and-copy.spec.ts` 四条；`e2e/metrics-entry.spec.ts` 空态两条；`e2e/settings-landing.spec.ts` 404 |
| 14 对比度规则（决策） | B | **决策 195**（`docs/decisions.md`）+ 视觉规格 §2.6 | 无（决策票） |
| 15 对比度实现 + 机器门 | B | `theme/contrast.ts`（从契约读值、逐档门槛、豁免名单）+ `contrast.test.ts` + `--text-3` 提值（深 `#8E8CA5` / 浅 `#636477`，`--done` 不动）+ `app.css` 归位 | `theme/contrast.test.ts`（19）+ `theme/css-parity.test.ts`（62）+ `e2e/pixel-theme.spec.ts` 的计算值断言 |
| 16 看板溢出（决策） | B | **决策 196** + 视觉规格 §2.7 | 无（决策票） |
| 17 看板溢出实现 + 几何守护 | B | `Board.svelte` 钉右 / 初始滚动 / 右缘指示；`pipeline.ts` 段内几何由契约列宽推导；`pipeline.geometry.test.ts` 守护（同一把刀、**带键唯一**、`1400px` === 契约） | `pipeline.geometry.test.ts`（10）+ `e2e/board-overflow.spec.ts`（6） |
| 18 脊线数字口径（决策） | B | **决策 197** + 视觉规格 §3.4 | 无（决策票） |
| 19 脊线实现 | B | 框内 `累计` 词（同色同框、`tabular-nums` 只作用于数字）+ `.ct.pen` 补齐；列头不带词、并行侧站不画数字 | `e2e/board-overflow.spec.ts`「脊线数字读作『累计 N』」；`pipeline.geometry.test.ts` 的口径断言 |
| 20 设置信息架构（决策） | B | **决策 198** + 交互规格 §4.1 / §4.3 + 视觉规格四处过时表述 | 无（决策票） |
| 21 设置落地页 + 顶栏三项 | B | 新增 `SettingsLanding.svelte` / `SettingsStages.svelte`；顶栏页面导航行收到三项；手机访问入口挪到落地页（判据仍是回环来源）；阶段配置从 providers 页摘出（S 接收 handoff） | `e2e/settings-landing.spec.ts`（6）+ `router.test.ts`（13）+ `SettingsLanding.test.ts` |
| 22 内部编号退场（决策） | B | **决策 199** + 交互规格 §12.3 的「行为 / 规则 → 实现位置」表 + 视觉规格那处冲突改写 | 无（决策票） |
| 23 编号退场实现 + 悬空检查 | B | 各页正文去编号（T/S/Me/D/M/B 各自名下）；`lib/copy-discipline.test.ts` 兜底门；`lib/behavior-map.test.ts` 悬空引用检查（PENDING 名单已按「落地后逐条删」清空） | `copy-discipline.test.ts`（8）+ `behavior-map.test.ts`（40）+ 两条 e2e 的「整页无编号」 |
| 24 隐喻首现翻译（决策） | B | **决策 200** + 交互规格 §12.2 + `docs/glossary.md` 的词表列 | 无（决策票） |
| 25 隐喻首现实现 | B | 各页在自己名下文件里给首现译文（Talk / TaskDetail / Metrics / Share） | 文案类不做精确匹配断言；`e2e/metrics-entry.spec.ts` 的页脚译文一条 |
| 26 过滤槽（决策） | B | **决策 201** + 交互规格 §4.4 | 无（决策票） |
| 27 指标页说人话 | A | 第一段整段重写（四个量各自的含义与算法）、计数句去字段名、分母为 0 给解释而不是一条横线、页脚去编号 | `e2e/metrics-entry.spec.ts` 四条 + `lib/metrics.test.ts` |

**决策日志**：195–203 已落 `docs/decisions.md`（表尾 9 行），195/196/198/199/201 修订决策 169；
202 / 203 由票 01 的取证判定追加、不修订任何既有决策。

**新增机器门**（都不新增可测试性接缝，全在既有两条接缝上）：`theme/contrast.test.ts`、
`lib/copy-discipline.test.ts`、`lib/behavior-map.test.ts`、`lib/pipeline.geometry.test.ts`。

**新增 e2e**（都进 `make check-e2e`）：`board-overflow` / `modal-keyboard` / `settings-landing` /
`settings-empty-and-copy` / `metrics-entry` / `pending-dossier`。

---

## 二、交付中发现并修掉的两个真实缺陷（e2e 才抓得到）

1. **`each_key_duplicate`（严重）**：`pipeline.ts::spineBelts` 的键是 `` `${kind}${col}` ``，
   而并行的上下两条 `br` 同 `col=1` → 撞键。Svelte 在浏览器里当场抛错、**整块看板不渲染**；
   单测全绿（不跑 Svelte 运行时）、jsdom 也看不见。修法：键带上 `top`；守护加进
   `pipeline.geometry.test.ts`「段内每条带的 key 两两不同」。
2. **窄屏对讲台页头被译文挤成两行（决策 192 的回归）**：`值班长（跟我对话的 AI）` 让 430px 下的
   页头从 **38.44px** 涨到 **72.23px**，对话区从 352.63px 掉到 **318.83px**——正是决策 192
   把对话区从 26px 救回来的那件事。修法：页头是标题栏，按决策 200 裁决 ④「标题里不翻译」
   去掉译文；译文落到本页正文里 `值班长` 首现处（时间线空态），窄屏同样读得到。
   实测页头回到 38.44px、对话区回到 352.63px。**这条只有既有的 `talk.spec.ts` 窄屏几何用例抓得到。**

---

## 三、闸门

`make check` 四层全绿（2026-09-17）：

- `make check-lint`：fmt --check + clippy -D warnings ✅
- `make check-test`：`cargo test --workspace` ✅
- `make check-frontend`：vitest（41 文件 / 467 用例）+ svelte-check 0 error + vite build ✅
- `make check-e2e`：产物新鲜度守卫 + **79 passed** ✅

---

## 四、未覆盖 / 有意留下的

> **这一节里的「延后项」已同步搬进 [docs/backlog-v2.md](../../docs/backlog-v2.md) §B.8**（v2 预留），
> 两处一起改，别只改一边。本文件保留逐条的来龙去脉，那边只留「要做什么」。

1. **票 12 / 13 / 25 的「收敛后」截图证据没拍。** 原因是有意保护票 01 的证据：
   `e2e/ux-audit.spec.ts` 的产物落在 `.scratch/ux-audit/*.png` 的**固定文件名**上，
   再跑一次就会把「改动前」的 72 张覆盖掉，而那些图正是决策 195–203 的判据来源。
   要看收敛后的样子，任选其一：先把现有 PNG 挪走再 `UX_AUDIT=1` 跑一次；或改 spec 的输出前缀。
2. **手机访问页「读不到配对令牌」那条故障分支有意不 EmptyState 化**（决策 202 留档）：
   它是后端报文精确文本的承载位，进了长句会失去可定位性。
3. **票 09 只断言了市场页与模型与密钥页**；阶段配置页的层级由 `settings-landing.spec.ts` 覆盖。
4. **`LICENSE` 的 holder 写作 `Copyright (c) 2026 泽运`**（仓库无 `authors` / `copyright` 字段，
   取 git 提交者身份）——若想改成项目名 / 组织名，改这一行即可。
5. **`.scratch/ux-audit/` 本身是否随仓库公开**属编排者 / 维护者决定；已核实它不被整目录排除
   （只排除 `*.png`）。
6. **票 25 的「逐词深浅两套截图证据」**与第 1 条同一处置（证据不是门）。
7. **票 05 的第 3 条验收「逗号分隔的多个依赖仍然成立，候选只对正在输入的那一段生效」只做到一半**：
   原生 `datalist` 按**整串**过滤，所以填第二个 ID 时不会给候选（第一个 ID 有候选）。
   做「只对正在输入那一段生效」要自绘下拉，而 spec 的 Out of Scope 明确排除「把依赖任务 ID
   做成完整选择器」——**两条要求互相顶**，本轮按 Out of Scope 让路，多 ID 仍可手打（原行为不变）。
8. **决策 200 的 12 词表里有 7 个词在本轮没有可落的位置**（`工头` / `对讲台` / `值班经理` /
   `信号灯` / `回流带` / `道具栏` / `台账` 的一半）：它们在本站要么只作为**页标题 / 顶栏控件名 /
   发言者称谓**出现（裁决 ④ 明说标题与第一屏控件不翻译），要么只在**代码注释**里出现
   （`工头` 全是注释与内部标识符）。`台账` 已按定稿说法落在设置落地页；`Talk.svelte:185` 的
   `值班长正在查台账…` 指的是**对话记录**、不是「设置这一类页面」，照抄定稿说过去会说错，
   **故有意留空并在此登记**（这是决策 200 词表与其裁决 ④ 之间的一处张力，改词表要动决策）。
9. **票 16 的「列 i 内容中心与脊线站心每列差 2px」**按裁决照原样保留（不当作断言）。

---

## 五、code-review 收口（两轴审查后的改动）

`code-review` 跑完之后按它的发现动手修了下面这些（都在本轮闸门重跑之前落盘）：

| 轴 | 发现 | 处置 |
|---|---|---|
| Spec | `.warnnote` 被改成中性档，与票 01 的判定 / 决策 203 相反 | **回琥珀**：`SettingsProviders.svelte` 不再覆盖 app.css 的 `.warnnote`；决策 203 裁决③ 补明这一处「规格明文允许 → 保留」；e2e 断言改成断言琥珀 |
| Standards | `box-shadow: inset 2px 0 0` 是第 4 种投影形态，不在视觉规格登记的三态里 | `Metrics.svelte` / `SettingsProjects.svelte` 的位置标记改用像素纪律里那条唯一例外 `border-left: 4px`，并用 `padding-left` 抵掉多出的 2 / 4px |
| Standards | `.btn.danger` 在项目页与模型与密钥页各复制了一份**逐字相同**的局部覆盖 | 定档到 `app.css` 的 `.btn.danger`（全站一处，票 04 的诉求本来就是「这一类动作的整体量级」），两处局部覆盖删除；顺带补上按下消影（3px） |
| Standards | `metrics.ts` 的 `firstPassAvailable` 在生产代码里已无消费者（只被测试读） | 删掉该字段，「有没有意义」读 `firstPassReason === ''`；`metrics.test.ts` 跟着改 |
| Standards / Spec | 两个 e2e 文件按 `EmptyState` 的**内部 class**（`.empty` / `.es` / `.en`）定位，违反 spec 的 Testing Decisions | 改成按**用户看得见的文字**定位（`getByText`），顺带把「状态那半句」也纳入断言 |
| Spec | 票 25：`台账` 在设置落地页有可见正文而没译文 | 按决策 200② 的定稿说法补上（见上表票 25 行） |

**未改判的（审查提出但判为不需要动）**：`Modal.svelte` 的 `WeakSet` 事件去重被指为冗余
（遮罩的 `stopPropagation` 已经拦住 window 那一路）——它是一条便宜且被 e2e 覆盖的兜底，
删掉的收益不足以承担回归风险，故保留；`pipeline.ts` 的 `spineBeltsAll` 等导出是守护测试的
读值口，属既有契约模块的纯函数，不视为投机抽象；决策 200 的文案在各页**各自持有**（而不是抽一个
共享常量模块）是按「按页面首现」的口径本来就要各写各的，抽模块属于过度设计。
