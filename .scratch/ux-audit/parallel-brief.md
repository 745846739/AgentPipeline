# UX 审计整备 · 并行协作须知（所有子代理先读这一份）

**规格:** [spec.md](spec.md)　**票:** [issues/](issues/)　**审计证据与报告:** [README.md](README.md)

本 effort 把 27 张票拆成若干**互不重叠的文件所有权**的工作流，由多个子代理并行推进，
最后由编排者统一跑全量质量闸门（`make check`）。你只做派给你的票，只改派给你的文件。

---

## 一、铁的规则（违反 = 破坏并行）

1. **只改你名下文件。** 需要改别人名下的文件时，不要动手——把需求写进
   `.scratch/ux-audit/handoff/<你的代号>-<票号>.md`（格式：目标文件 / 要什么 / 为什么），
   并在最终汇报里点出这一条。编排者会转交。
2. **不要跑全量构建或 e2e。** 禁止 `npm run build`、`make build`、`make check`、
   `npx playwright test`（`frontend/dist` 与 release 二进制是全仓库共享产物，
   并行重建会互相污染）。编排者在所有实现落地后统一构建并跑 e2e。
   - 允许：`cd frontend && npx vitest run <你自己的测试文件>`、
     `cd frontend && npx svelte-check --tsconfig ./tsconfig.json`。
   - 允许跑 e2e 的唯一例外：票面明确要求你取证/补拍截图，且你被点名负责 `.scratch/ux-audit/` 证据。
3. **e2e 用例写新文件，不改既有 spec。** 新增 `frontend/e2e/<name>.spec.ts`。
   需要改既有 spec（例如 `pixel-theme.spec.ts` 的 token 断言）时按第 1 条走 handoff，
   **除非该文件在你名下**（见所有权表）。
4. **`npm test` 会跑全仓库 vitest。** 如果红的用例不在你名下文件里，那是并行的别人在半路，
   **不要去修**，写进汇报即可。
5. **不新增可测试性接缝、不为测试改生产代码形状。** 测试只落既有两类接缝：
   主题契约模块（`frontend/src/theme/contract.ts`）与前端 e2e harness（`frontend/e2e/harness.ts`）。
6. **像素纪律（`css-parity.test.ts` 机器门，务必先读它）：** 圆角 0；描边只有 2px（`border-left` 4px 例外）；
   `font-size` 必须是 12 的倍数（16px 仅移动输入框豁免）；不得出现裸十六进制颜色（一律走 token）；
   不得用平滑渐变、缓动关键字；`@keyframes` 名字在白名单内；不得手绘 sprite（必须来自契约 `SPRITES`）。
7. **文案不用在测试里精确匹配**（按钮名 / 状态词这类契约性字符串除外）。

---

## 二、文件所有权表（一个文件只有一个主人）

| 代号 | 工作流 | 名下文件（互斥） |
|---|---|---|
| **T** | 主题 / 对比度 | `frontend/src/theme/contract.ts`、`frontend/src/theme/contract.test.ts`、`frontend/src/theme/css-parity.test.ts`、`frontend/src/theme/contrast.ts`（新）、`frontend/src/theme/contrast.test.ts`（新）、`frontend/src/app.css`、`frontend/src/lib/copy-discipline.test.ts`（新）、`frontend/e2e/pixel-theme.spec.ts` |
| **B** | 看板 / 脊线 | `frontend/src/lib/pipeline.ts`、`frontend/src/routes/Board.svelte`、`frontend/src/components/board/BoardColumn.svelte`、`frontend/src/components/pipeline/PipelineRail.svelte`、`frontend/src/stores/board.svelte.ts`、`frontend/src/lib/pipeline.geometry.test.ts`（新）、`frontend/e2e/board-overflow.spec.ts`（新） |
| **N** | 外壳 / 导航 / 落地页 | `frontend/src/router.svelte.ts`、`frontend/src/router.test.ts`、`frontend/src/App.svelte`、`frontend/src/components/layout/TopBar.svelte`、`frontend/src/components/layout/StatusLine.svelte`、`frontend/src/routes/Talk.svelte`、`frontend/src/routes/SettingsLanding.svelte`（新）、`frontend/src/routes/SettingsStages.svelte`（新）、`frontend/e2e/settings-landing.spec.ts`（新） |
| **S** | 设置各页 | `frontend/src/routes/SettingsProjects.svelte`、`SettingsProviders.svelte`、`SettingsMarket.svelte`、`Share.svelte`、`frontend/src/components/settings/*`（除 `TrackSegmentBars.svelte`）、`frontend/e2e/settings-empty-and-copy.spec.ts`（新） |
| **D** | 任务详情 | `frontend/src/routes/TaskDetail.svelte`、`frontend/src/components/task/*`（除 `SplitDialog.svelte`、`ModelOverrideDialog.svelte`）、`frontend/e2e/pending-dossier.spec.ts`（新，仅 08 那条） |
| **M** | 三个模态框 | `frontend/src/components/board/NewTaskDialog.svelte`、`frontend/src/components/task/SplitDialog.svelte`、`frontend/src/components/task/ModelOverrideDialog.svelte`、`frontend/src/components/ui/Modal.svelte`（新）、`frontend/e2e/modal-keyboard.spec.ts`（新） |
| **Me** | 指标页 | `frontend/src/routes/Metrics.svelte`、`frontend/src/lib/metrics.ts`、`frontend/src/lib/metrics.test.ts`、`frontend/src/components/settings/TrackSegmentBars.svelte`、`frontend/e2e/metrics-entry.spec.ts`（新） |
| **DEC-VIS** | 决策票 14 / 16 / 18 | `design/theme-6-pixel.md`、`.scratch/agentpipeline-pixel-theme/spec.md`（只加修订标注） |
| **DEC-IA** | 决策票 20 / 22 / 24 / 26 | `design/frontend-design.md`、`docs/glossary.md`、`frontend/src/lib/behavior-map.test.ts`（新，悬空引用检查） |
| **EV** | 票 01（取证） | `frontend/e2e/ux-audit.spec.ts`、`.scratch/ux-audit/README.md`、`.scratch/ux-audit/issues/01*.md`、`.scratch/ux-audit/issues/12*.md`、`.scratch/ux-audit/issues/13*.md` |
| **OSS** | 票 11（开源落地） | `.gitignore`、`LICENSE`（新）、`.scratch/ux-audit/oss-readiness.md`（新）、`docs/README.md` |
| **编排者** | 决策日志 / 文档回填 / 闸门 | `docs/decisions.md`、`docs/testing.md`、`docs/README.md`（票 11 相关行由 OSS 提，其余编排者改）、`frontend/src/components/ui/EmptyState.svelte`（已建好，见 §四.3） |

**跨流接口（已定，照抄别改）**

| 接口 | 形状 | 生产者 | 消费者 |
|---|---|---|---|
| 任务指标入口 | `#/metrics?task=<task_id>`，路由解析出 `query`，指标页据 `query.task` 自动载入并高亮 | N（router 解析）+ Me（消费） | D（放链接）、Me |
| 项目分析入口 | `#/settings/projects?project=<id>&analyze=1`：设置落地页/任务侧的链接都走它，项目页据此自动就位并触发分析 | N（router 解析）+ S（消费） | D（放链接）、S |
| 空态组件 | `frontend/src/components/ui/EmptyState.svelte`，props：`state` / `next?` / `href?` / `linkLabel?` | 编排者（已建） | 所有需要空态的页面 |
| 设置落地页 | 路由 `#/settings`（`route.name === 'settings-landing'`）；分类两项「谁能进来」「怎么跑」 | N | S（各页可放「返回设置」）、N |
| 阶段配置页 | 路由 `#/settings/stages`（`route.name === 'settings-stages'`），内容从「模型与密钥」页搬出 | N（建页）+ S（从 providers 页摘掉该段，不写新页） | N、S |

---

## 三、跨流统一规则（每条票都按这套口径做，避免各改各的）

### 1. 灰字档位（决策 195 / 票 14·15）

- `--text-hi`：标题、可执行物、当前游标。`--text`：正文。
- `--text-2`：次文本，门 ≥ 4.5:1（保持）。
- `--text-3`：**重新定位为「次级必读」**，门 ≥ 4.5:1（**要提亮/加深**，深浅两套成对改）。
- `--text-4`：**纯装饰**，门里豁免，**凡用到它的地方不得承载必读信息**。
- 归位清单（票 15 逐处）：页面导入语、空列/空态的引导句、待处理下拉里的原因文字、
  输入框占位符、任务卡时长、返回入口、各类元信息行（语言 / 闸门命令 / 地址 / 归档行）。
  口径：**读不到就会挡住下一步的 → `--text-3`；纯刻度装饰 → 才留 `--text-4`。**
- 组件里只允许改**用的 token**（class 名/内联 token），不许在组件里写死颜色值，也不许改 `app.css`（那是 T 的文件）。

### 2. 内部决策编号退场（决策 199 / 票 22·23）

- 面向用户的正文里**不再出现**「决策 NN」；正文只留**动作与后果**
  （例：「先去处理那 3 个任务」而不是「有活跃任务的项目不能删除（决策 101）」）。
- 追溯性改由 `title` 悬停提示或折叠说明承载；可追溯的权威索引是
  `design/frontend-design.md` 里那张「行为 / 规则 → 实现位置」表（DEC-IA 建）。
- 例外：代码注释、开发文档、测试文件里的编号照旧（机器门只扫面向用户的文案）。

### 3. 空态语汇（票 13）

- 七处空态（看板 / 对讲台 / 指标 / 项目 / 模型与密钥 / 技能市场 / 手机访问）+ 404，
  统一用 `<EmptyState>`：**状态 → 下一步 → 可选入口**；凡是提到另一个页面，**必须可点**。
- 文字用可读档（`--text-3`），不用装饰档。

### 4. 车间隐喻首现翻译（决策 200 / 票 24·25）

- 隐喻保留，每个词在全站**首次出现处**给一次平实说法；同一页面内不重复解释。
- 顶栏项名、「新建任务」等第一屏控件直接用平实词（票 21）。
- 词表与定稿说法以 DEC-IA 落表的为准（见 `design/frontend-design.md`）。

### 5. 琥珀（`--pending`）收敛（票 12）

- 琥珀只出现在**有东西要你处理**的地方；不是告警的用途回到中性档
  （逐处处置以票 01 的判定结论为准，判定结论写在 `.scratch/ux-audit/README.md`）。

---

## 四、验证协议

1. 每个改动都要有**测试锚点**，落在既有层：vitest 纯函数 / 契约，或 playwright（真应用、真后端）。
2. 单测写法照既有模板：`frontend/src/theme/contract.test.ts`（纯数据断言 + 逐值比对）、
   `frontend/src/lib/pipeline.hero.test.ts`（几何）。**断言落在用户可见/计算样式上，不落在 class 名或组件内部状态。**
3. e2e 写法照既有模板：`frontend/e2e/pixel-theme.spec.ts`（计算样式与几何，含移动断点）、
   `frontend/e2e/happy-path.spec.ts`（真后端 + 按用户动作断言）。用 `./harness` 的 `startApp`。
   新增 spec 默认就进 `make check-e2e`（**审计/取证类截图 spec 例外，须 `test.skip` 默认跳过**）。
4. 汇报格式（编排者按这个收口）：
   - 做了哪些票、每票改了哪些文件（路径:行）
   - 新增了什么测试、跑过什么命令、结果如何（贴关键输出）
   - 哪些断言/验收点**没有**被覆盖到（诚实列出，别写「完成」）
   - 需要别人改什么（handoff 文件名）
