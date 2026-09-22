# 前端设计规格（v1 看板视图）

> 本文扩展 §12.11 前端交互设计，落地为可实现的前端规格。遵循既有决策：16（Svelte + TS + Vite）、
> 79（v1 只发看板）、76（单 SSE 通道）、153（传输层 Tauri 防御约束）、84（并行分支消歧）、49/69/101（allowed_actions 纯渲染）、
> 92（列归属与焦点游标）、34（stalled / archived 表示）、65（v1 只做应用内通知）。
> 交互骨架与页面元素以本文为准（§4–§7）；**视觉方向以主题六「像素机房 · 夜班流水线」为准**
> （[theme-6-pixel.md](theme-6-pixel.md)、[prototype-pixel.html](prototype-pixel.html)，决策 169）。
> 主题三「终端 · 调度电报」已退役（[deprecated/](deprecated/README.md)），其终端专属 token
> 已从 `app.css` 删除。本文 §3 的「夜间调度台」视觉语言已随
> [deprecated/prototype.html](deprecated/prototype.html) 归档。

> **修订（2026-09-16「ux-audit」审计票 20–26 / 决策 198–201）：本文的范围变了——这是规格变更，不是修 bug。**
> 主题六升格为现行视觉规格时（决策 169），**明文把「交互与信息架构改动」划进 Out of Scope**
> （原措辞在 `.scratch/agentpipeline-pixel-theme/spec.md` 的 Out of Scope 首条：「路由、五页签、
> 列语义……全部照旧」）。本次改的正是那一块：
>
> | 改了什么 | 本文落点 | 决策 |
> |---|---|---|
> | 设置类入口按用途两分、新增设置落地页与阶段配置页、顶栏导航行收到三项（决策 240 起四项）、手机访问入口挪位 | §1、§4、§7、§8 | 198 / 240 |
> | 界面正文里的内部决策编号退场，可追溯性交给「行为 / 规则 → 实现位置」表 | §12.1、§12.3 | 199 |
> | 车间隐喻在首次出现处给一次平实说法 | §12.2 | 200 |
> | 状态过滤槽加词、同一个数不再在相邻控件上重复 | §4.4 | 201 |
> | 窄档顶栏重排：铭牌行退场、导航行升为首行、道具栏行只在看板露出（高度按路由两档） | §4.2、§12.3 | 242 |
| 窄档页面导航**移出顶栏、钉到屏幕底缘**成底部页签栏（四项等分 / 命中区 44 / 独占安全区），顶栏非看板路由清零，`--nav-h` + `--sbar-h` 两层让位账本 | §4.2、§12.3 | 243（修订 242 的导航行位置与高度两档） |
>
> **不变的部分写死**：过滤语义与端点一律不动；像素纪律（2px 描边 / 零圆角 / 12 的倍数字阶 /
> 不新增图元 / 动画预算四处）一字不动；`allowed_actions` 纯渲染与卡片禁拖不动。

> **修订（2026-09-18「ux-audit-2」第二轮审计 / 决策 215）：补上「中间档」这一整段。**
> 第一轮的采样点只有 1440（桌面）与 430（移动）两个端点，**480–1240 之间没有任何断点**，
> 于是详情页主栏在 480px 只剩 102px、对讲台对话列只剩 82px、`hero` 轨道把整页撑出横向滚动、
> 状态行在 480–748 之间静默裁掉时钟。决策 215 定的就是这一段：
>
> | 定什么 | 结论 | 本文落点 |
> |---|---|---|
> | 详情页断点 | `≥1100` 两栏（右 320）／`820–1099` 两栏（右 280）／`<820` 折成一列，左栏下限 480px | §6.1（就地标注，见那一节的裁决框） |
> | 对讲台断点 | `≥1100` 两栏（右 340）／`900–1099` 两栏（右 280）／`<900` 折成一列，对话列下限 420px | **主文档 §12.4 的交互设计**（本文只给路由，不给对讲台版面） |
> | 右栏收缩下限 | 280px（两处同值） | §6.1 |
> | `hero` 轨道 | **容器内横向滚**，不裁切、不把滚动传给文档 | §6.1（同上的裁决框） |
> | 状态行档位 | `480–748` 隐三格 `.dep`；`480–560` 再隐量表；容器 `overflow-x: auto` 兜底 | 已落地，见 §12.3 索引表那一行 |
>
> **折行档（`480–899px`）的控件形态（决策 218 补，2026-09-18）：**上面那张表说的是**栏数**，
> 而这一档的**控件形态**同一时刻整体换过一遍——页头收成一行（`<h1>` 转 visually-hidden）+ 右端
> ⋯ 班次菜单、**不画**值班板灯条、工位回执默认收起、输入坞撤掉提示语那一行、单张急停也折、
> 页头做成 **46px 的钉住带子**（这一档共三只钉住物）。**三个断点自此各司其职**：`899` 同时管
> 「时间线折成一列」「页头 ⋯ 与折行档版面」「`forceFold`」（一个断点三处用），`1099` 管右栏
> 280 → 340，`479` 只剩输入框占位语一处用它。断点常量住在 `frontend/src/lib/talkLayout.ts`
> 一处，`talkLayout.test.ts` 把它与 `Talk.svelte` 里的 CSS 字面量对着钉。生效口径见
> `design/theme-6-pixel.md` §3.3 的「修订决策 218 / 220」那一段（含逐块处置表与实测数）。
>
> **移动款（`≤479px`）既有版面一字不动**——决策 192 / 208 的口径只在那里适用；
> 折行那一档复用的是它的三条规则，不是把它抬上来。
> **（2026-09-18 / 决策 218 修订：这一句已不成立。）**这一档的版面本就是按重排后的形状重写的
> （折行档与窄档同源），且窄档自己动了四处：值班板灯条不再画、坞的提示语那一行撤掉、页头收成
> 一行**并钉住**（钉住物由两只变三只）、单张急停也折（`forceFold` 的断点从 `479` 扩到 `899`）。

> **修订（2026-09-18「ux-audit-2」第二轮审计 / 决策 216–217）：动作的确认与量级，以及中流状态去哪儿。**
> 这一轮还翻出两件「规格没写、实现各按各的来」的事，两件都**不动版面**：
>
> | 改了什么 | 结论一句话 | 本文落点 | 决策 |
> |---|---|---|---|
> | 不可逆动作的确认步、后果句、三档量级、打回标签随输入变 | 四类动作走**内联两步确认**（沿用删项目那套）；跳过质量闸的动作降为琥珀描边，破坏性动作红描边 | §6.1、§9.3（新） | 216 |
> | 中流状态：页签 / 过滤 / 班次 / 草稿的去向，以及谁写地址 | 「我在哪」进 URL、「我平常怎么用」与草稿进 localStorage、程序改地址用 `replaceState` | §4.1、§9.4（新） | 217 |
>
> **不变的部分写死**：像素纪律与全部视觉规格（2px 描边 / 12px 字阶 / 零圆角 / 四处动画预算）一字不动；
> `allowed_actions` 纯渲染不变——确认步只加在**在册动作**的提交路径上，不改动作集；
> 决策 132 已移出的无端点动作（「放弃合入」「合并任务」）不复活。

---

## 1. 定位

**一句话：** 一张挂在开发者本机的调度台——你看得到每列车（任务）在线路（流水线）上的位置，
看得到它正在说什么（流式会话）、跑了什么（命令日志）、花了多少（token），以及最要紧的：
**哪里停下来了等你拿主意（pending）**。

| 维度 | 结论 |
|---|---|
| 用户 | 在自己机器上跑 agent 流水线的开发者 |
| 首要工作 | 一眼看清"跑到哪、哪里等人"，并在 pending 时做决策 |
| 信息密度 | 高：流水线状态 + 流式输出 + 命令日志 + 成本同屏 |
| v1 范围 | 看板视图 + 任务详情 + 对讲台 + 设置类页面 + 全局指标（决策 79，经决策 176 / 198 修订）**[1]** |

> **[1] 修订（决策 198）：** 本行的「设置（项目 / provider）」按 §4 展开为**一个设置落地页 +
> 五个独立设置路由**（项目 / 手机访问 / 模型与密钥 / 阶段配置 / 技能市场）；**指标不是设置**，
> 留在顶栏。功能面没有新增——变的是入口的分类与位置，不是能力。

---

## 2. 设计概念：夜间调度台

流水线的领域语言本身就是铁路语言：**线（track）、站（stage）、车（游标上的任务）、
岔口（sync-check 汇合）、折返线（review→develop / merge→test 打回）、信号灯（状态色）**。
视觉系统全部由此展开，不做第二套装饰语言。

四条原则：

1. **轨道即导航。** DAG 不是配图，是骨架。看板列头挂在轨道站点上，卡片内置迷你轨道线，
   任务详情页复用同一轨道组件作 hero。用户在任何页面看到的都是同一条线。
2. **状态即信号灯。** 五色信号系统（执行 / 等人 / 失败 / 完成 / 支线）只承载状态语义，
   不做装饰用色。全站无渐变、无投影堆叠。
3. **等人优先。** pending 是唯一被允许"响"的东西：琥珀色、顶栏待办计数、任务详情右侧
   dossier 栏。其余一切保持安静。
4. **等宽只给可执行物。** 命令、路径、diff、工具参数、ID、token 数字用等宽字体；
   界面文案用中文短句，按钮写"合入 / 返回修改 / 补充说明并继续"，不写"提交"。

---

## 3. 视觉语言（Design Tokens）

> **视觉方向以主题六「像素机房 · 夜班流水线」为准**（决策 169）：本节的具体 token、字体、
> 形状与动效以 [theme-6-pixel.md](theme-6-pixel.md) §2 为唯一权威——
> 深色「夜班靛」/ 浅色「掌机背光」两套，缝合像素 12px 单一字族，圆角恒 0、描边 2px 一档、
> 硬投影 `4px 4px 0`、字阶只取 12 / 24 / 36、dither 是全站唯一「渐变」、
> 动画只有四处且一律帧步进（`steps()`）。实现侧的事实源是 `frontend/src/theme/` 的主题契约模块，
> 与 `app.css` 互为镜像（由解析测试锁死一致性）。
>
> 下文 §3.1–§3.4 是**主题一「夜间调度台」的历史记录**（已归档，
> 见 [deprecated/prototype.html](deprecated/prototype.html)），保留仅为沿革参考，不再权威。

### 3.1 色彩（历史：夜间调度台）

| Token | 值 | 语义 |
|---|---|---|
| `--ink-900` | `#0C121B` | 页面底色，港湾蓝黑 |
| `--ink-800` | `#111A26` | 面板 / 卡片 |
| `--ink-700` | `#16212F` | 悬浮 / 二级面板 |
| `--line` | `#223246` | 描边、分隔线 |
| `--line-soft` | `#1B2838` | 弱分隔、轨道暗段 |
| `--text-hi` | `#E9EFF7` | 主文本 |
| `--text-2` | `#9DB0C5` | 次文本 |
| `--text-3` | `#64788F` | 弱文本、占位 |
| `--signal-go` | `#3FD68F` | running / 通过 / 游标亮段 |
| `--signal-caution` | `#F2B33D` | pending / stalled（唯一告警色） |
| `--signal-stop` | `#E5544B` | failed / 打回 / 破坏性动作 |
| `--signal-done` | `#7C93AC` | done（刻意退后，不抢视线） |
| `--branch-dev` | `#4CC3E0` | develop-design 支线 |
| `--branch-test` | `#A78BFA` | test-design 支线 |
| `--diff-add` / bg | `#7EE2B0` / `#16342A` | diff 增行 |
| `--diff-del` / bg | `#F0918A` / `#3A2320` | diff 删行 |

**规则：** 一张卡片上最多出现一种信号色。queued / waiting 用 `--text-3` 呈灰。
对比度：正文对底色 ≥ 7:1，弱文本 ≥ 4.5:1，信号色只作点睛不承载正文。

### 3.2 字体

| 角色 | 字体 | 用法 |
|---|---|---|
| 拉丁 UI / 数字 / wordmark | Barlow（400/500/600） | tabular-nums 用于一切数字列 |
| 密集表头 / 列头 / 卡片 meta | Barlow Semi Condensed（500/600） | 站点标签、成本行 |
| 中文正文 | PingFang SC（系统栈，不加载 CJK webfont） | 13–14px 基准，行高 1.55 |
| 可执行物 | JetBrains Mono（400/500） | 命令、路径、diff、工具参数、ID、token 数 |

字阶：11 / 12 / 13（基准）/ 14 / 16 / 20 / 26。纯本地工具，字体经 CDN 引入并带完整系统回退栈。

### 3.3 形状与密度

- 圆角：面板与卡片 6px，药丸 / 标签 3px。全局只有这两档。
- 无卡片投影。层级靠底色（900 → 800 → 700）与 1px 描边表达。
- 看板列宽 280px，列间距 12px，横向滚动；详情内容区最宽 980px 左对齐。
- 间距基准 4px 栅格。

### 3.4 动效

唯一主动效：**首次加载轨道自绘**（stroke-dashoffset 600ms，游标点随后淡入）。
其余全部是响应状态的动效：游标点沿轨滑动（stage_changed，280ms ease-out）、
pending 琥珀呼吸（2.4s 周期）、流式输出尾随光标。`prefers-reduced-motion` 全部关闭。
按钮长耗时异步（§12.11）：点击即 loading 禁用，SSE 回执后复位。

---

## 4. 信息架构与路由

> **本节由决策 198 改写**（原文只列五条路由、顶栏常驻一行把六个入口摊在一起）。改动是**有意的
> 规格变更**：顶栏是「第一屏必须懂」的那一处，六个设置类入口摆在那里等于让人先学词表再开始用。
> **过滤语义、端点与写操作一律不变**；视觉纪律一字不动。

### 4.1 路由表（唯一权威）

| 路由 | `route.name` | 页面 | 入口 |
|---|---|---|---|
| `#/` | `board` | 看板（按 project 过滤；决策 58） | 顶栏第 2 项（决策 240；入口**只此一处**） |
| `#/talk` | `talk` | 对讲台（与值班长对话，决策 176 / 182） | 顶栏第 1 项（兼容原型写法 `#v-talk`） |
| `#/task/:id` | `task` | 任务详情（轨道 hero + 时间线/会话/命令/产出/Diff） | 看板卡片 / 待处理下拉 |
| `#/metrics` | `metrics` | 全局指标（`GET /metrics`） | 顶栏第 3 项；任务侧入口见 §4.5 |
| `#/settings` | `settings-landing` | **设置落地页（新）** | 顶栏第 4 项 |
| `#/settings/projects` | `settings-projects` | 项目（创建即 `POST /projects` + 可选 `/projects/analyze`） | 落地页「谁能进来」 |
| `#/share` | `share` | 手机访问（局域网扫码接入，决策 167 / 186） | 落地页「谁能进来」的手机访问项（**仅本机渲染**） |
| `#/settings/providers` | `settings-providers` | 模型与密钥（provider 台账；决策 112：明文存储、读接口回显 `***`） | 落地页「怎么跑」 |
| `#/settings/stages` | `settings-stages` | **阶段配置（新页，内容从「模型与密钥」页搬出）** | 落地页「怎么跑」 |
| `#/settings/market` | `settings-market` | 技能市场（决策 187 / 194） | 落地页「怎么跑」 |
| 其它 | `not-found` | 404（状态 + 下一步；出口是顶栏那一行页签，决策 240） | — |

- 无 SvelteKit，Vite + Svelte 5（runes）+ 轻量 hash 路由（本地应用，无 SEO 诉求）。
- **首屏默认是对讲台（决策 241）**：地址栏**没写 hash**（`''` / `#`）时，进 store 之前归一到
  `#/talk`。默认落点只接管**空地址**——显式 `#/` 照旧是看板（本表第一行、§4.2 第 2 项，
  决策 240 的「看板是根路由」不修订）；带 hash 的开屏一律不动。
- **query 是这条路由表的一部分**（决策 217）：`#/task/:id?tab=`、`#/?filter=`、`#/talk?session=`、
  `#/metrics?task=`、`#/settings/projects?project=&analyze=1`。参数是短枚举、缺省值不写进地址；
  谁写地址（用户 `pushState` / 程序 `replaceState`）与刷新恢复语义见 §9.4。
- **传输层 Tauri 防御（决策 153）：** ① 本前端是**纯 API 客户端**，一切数据经 HTTP + SSE，不假设部署形态（桌面化 = Tauri 只当外壳，不走 IPC 重写）；② SSE 消费用 **fetch 流式读取**（可携带自定义头），不用 `EventSource`——它带不了自定义头，跨源过不了决策 128 防护；③ 所有写请求**恒携带** `X-AgentPipeline` 头（决策 128 旁路，桌面 webview origin 靠它放行）；④ API base 收敛**单一配置点**：默认同源相对路径，留注入覆盖口（桌面壳注入 `http://127.0.0.1:{port}`）。

### 4.2 顶栏：页面导航行四项（定稿，决策 240 修订决策 198）

| 序 | 项名（定稿） | 落点 | 高亮判据（`route.name`） |
|---|---|---|---|
| 1 | **对讲台** | `#/talk` | `talk` |
| 2 | **看板** | `#/` | `board` |
| 3 | **指标** | `#/metrics` | `metrics` |
| 4 | **设置** | `#/settings` | `settings-landing` / `settings-projects` / `settings-providers` / `settings-stages` / `settings-market` / `share` |

- 决策 198 的收缩**照旧成立**（顶栏是「第一屏必须懂」的那一处，六项让人先学词表再开始用）：
  原先移入落地页的四项不回这一行。**决策 240 只加了一项——看板**：它是根路由，入口原先散在
  wordmark（`href="#/"`）、各页面包屑（`← 看板`）与空态（「回看板」/「去看板新建任务」）
  三处，于是「看板在哪儿进」取决于人当时站在哪一页。收成**一枚页签**之后，那三处一并摘除：
  **看板只从这一行进**。
- **看板页签的高亮只有 `board` 一条判据**：任务详情不是这一行的项，与「对讲台不在详情页点亮」
  同理。
- **收缩只针对页面导航行**（`.navbar`）。顶栏其余部分一字不动：wordmark、项目切换器、
  道具栏（状态过滤槽，§4.4）、**「待处理 N」芯片**（`has_pending_cursor` 的任务数，决策 92 的
  唯一动态计数入口，点击下拉列出全部 pending 任务——琥珀点 + 阻塞原因摘要，点击进入对应任务）、
  「新建任务」。**它们不是导航项**，不受本项影响。
- 原先挂在顶栏的四个设置类项（项目 / 模型与密钥 / 技能市场 / 手机访问）**改为落地页里的项**，
  各自路由不变。
- **窄档（≤479px）的结构由决策 242 重排、再由决策 243 修订位置**：**铭牌行整行退场**
  （logo / wordmark / 会话名 / 信号灯缩略条都不再露出——决策 218 ⑥ 的「点灯跳段」随灯一起
  没有了，故决策 240 记的那条例外作废）；**页面导航行不再属于顶栏——`position: fixed` 钉到
  屏幕底缘**，成为底部页签栏：四项等分整宽、图标在上文字在下、命中区定死 44px、
  `padding-bottom: var(--safeb)` 独占安全区、项目切换器 `.proj` 落行内右端；**状态条叠在
  页签栏上方**（`bottom: var(--nav-h)`）。顶栏只剩**道具栏行且只在看板路由露出**，高度因此
  **按路由分两档：看板 52px、其余 0px（清零）**，连带的 `scroll-margin-top` / 横幅 `top` /
  待处理下拉 `top` 一律改读实测的 `--topbar-h`（**0 也是合法值**，写死一个数必错一档）；
  底部让位读重定义后的 `--sbar-h`（= 状态条 42 + `--nav-h`）。桌面档（≥480px）的两行结构
  一字不动。四项的 DOM、role、`aria-current` 契约一个字不动——变的只是位置。

### 4.3 设置落地页（`#/settings`）

**分类法：按用途两分——「谁能进来」/「怎么跑」。** 判据一句话：**决定「边界」的进「谁能进来」，
决定「跑起来靠什么」的进「怎么跑」。**

| 分类（小节标题） | 分类的一句话（定稿） | 项（定稿） | 项的一句话（定稿） | 落点 |
|---|---|---|---|---|
| **谁能进来** | `哪些仓库算工作对象、哪些设备能连进来。` | 项目 | `把本地仓库接进来当工作对象。` | `#/settings/projects` |
| | | 手机访问 | `让同一局域网里的手机连进来（只在跑服务的这台电脑上配置）。` | `#/share` |
| **怎么跑** | `跑起来用谁的能力、按什么规矩。` | 模型与密钥 | `配 provider 台账与密钥。` | `#/settings/providers` |
| | | 阶段配置 | `每个阶段用哪个 provider、带哪些工具与技能。` | `#/settings/stages` |
| | | 技能市场 | `从 GitHub 仓装技能、看已装技能。` | `#/settings/market` |

- 页面标题 `设置`；导入语 `这台机器上的流水线怎么跑、谁能进来。`
- **各项仍是独立路由，落地页只是入口**（决策 198）：它不复制任何设置内容，不内嵌表单，
  不替子页保存状态。
- **指标不列在落地页**：它不是设置类入口，留在顶栏（第一屏四项之一，决策 240 起）。
- **被拆出来的东西各有其位**：技能市场归技能（`#/settings/market`）、**阶段配置独立成页**
  （`#/settings/stages`）、provider 台账留在原处（`#/settings/providers`，**只摘掉阶段配置那一段**）。
- 每个设置子页给一条**返回设置**的路（`← 设置` → `#/settings`；决策 240 之前还与一条
  `← 看板` 并列，看板收进顶栏页签后那条已摘除——设置子页的上一级是落地页，不是看板）。
- **手机访问项只在跑服务的这台机器本机上渲染**——判据与行为**逐字沿用决策 190**：
  看**来源是否回环**（`onHostMachine()`），不看视口宽度；被判为非本机时这一项**不渲染**
  （不是禁用、不是留个空位）；非本机来源直接敲 `#/share` 仍得到那一页既有的指引
  （决策 189 的「二维码要在这台电脑本机上打开本页才拿得到」，不画扫不出的码）。
  **规则与后果一个字没改，只是入口的位子从顶栏换到落地页。**
- 层级：落地页的分类标题与各子页的小节标题**同档**——12px 字阶（字阶只有 12 / 24 / 36），
  **不与页面标题（24px）同级**（票 09 的口径，两处必须一致）。

### 4.4 状态过滤槽（道具栏，决策 201）

**裁决：图标 + 文字标签。** 七个槽都带词——屏幕上的字才是「一眼扫过去就懂」的那一层，
`title` 与 `aria-label` 只作辅助。

| # | 槽（过滤桶） | 图元（契约 sprite） | 标签（定稿） | `title` | 计数徽章 |
|---|---|---|---|---|---|
| 1 | `all` | `chest` | `全部` | `全部` | 保留 |
| 2 | `running` | `gear` | `执行中` | `执行中` | 保留 |
| 3 | `pending` | `alert` | `待处理` | `待处理` | **去掉**（见下） |
| 4 | `waiting` | `merge` | `等依赖` | `等依赖` | 保留 |
| 5 | `queued` | `flag` | `排队` | `排队` | 保留 |
| 6 | `done` | `trophy` | `已完成` | `已完成` | 保留 |
| 7 | `ended` | `hammer` | `已结束` | `已结束（失败·取消）` | 保留 |

- 词表**逐字取自** `frontend/src/stores/board.svelte.ts` 的 `FILTER_LABELS`，**不新增文案**；
  唯一的例外是第 7 槽在标签位上用短式 `已结束`（完整式 `已结束（失败·取消）` 放 `title`）
  ——34px 槽位行放不下 7 个字。
- 这不是新增的口径：**§5.1 的版面示意里本来就写着词**
  （`全部 执行中 待处理③ 等依赖 排队 已完成 已结束`）——实现此前只画了图元，没画词。
- 形态：槽位高度仍 **34px**，宽度随词走（图元 16px + 4px 间距 + 12px 词）；2px `--pane` 描边、
  相邻槽共享边框（`margin-left: -2px` 照旧）、零圆角、选中态照旧（亮描边 + `--wash` 底）、
  计数徽章位置照旧。**不新增图元、不新增颜色、不新增动画位。**
- **窄屏（<480px）只给当前选中槽带词**，其余六槽保持 34px 图标槽（行宽由约 226px 涨到约 250px）。
  槽位行照旧横滚，但**「待处理 N」芯片与「新建任务」必须完整可见**——430×900 下
  `新建任务` 的 `getBoundingClientRect().right ≤ 430`，不随槽位横滚出屏。
- **为什么不是「收进一处」**：过滤是看板第一屏最常用的一次动作，收进下拉要多一次点击，
  还丢掉「每个桶里有几个」的同屏读数；而这一条修的是「看不懂」，不是「太占地方」。
- **为什么不是「只保留悬停与读屏名」**：触屏上没有悬停，而「一眼扫过去知道每个桶是什么」
  正是这条诉求本身。
- **过滤词表不进隐喻词表（决策 200 / 201）**：那七个词是领域状态词（`TaskStatus` 的中文叫法），
  它们本身就是平实词、没有第二个叫法；需要认的是七枚**自造图元**，那由主题契约的 sprite 表管，
  不是术语表管。

**计数去重（定稿规则）：同一个数不得在相邻的两个控件上同时出现。**

| 位 | 处置 |
|---|---|
| 第 3 槽的计数徽章 | **去掉**——`countFor('pending')` ≡ `pendingCount`，紧邻的「待处理 N」芯片显示的是同一个数 |
| 「待处理 N」芯片 | **保留原位**，成为 pending 数的**唯一显示位**（决策 92 的入口与下拉逐字不变） |
| 第 3 槽的 `title` / `aria-label` | **保留数量**（`aria-label="待处理（3）"` 照旧）——去掉的是**视觉重复**，不是信息 |
| 其余六槽的徽章 | 保留（各自桶唯一的就地读数） |
| 底部状态行 | **不去数、不减项**：它在页面另一端固定位置，是「流水线整体态势」的读数（含 token 总量与主题切换），与顶栏中枢不构成相邻同屏重复；五组数与顶栏徽章的重复是「位置即用途」的重复，不是噪声 |

### 4.5 输入与入口

- 依赖任务 ID 用**原生 `datalist`**（不做完整选择器）。
- 指标页的任务级入口：任务详情放链接 `#/metrics?task=<task_id>`，路由解析出 `query`，
  指标页据 `query.task` 自动载入并高亮——不再要求手打 ULID。
- 项目分析入口：`#/settings/projects?project=<id>&analyze=1`，项目页据此自动就位并触发分析；
  任务侧的链接都走它。

---

## 5. 看板视图

### 5.1 布局

```
┌────────────────────────────────────────────────────────────────────────────┐
│ AgentPipeline [项目▾] 全部 执行中 待处理③ 等依赖 排队 已完成 已结束 [+ 新建任务] │
├────────────────────────────────────────────────────────────────────────────┤
│   ●init───●architect──┌●design∥┌──●develop───●review───●test───●merge──●done│  ← 轨道脊线（列头即站点）
│                       │ dev ∥ test │┄┄ 折返虚线（review→develop 等）┄┄┄┄ │
├────────────────────────────────────────────────────────────────────────────┤
│ [init]   [architect-design]   [develop-design ∥ test-design]   [develop] …  │
│  卡片      卡片                卡片（双药丸）                   卡片         │
└────────────────────────────────────────────────────────────────────────────┘
```

> 上图只画顶栏的**道具栏一行**（过滤槽 + 待处理计数 + 新建任务）；它下面的**页面导航行**按
> §4.2 只有四项（对讲台 / 看板 / 指标 / 设置，决策 240），图上不重复画。

- **列 = 阶段**（决策 92 的"独立槽位"）：8 列 = init / architect-design /
  **develop-design ∥ test-design（双轨合并列）** / develop / review / test / merge / done。
- **sync-check 全站不展示**（决策 107 + 2026-09-11 评审确认）：它不占游标行、任务永远不会
  "停在"它上面——不设列、轨道不设站、卡片迷你轨不设刻度、时间线不显示其 join 记录。
  汇合由双轨直接合流进 develop 表达；`waiting_join` 由分支药丸的"等待汇合"字形承载；
  backtrack 呈现为回到 architect-design 的自动流转记录。
- 列头即轨道站点：站点圆点渲染该列任务聚合状态（有 pending → 琥珀；在跑 → 绿），
  轨道把各列连成一条线；并行区间画成真实分岔双轨（青 / 紫），在 develop 站前直接合流
  （不设汇合站点）；打回路径（sync-check→architect、review→develop、merge→test / develop）画为
  轨道下方的灰色虚线折返弧，出现打回任务时短暂点亮为红。
- 终态任务：done 列展示已合入任务；failed / cancelled 卡片停留在其终态游标所在的列，
  用「已结束（失败·取消）」过滤桶收敛（决策 131）；archived 默认不可见
  （`include_archived=false`，决策 101）。

### 5.2 任务卡片解剖

```
┌──────────────────────────────────────┐
│ 实现用户登录                     12m  │   标题（text-hi）＋ 持续时长（等宽数字）
│ ─●────●────●──○──○──○──○──○─         │   迷你轨道线：9 站点（去 sync-check），游标点亮
│ develop.execute · 第 2 次尝试         │   游标药丸（并行区间双药丸，决策 84）
│ 45.2k tok   214 calls  kanban/t-0042 │   成本行（等宽，token 计数实时累加；金额 v2——差距④）
└──────────────────────────────────────┘
```

- **迷你轨道线是卡片的身份特征**，替代进度条：9 个刻度点串成一条线（10 阶段去掉
  sync-check），完成的点亮，游标处是滑动圆点，并行区间用青紫双色刻度。
- 游标药丸：`分支 · 节点`（mono）。pending 的药丸按**分支色**着色（develop 青 /
  test-design 紫）并加 ⏸ 字形表达阻塞，可分别展开（决策 84：按分支着色，动作集按游标
  独立下发）。
- pending 卡片：琥珀左缘 + 阻塞原因一行 + 动作按钮组（`allowed_actions` 纯渲染，
  `kind: resume` 走 `POST /resume`，`side_effect` 走专用端点，决策 69/105）；
  `requires_input` 的动作渲染行内输入框（仅 `info_insufficient` 有自由输入，决策 79）。
- stalled 卡片（`stalled = true`，决策 34）：整卡琥珀描边 + 「已滞留 3 天」角标。
- **禁止拖动**（§12.11）。卡片整卡可点进详情。

### 5.3 空态

空列不放插画，一句话：如 done 列空 →「还没有任务走到终点。跑完一个任务它会出现。」
整板空 →「新建第一个任务，流水线会从 init 开始走。」

> **决策 202（2026-09-16「ux-audit」审计票 13）：本节的空态形状由「只写两句」升格为全站唯一的空态规格。**
> 票 01 的取证判定：本节原先只写了看板的**空列**与**整板空**，主题四 / 主题五各自都有「空态」一行而
> 主题六那一行丢了；七个页面的空态与 404 从未被定义过形状，于是同一件事长成七个样子（其中看板那句
> 「到『设置 · 项目』添加一个本地 git 仓库」**指着一个页面却不是链接**）。定稿规则：
>
> 1. **形状唯一**——状态（现在是空的、缺什么）→ 下一步（做什么）→ 可选入口（去哪），由
>    `frontend/src/components/ui/EmptyState.svelte` 一处承载（props `state` / `next?` / `href?` /
>    `linkLabel?`），七处空态（看板 / 对讲台 / 指标 / 项目 / 模型与密钥 / 技能市场 / 手机访问）
>    与 404 **全部用它**，不新增第二种写法。
> 2. **提到另一个页面必须可点**——`href` 给真路由（`#/settings/projects`、`#/` 这类），不是「一段提到
>    页名的文字」，也不是 `<button>` + `router.navigate`：入口是**路**，用户在地址栏看得见它去哪。
> 3. **文字用次级必读档**（视觉规格 §2.6 的 `--text-3`，门槛 4.5:1），不用装饰档——空态引导句正是
>    「读不到就挡住下一步」的典型。
> 4. **404 一并纳入**——不存在的地址给一条回看板的路，展示的地址不带 `#`。
>
> **有意例外（留档）**：手机访问页「读不到配对令牌」那条故障分支**不** EmptyState 化——它是后端报文的
> 精确文本承载位，进了长句会失去可定位性。形状与色档见视觉规格 §3 的「空态」一行（决策 202 补回）。

---

## 6. 任务详情页

### 6.1 结构

```
┌───────────────────────────────────────────────────────────┬───────────────┐
│ ← 实现用户登录        running · 12m34s · 45.2k tok · kanban/t-0042        │
│ ┌──────────────── 轨道 hero（全 DAG，节点级 ✓/●/○/⏸/✗/↩）───────────────┐ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│ [时间线] [会话] [命令与输出] [产出文件] [Diff]                             │
│                                                                          │
│  （当前 tab 内容）                                                        │  │ 待办
│                                                                          │  │ dossier
│                                                                          │  │ (pending
│                                                                          │  │  时出现)
└──────────────────────────────────────────────────────────────────────────┴───────────────┘
```

- **轨道 hero**：§12.4.2 流水线视图的实现体（9 站，不含 sync-check——决策 107，它不是
  任务位置），节点状态图例沿用 ✓/●/○/⏸/✗/↩；并行区间双轨分岔；当前游标有滑动圆点与心跳微光。
> **裁决已落、实现未到（决策 215 / 票 07；实现票 18 / 19 状态 open）**
>
> 这一节画的是一栏宽屏的形态。**中间档（480–1240px）的折法已经定了，但代码还没改**——
> 实现时按这张表，不要另定一套：
>
> | 宽度 | 形态 |
> |---|---|
> | `≥1100px` | 现状两栏：`minmax(0, 1fr) 320px` |
> | `820–1099px` | 两栏、右栏收到 **280px**：`minmax(480px, 1fr) 280px`（依据 `820−40−18−280 = 482 ≥ 480`） |
> | `<820px` | **折成一列**（档案盒落到主栏下方，DOM 顺序不变），复用窄屏那三条规则 |
>
> `hero` 轨道在折行档**容器内横向滚**（`.rail.hero { overflow-x: auto }`）——站点坐标写死在
> `lib/pipeline.ts`，裁掉等于「后面的工位不存在」；**滚动只发生在容器里，不传给文档**。
> `.rail.spine` 的裁切语义不变。移动款（`≤479px`）既有版面一字不动。

- **待办 dossier（右栏 360px，仅 pending 时出现）**：G4 的展开实现。2026-09-11 确认以
  **持久面板**替代 §12.7 原定的弹窗形态——面板随任务常驻、不遮挡内容区，看板侧以
  toast + 顶栏待办计数提醒。内容：
  阻塞原因（`pending_reason.message`）、按分支分组的动作按钮（含行内输入框）、
  「agent 为什么这么判断」= 触发节点的会话直达链接（§12.4.3 联动）、
  `merge_approval` 时内嵌 Diff 面板、产出文件入口。任务不再 pending 时该栏收起，
  内容区回到全宽。动作按钮的**确认步与三档量级**见 §9.3（决策 216）——不可逆动作
  就地走内联两步确认，跳过质量闸的动作是琥珀描边而不是实心主按钮。

### 6.2 五个页签

| 页签 | 内容 | 数据源 |
|---|---|---|
| 时间线 | `kanban_transitions` 渲染的流转记录，并行区间两条交错记录以 `∥` 分支徽标区分；trigger 用词表（normal/retry/kickback/user_resume…）原样展示；sync-check 的 join run 记录不进入时间线（决策 107），backtrack 呈现为进入 architect-design 的自动流转行 | `GET /tasks/{id}/flow` + SSE |
| 会话 | 按 stage/node/attempt 分组的会话列表 → 会话查看器（§12.4.3 ASCII 的实现体）：System Prompt 折叠块、消息气泡、tool 调用卡（工具名 + 参数摘要 + 结果行数）、submit_metadata 元数据卡；子代理（`agent_type` ≠ main）以缩进子会话呈现；失败 attempt 以红色分隔条标注原因；**进行中的 run 实时流式渲染** | `/conversations` + SSE 流 |
| 命令与输出 | 命令行表（`✓/✗ 时刻 source 命令 耗时 exit`），行展开见首尾预览与完整输出（卸载文件走 `/commands/{id}/output`）；`source = agent | system` 用徽标区分但共用一表（§12.4.4） | `/commands` + SSE |
| 产出文件 | design.md / dev-plan.md / test-scenarios.md / review-report.md / review-diff.diff / test-report.md，Markdown 渲染 | `GET /tasks/{id}/files/{path}` |
| Diff | merge proposal：DiffStats 摘要（文件数 / 增删行 / 逐文件明细）+ unified diff 渲染 + 审批动作（合入 / 返回修改，决策 23：无"拒绝"）；`base_commit` 过期时后端会重置 approval，前端在 diff 顶部提示「基准已前移，diff 重新生成中」 | stage_outputs + `/files/merge-proposal.diff` |

- 人工评审（`review_mode = human`）：pending(human_review) 的 dossier 呈现三件套——
  变更 diff（`review-diff.diff`，系统生成，决策 124）、agent 预审报告、单元测试结果，
  底部「通过 / 打回并附意见」（`POST /review`）。

---

## 7. 设置与全局指标

> **本节由决策 198 改写**：每块设置有自己的位置，不再挤在同一页里；入口的分类见 §4.3。

| 页面 | 要点 |
|---|---|
| 设置落地页 | `#/settings`：只做入口与分类（谁能进来 / 怎么跑），不承载设置内容（§4.3） |
| 项目 | 本地路径为唯一事实来源（决策 29）；创建后引导触发 `POST /projects/analyze`（伪阶段探测事实清单 + agent 摘要，决策 78），结果以核对清单呈现供确认；删除被拒时给的是**下一步**（「先去处理那 N 个任务」）而不只是一个理由，且这行只在动手时出现（决策 105；文案见 §12.1） |
| 手机访问 | 局域网扫码接入 + 运行时改绑 + 「这次绑定是谁定的」（决策 167 / 186 / 189 / 190）；**入口只在跑服务的这台机器本机渲染**（§4.3） |
| 模型与密钥 | **只剩 provider 台账与密钥提示**（阶段配置已搬去 `#/settings/stages`）：provider 行 = (vendor, model, context_window)（决策 111）；`api_key` 输入框写后即掩码回显 `***`（决策 112），旁边固定一行提示「密钥明文存于本机 `~/.agentpipeline`，目录权限 0700」；`supported_adapters` 之外的行降级灰显 + 告警，不崩（决策 103，界面文案见 §12.1） |
| 阶段配置 | `#/settings/stages`（**新页，决策 198**）：把「模型与密钥」页里的阶段配置那一段整体搬来——每个阶段用哪个 provider、带哪些工具与技能、超时覆盖（决策 111 / 170 / 172）；小节标题 12px 档，不与页面标题同级（票 09） |
| 技能市场 | `#/settings/market`：来源仓名单（保存即生效）+ 该仓的技能列表 + 安装后的三项预览（决策 187 / 194） |
| 全局指标 | GET /metrics：成功率、各阶段平均耗时 / 重试率 / validate 通过率、token 消耗。全部以**轨道分段条形图**呈现（横条挂在轨道站点下），延续"轨道即导航"；无 KPI 卡片横排。第一段用**平实说法**说清每个数是什么、怎么算的（票 27），不带内部编号（§12.1） |

---

## 8. 组件架构

```
frontend/
├── src/
│   ├── routes/            # Board / TaskDetail / Talk / Metrics / Share /
│   │                      #   SettingsLanding（新，决策 198）/ SettingsProjects / SettingsProviders /
│   │                      #   SettingsStages（新，决策 198）/ SettingsMarket
│   ├── api/               # 类型化客户端；TS 类型与 §4 数据模型一一对应（文档已是 TS interface）
│   ├── realtime/
│   │   ├── connection.ts  # EventSource 生命周期、退避重连、visibilitychange 处理
│   │   └── reduce.ts      # SSE 事件 → store 归约（按 branch 分拣，决策 84）
│   ├── stores/            # board.svelte.ts / taskDetail.svelte.ts / notifications.svelte.ts
│   ├── lib/               # 纯函数层：pipeline（几何/脊线）、actions、metrics、notificationPolicy、
│   │                      #   enterToSend（决策 184）、localPage（决策 190）、behavior-map.test.ts（§12.3）
│   ├── theme/             # 主题契约模块 + 对比度门（决策 195 的 contrast.ts）
│   └── components/
│       ├── ui/            # 公共非业务件：EmptyState（票 13）、Modal（三个对话框共用的键盘/语义底座）
│       ├── pipeline/      # PipelineRail（轨道，3 种变奏：脊线 / 卡片迷你轨 / hero）、CursorDot、BranchPill
│       ├── board/         # BoardColumn、TaskCard、PendingActions、StalledBadge、NewTaskDialog
│       ├── task/          # TimelineView、ConversationViewer、CommandLog、FileViewer、DiffReviewPanel、ReviewForm
│       ├── render/        # ★ 公共渲染件（§12.11 复用表）：MarkdownView、MessageBubble、
│       │                  #   ToolCallCard、MetadataCard、CodeHighlight、DiffView、TokenMeter
│       └── settings/      # ProjectForm（含 analyze 清单）、ProviderForm、StageConfigForm（阶段配置页用）
```

- `render/` 即 §12.11"外壳独立、渲染件复用"的落点：v1 会话查看器与 v2 对话窗口共用。
- `PipelineRail` 是全站唯一"重"组件：一份 DAG 拓扑数据（来自 petgraph 的静态形状，构建期内联）
  + 游标数组 → 三种密度变奏（脊线 120px / 迷你轨 16px / hero 200px）。
- token 计数组件 `TokenMeter` 唯一允许每秒多次重渲（流式累加），并做 `requestAnimationFrame` 合帧。

---

## 9. 状态与实时

### 9.1 数据流

```
GET /tasks?project_id=        → board 初始装载（决策 101，含分支级摘要）
GET /tasks/{id}/stream (SSE)  → 执行中 / pending 任务的实时更新（每任务一条，决策 76）
10s 对齐 tick 的轻量 refetch   → 兜底 waiting / queued 状态迁移（对齐 tick_interval_sec）
GET /tasks/{id}               → 详情页装载 + 断线重连后的全量校准（决策 76 唯一状态入口）
```

- board 打开时仅为 `status ∈ {running, pending}` 的任务开 SSE（数量受 `max_concurrent_tasks`
  约束，≤ 10 条连接）；其余状态靠 refetch。连接指数退避重连，`document.visibilitychange`
  恢复时立即校准一次。
- SSE 事件 → 归约表：

| 事件（§12.7） | board 归约 | 详情归约 |
|---|---|---|
| `node_started` / `node_finished` / `cursor_changed` / `stage_changed` | 列归属 / 站点信号色 / 迷你轨游标 | hero 状态 + 时间线追加 |
| `command_started` / `command_output` / `command_finished` | — | 命令表增行 / 追加输出 / 收尾 |
| `pending` / `pending_updated` | 顶栏计数 + 琥珀药丸（按 branch） | 打开 dossier，覆盖上下文 |
| `task_done` / `task_failed` / `task_cancelled` | 移列 + toast（按 NotificationPolicy） | 终态横幅 |
| `conversation_delta` / `tool_event`（决策 123） | — | 会话页追加消息 / 工具卡 / token 累加 |

- **通知策略实现**（决策 65）：toast 只对 pending / done / failed 弹（cancelled 不弹），
  同类 5 分钟 cooldown（`cooldown_sec`），22–8 免打扰（pending 豁免）；stalled 高亮走 SSE。

### 9.2 前端约束清单（来自主文档，实现时逐条对照）

- `allowed_actions` 纯渲染，每个 side_effect 必须有配对端点（决策 101/105）——前端不做动作白名单。
- 多游标 resume 必须带 `cursor_id`，省略仅在恰有一条游标时允许（决策 91）——按钮提交时从所属游标药丸取 id。
- pending 面板动作 = `continue / skip / goto`（resume 类）＋ 旁路动作（取消 / 拆分 / 换模型——决策 132 已把无端点的「放弃合入」「合并任务」移出动作集），
  两类按钮视觉分组，旁路动作弱化（决策 69/70）。完整映射见主文档 §5 的 **allowed_actions 权威总表**（决策 130）。
- 长耗时按钮异步 + loading 禁用（§12.11）；看板卡片禁拖（§12.11）。

### 9.3 动作的确认步与量级（决策 216）

**判据一句话：这一下之后，有没有东西在物理上回不去。** 有 → 走确认步；只是「走错要花时间」
→ 直接发。

| 类 | 在册动作 | 量级 | 确认步 |
|---|---|---|---|
| 推进 | `通过评审`（`approve`） / `continue`（带自由输入那个） / `goto` / `重试执行` / `重试合并` | 实心 `.btn.solid` | 无 |
| 写进项目仓库 | `合入`（`approve`@`merge_approval`） | 实心 | **有**：`确认合入到 {default_branch}？` |
| 跳过质量闸 | `skip`（`跳过本设计阶段` / `跳过当前阶段` / `强制进入下一阶段` / `强制通过评审`）+ 不带自由输入的 `continue`（`忽略失败依赖，继续执行`） | 琥珀描边（`--pending`） | **有**：`确认跳过评审闸门？` |
| 终结任务 | `cancel`（`终止任务` / `取消任务` / `取消本任务（其一）`） | 红描边 `.btn.danger` | **有**：`确认终止？任务会停在当前节点不再推进` |
| 让设备失效 | `重置配对` | 红描边 `.btn.danger` | **有**：`确认重置？{n} 台已配对的设备要重新扫码`（取不到 `n` 就不写数） |
| 弱化旁路 | `split_task` / `model_override` / `return`（`返回修改`） / `归档` / 上述之外的旁路 | `.btn.quiet` | 无（`拆分` / `换模型` 本来就要填表） |

- **怎么就认出「跳过质量闸」**（不靠标签文字匹配）：`kind === 'resume'` 且
  （`action === 'skip'` 或（`action === 'continue'` 且 `requires_input !== true`））。
  理由：`skip` 的语义就是「不看本阶段产出直接放行」（决策 69 / 130），而 `continue`
  只有**带自由输入**那一支是「把缺的信息补上再继续」——那是推进，不是跳闸。
  `goto` 一律属推进：它把任务送回某个阶段重做，不跳过任何东西。
- **形态 = 内联两步，不是模态**：动作行就地换成 `确认…？` + **同一颗钮**（保留原词与原量级，
  进 `busy` 才禁用）+ 紧邻一颗 `取消`（`.btn.quiet`）。沿用删项目 / 删 provider 的既有写法
  （`SettingsProjects.svelte` 的 `confirmingDelete`），理由见决策 216②。
- **后果句只在动手那一步出现**，常驻处不摆（每个动作都挂一句会把坞顶满）；与决策 105
  「理由只在动手时出现」同源。
- **键盘**：确认态不移动焦点（焦点仍在刚点的那颗钮上，再按一次回车即确认）；`Escape`
  从确认态退回普通态；`取消` 不进默认焦点。
- **打回**不强制非空意见，但标签随输入变：空 → `打回开发`，非空 → `打回并附意见`。
  它**不进确认步**（不跳过闸门，可再走一遍）。

### 9.4 中流状态的地址与本地留存（决策 217）

| 状态 | URL | localStorage | 谁写地址 |
|---|---|---|---|
| 详情页签 | `#/task/{id}?tab=timeline\|conversation\|commands\|files\|diff`（缺省 `timeline` **不写**） | — | 用户点页签 = `pushState` |
| 看板过滤 | `#/?filter=all\|running\|pending\|…`（缺省 `all` 不写） | `agentpipeline.board_filter`（URL 里没有时兜底） | 用户切过滤 = `pushState` |
| 对讲台班次 | `#/talk?session=42` | `agentpipeline.talk_session`（URL 里没有时兜底） | 用户换班次 = `pushState` |
| 对讲台输入草稿 | **不进** | `agentpipeline.talk_draft`（`{sessionId, text, at}`） | — |
| 会话页签里选中的 run | **不进** | **不进** | — |

- **「我在哪」进 URL，「我平常怎么用」与没写完的草稿进 localStorage。** 草稿不是位置：
  把半句话塞进地址，分享出去的是一个别人看不懂的 URL，而地址栏还会在打字时被反复改写。
- **程序改地址一律 `replaceState`**（触发节点直达、打开产出文件、`?task=` 自动就位、
  `?project=&analyze=1` 自动触发）——否则自动联动会把历史灌满，后退不再是「回到上一页」。
- **刷新恢复**：地址里有就照地址；没有就用缺省，**不拿 localStorage 去覆盖**——过滤与班次
  是两个例外（跨页面的工作语境：从看板点进任务再点「← 看板」回来时地址会丢参数，
  而「我一直在看 pending」不该因此被重置）。取值非法（枚举外 / 库里已不存在）回落缺省**并删键**。
- **清理**：草稿发送成功后立刻清零，装载时若 `at` 早于 7 天也清零；不新增「清空本地状态」界面
  （theme / project / pairing 各有各的复位处，多一个总闸只多一个误触面）。
- **上界**：只允许这几类短枚举参数（连同既有 `task=` / `project=` / `analyze=`）；
  参数总数 ≥5 或出现自由文本时回来重新裁决。地址是给人看、给人抄的，不是状态垃圾桶。

---

## 10. 与后端契约的差距（需在实现前排掉）

| # | 差距 | 建议 |
|---|---|---|
| ① | SSE 事件表（§12.7）没有**会话流式输出**的事件类型，但 §12.11 要求 agent 文本 / 工具调用实时流式 | 在 `/tasks/{id}/stream` 增补 `conversation_delta`（`run_id` / `agent_type` / `branch` / `role` / `text`）与 `tool_event` 两类事件，落库仍走现有会话 API。**已采纳（决策 123）**：§12.7 已增补，本表差距①排掉 |
| ② | 看板只有一个 per-task SSE；board 的多任务实时性只能靠"活跃任务逐条开流 + tick 对齐 refetch"凑 | v1 照此实现（够用）；v1.1 可加 `GET /stream?project_id=` 全局通道作为升级路径，`reduce.ts` 归约表不变。**g6 确认照此实现** |
| ③ | 人工评审需展示"变更 diff"，但 `merge-proposal.diff` 在 merge 阶段才生成 | review 阶段由系统生成一份 develop 产出 diff（或复用命令表里的 `git diff` 记录），经 `/files/{path}` 下发。**已采纳（决策 124）**：系统生成 `review-diff.diff`，本表差距③排掉 |
| ④ | 成本以 token 计；"¥"换算需要 provider 价目 | v1 只显示 token 与调用次数（原型已同步为 token-only）；价目进 providers 表后再开金额。**g6 确认**：主文档 §12.4.2 的 ¥ mock 一并移除（决策 131） |
| ⑤ | 流式 token 计数事件未定义 | 随 ① 的 `conversation_delta` 带 `prompt_tokens` / `completion_tokens` 增量。**已采纳（决策 123）** |

---

## 11. 实施顺序

1. **骨架**：token 体系 + `PipelineRail`（三变奏）+ 路由 + API client（类型从 §4 直拷）。
2. **看板**：`GET /tasks` 装载 + 卡片 + 列语义 + 过滤 + 新建任务对话框；无 SSE 也能用（refetch）。
3. **任务详情**：hero 轨道 + 时间线 + 会话查看器（`render/` 组件就位）+ 命令表 + 文件。
4. **实时**：SSE 连接层 + 归约表 + 异步按钮 + 通知策略；pending dossier 与待办计数。
5. **Diff 审批与人工评审** → 6. **设置页与全局指标**（含决策 198 的落地页与阶段配置页）→
   7. 打磨：空态、reduced-motion、键盘可达、断线重连横幅、文案规范（§12）。

---

## 12. 文案规范与可追溯性（决策 199 / 200）

> 本节是**新增的**（原文没有文案规范）。三条规则指向同一件事：界面上的字说的是
> 「会发生什么、你该做什么」，而维护者要的那条「这条规矩从哪来」不占使用者的注意力。

### 12.1 内部决策编号退场（决策 199）

- **面向用户的正文里不再出现「决策 NN」。** 正文只留**动作与后果**：会发生什么、你该做什么。
- 编号的去向**只有两处**：元素的 `title` 悬停提示，或（一条说明确实需要给出理由时）就地折叠的
  `<details>` 说明。**同一处只选一种**，不叠加、不并排。
- **例外照旧**：代码注释、开发文档、测试文件里的编号**不动**——机器门只扫**面向用户的文案**。
  落到 `.svelte` 上有一条容易踩的判据：**`<!-- … -->` 是注释，不是文案**（`frontend/src/routes/Talk.svelte`
  里现有五处「决策 NN」全在 HTML 注释里，门若按纯文本搜就会把它们误判成文案）；**算文案的是**：
  标签之间的文本、`title` / `aria-label` 这类属性值、以及喂给它们的 JS 字符串常量。

**定稿例子（现文案 → 新文案，可直接抄）**

| 位置 | 现文案 | 正文（定稿） | 编号去向 |
|---|---|---|---|
| 设置·项目，删除被拒 | `该项目有 3 个活跃任务，不能删除（决策 101）。` | `先去处理那 3 个任务，然后再删除这个项目。` | `title="有活跃任务的项目不能删除"` |
| 设置·模型与密钥，不受支持的行 | `! 不受支持 · 决策 103` | `! 不支持这个厂商，该行已停用` | `title="适配器不支持的 provider 行降级停用；被阶段引用时启动会拒绝"` |
| 设置·模型与密钥，密钥提示 | `密钥明文存于本机 ~/.agentpipeline，目录权限 0700` | **逐字保留**（本来就没有编号，不属于本项） | — |
| 指标页第一段 | `统计口径见 core metrics（决策 130 / 137）：成功率 = done ÷（done+failed+cancelled）…` | **整段重写归票 27**；本项只保证重写后的那段不带编号、不说字段名 | `title` 或折叠说明给「这几个数出自哪几条决定」 |
| 任何说明行里的括注 | 形如 `（决策 N）` 的括注 | 换成一句**理由**，或直接删掉括注 | 编号移进该元素的 `title` |

**界面上的编号出现点（初版清单，票 23 直接用；归属按 `parallel-brief.md` §二的所有权表）**

清单是**初版**：行号会随并行改动漂移，**按「决策 N」搜一遍再核**；`title` 里的编号与正文里的
同样算（它们都是使用者看得到的字）。真正的兜底是机器门 `frontend/src/lib/copy-discipline.test.ts`，
这张表只是把「要改哪些处」一次说清。

| 出现点 | 现文案（片段） | 正文（定稿） | 归属 |
|---|---|---|---|
| `frontend/src/routes/SettingsProjects.svelte:177` | `本地路径是项目唯一事实来源（决策 29）。创建时立即校验 git 仓库（决策 61）；删除有活跃任务的项目会被拒绝并给出原因（决策 101）。` | `本地路径是项目唯一事实来源。创建时立即校验是不是 git 仓库；删除有活跃任务的项目会被拒绝，并告诉你先去处理哪几个任务。` | S |
| `frontend/src/routes/SettingsProjects.svelte:221` | `该项目有 {active} 个活跃任务，不能删除（决策 101）。` | `先去处理那 {active} 个任务，然后再删除这个项目。`（这行**只在动手时出现**，不再常驻——票 04） | S |
| `frontend/src/routes/SettingsProjects.svelte:253` | `title` 里的 `该项目有 {active} 个活跃任务，不能删除（决策 101）` | `title="有活跃任务的项目不能删除"` | S |
| `frontend/src/routes/SettingsProviders.svelte:256` | `provider 行 =（vendor, model, context_window）（决策 111）。api_key 明文存储，读接口只回显 ***；` | `每行一个 provider，写明厂商、模型与上下文窗口。密钥明文存储，读接口只回显 ***；` | S |
| `frontend/src/routes/SettingsProviders.svelte:257` | `密钥明文存于本机 ~/.agentpipeline，目录权限 0700（决策 112 / §12.14）。` | `密钥明文存于本机 ~/.agentpipeline，目录权限 0700。` | S |
| `frontend/src/routes/SettingsProviders.svelte:297` | `! 不受支持 · 决策 103` | `! 不支持这个厂商，该行已停用` + `title`（见上「冲突与处置」） | S |
| `frontend/src/routes/SettingsProviders.svelte:360` | `阶段 provider 优先于全局默认（决策 129）。…被拒并回显原因（决策 47 / 103）。` | `阶段配置优先于全局默认。…被拒并回显原因。`；**这一段随决策 198 搬去 `#/settings/stages`**（N 建页，S 从本页摘掉） | S + N |
| `frontend/src/routes/SettingsMarket.svelte:532` | `要启用请到「设置 · 模型与密钥」的阶段配置里声明` | `要启用请到「设置 · 阶段配置」里声明`，链接改指 `#/settings/stages`（决策 198 之后的正确落点） | S |
| `frontend/src/routes/SettingsMarket.svelte:534` | `工具按需拉取，决策 181⑤）。` | `工具按需拉取）。` | S |
| `frontend/src/routes/Share.svelte:206` | `（跨源防护只拦异源写请求，同源写请求自带客户端头，决策 128 / 153③）。` | `（手机加载的页与接口同源，不需要额外放行来源。）` | S |
| `frontend/src/routes/Share.svelte:222` | `启动时指定的绑定优先于这里的按钮（决策 186）：…` | `启动时指定的绑定优先于这里的按钮：…` | S |
| `frontend/src/components/settings/ProjectForm.svelte:85` | `…且 HEAD 已有提交（决策 29 / 61）。校验失败会原样回显后端拒绝原因。` | `…且 HEAD 已有提交。校验失败会原样回显后端拒绝原因。` | S |
| `frontend/src/components/settings/ProviderForm.svelte:109` | `…被 stage_configs 引用时配置加载会拒绝启动（决策 103）。` | `…被阶段配置引用时配置加载会拒绝启动。` | S |
| `frontend/src/components/settings/StageConfigForm.svelte:181` | `默认关闭——每次尝试干净对话（决策 33 / 180）。` | `默认关闭——每次尝试干净对话。`（该表单随决策 198 进 `#/settings/stages`） | S |
| `frontend/src/components/settings/AnalysisChecklist.svelte:34` | `以下为 project_analysis 探测到的事实，确认无误后即可创建任务（决策 78）。` | `以下是探测到的事实，确认无误后即可创建任务。` | S |
| `frontend/src/routes/Metrics.svelte:81` | `统计口径见 core metrics（决策 130 / 137）：成功率 = done ÷（done+failed+cancelled）…` | **整段重写归票 27**；重写后不带编号、不说字段名 | Me |
| `frontend/src/routes/Metrics.svelte:141` | `已从轨道图排除非站点阶段（决策 107）：{…}。` | `已从轨道图排除非站点阶段：{…}。` | Me |
| `frontend/src/routes/Metrics.svelte:179` | `stored_* 是任务表持久化值，与按 run 求和存在差异（决策 100 的父/子行口径或未落库更新）。` | `这个数与按执行记录逐个加起来的结果有出入（父子行的口径不同，或者还没落库）。` | Me |
| `frontend/src/components/task/SplitDialog.svelte:41` | `原任务将被置为 cancelled（决策 105）。` | `原任务会被置为已取消。` | M |
| `frontend/src/components/task/ModelOverrideDialog.svelte:38` | `仅影响本任务后续节点（决策 105 / 129），不改全局 stage config。` | `只影响本任务后面的节点，不改全局阶段配置。` | M |
| `frontend/src/components/board/PendingActions.svelte:137` | `title` 里的 `无配对端点（决策 101）` | `title="这个动作没有配对的端点"` | **未指派**（`components/board/` 只把 BoardColumn / TaskCard 给了 B） |
| `frontend/src/components/board/StalledBadge.svelte:10` | `title` 里的 `pending 超过 pending_timeout_hours（决策 34）` | `title="等你拍板已经超过超时上限"` | **未指派**（同上；请编排者指派或走 handoff） |

**冲突与处置（写死）**

- 现行视觉规格 `design/theme-6-pixel.md` 的组件映射表里有一条**明文要求把编号渲染进界面**：
  `| 行内降级 | 决策 103 的"不受支持 vendor"整行 --t4 灰显 + ! 不受支持 · 决策 103 琥珀标；… |`
  （审计报告记的位置是 `:251`；T 的 §2.6 / §2.7 增补已把它位移，**以文本匹配为准**）。
- 它与本节规则**直接冲突**，属于**要改写的那一处**：那处文案改成 `! 不支持这个厂商，该行已停用`，
  编号退到 `title`；改写 + 在原处标注修订来由**由 T 在视觉规格里执行**（本文不碰视觉规格——
  它是 T / DEC-VIS 的文件）。交接见 `.scratch/ux-audit/handoff/DEC-IA-22.md`。
- 之所以不是「修 bug」：那条要求是**决策 169 把主题六升格为现行视觉规格**时带进来的，
  故按本项目惯例**追加修订决策（199）并在被改写的原文处标注**，不静默改。

### 12.2 车间隐喻首现翻译（决策 200）

**口径：隐喻保留（它是整个主题的投资）；每个词在「每个页面」内首次出现处给一次平实说法；
同一页面内不重复。**

- 为什么按页面而不是按全站：本项目是 hash 路由的本地应用，**任何页面都能被直接打开**
  （手机扫码直接落看板、地址栏贴任务详情）——「全站只译一次」会让从别处进来的人永远看不到翻译；
  而「同一页面内不重复」保证翻译不变成噪声。
- 形态定稿：**行内、全宽括号、紧跟在词后**——`急停（等你拍板的阻塞）`。词与翻译**同字号、同色档**；
  翻译用 `--text-3`（决策 195 的「次级必读」档，门槛 4.5:1，读得到）。
  **不新增图元、不新增颜色、不加独立徽章、不加背景、不加动画位**；2px 描边、零圆角、
  12 的倍数字阶一字不动。
- **`title` 不承载翻译**（那是编号的位子）：译文必须是**屏上读得到**的字，悬停不算。
- **按钮与标题里不翻译**：可执行物（按钮）与标题保持原词，翻译只出现在**说明性文字**里。
- **第一屏与隐喻词的分界**：顶栏四项名、「新建任务」、状态过滤槽的词（§4.4）、空态的下一步
  （§5.3）**一律用平实词、不翻译**（票 21）；翻译只服务**其余正文**。
- 词表与译法的同步落点：`docs/glossary.md` 的「视觉语汇」一节（按同样的说法填一列），
  两处必须一致。
- **「急停」的首现落点在 `≤899px` 一档搬过位置（决策 218 ⑦b）：**那一档「一张急停都没有时
  状态区整块退场」，原先承载这个译文的空态因此不在了；译文随之**搬到摘要条自己的琥珀标签**
  （`⏸ 急停（等你拍板的阻塞）· 合并审批`，只在本页**第一张**急停上给括号，页面内仍不重复）。
  译文跟着词走：有急停则词在译文在，没有急停则词不在、也就没有「没被翻译的词」——与本页既有的
  「`工位` 的译文只落在桌面那一行、窄屏该词不出现故没有漏译」是同一条手法（口径不变，换的只是落点）。

| 隐喻词 | 首现处的定稿平实说法（可抄进界面） |
|---|---|
| **急停** | `急停（等你拍板的阻塞）` |
| **值班长** | `值班长（跟我对话的 AI）` |
| **工位** | `工位（流水线的阶段）` |
| **货箱** | `货箱（一张任务卡）` |
| **工头** | `工头（就是值班长，跟我对话的 AI）` |
| **值班经理** | `值班经理（你）` |
| **对讲台** | `对讲台（跟值班长说话的地方）` |
| **传送带 / 链节** | `传送带（这条流水线的顺序）` |
| **信号灯** | `信号灯（任务的状态色）` |
| **回流带** | `回流带（打回重做的那条线）` |
| **台账** | `台账（设置这一类页面）` |
| **道具栏** | `道具栏（顶栏那排状态过滤）` |

### 12.3 行为 / 规则 → 实现位置（决策 199）

**这张表是「决定 → 实现」的唯一权威索引。** 界面正文不再出现编号，编号住在**本表的「备注」列**
（以及代码注释与开发文档里）；维护者改代码前从这里查「哪几条决定约束着它」。表与视觉规格既有那张
「视图 → 组件」表**同形**（三列），**不新建文件**。

**「实现位置」列的机器可解析格式（定稿；加行必须照此写，否则 `behavior-map.test.ts` 变红）**

1. 每条位置**用反引号包裹**：`` `frontend/src/routes/Board.svelte` ``。
2. 多条之间用**顿号 `、`** 分隔。
3. 路径一律**相对仓库根**、用 `/`；**行号可选**，写成 `:行号`（如 `frontend/src/lib/pipeline.ts:120`）。
   **行号只作定位辅助、不参与断言**——断言的是**文件存在**，行号漂移不会变红。
4. **不写 glob**（`*`）、不写目录、不写锚点、不写仓库外的路径、不引用 `.scratch/` 下被 gitignore 的产物。
5. 表里可以引用**本 effort 正在新建的文件**：`SettingsLanding.svelte` / `SettingsStages.svelte` /
   `Modal.svelte` / `contrast.ts` / `copy-discipline.test.ts` 五个，由检查脚本登记为「允许尚未落地」，
   **落地后逐条删掉**；除这五个之外，每一行引的路径**必须现在就在磁盘上**。

**悬空引用检查**：`frontend/src/lib/behavior-map.test.ts`（vitest，纯静态扫描）逐行解析下表，
断言每条位置在磁盘上存在；**指不到实现位置的条目变红**，失败信息说清「哪一行（行为列的文本）
指向哪个不存在的路径」。索引最大的失败形态不是没建，是建完之后悄悄烂掉——这次的对比度规则
就是这么没的。

| 行为 / 规则 | 实现位置 | 备注 |
|---|---|---|
| 动作集纯渲染：前端不做白名单，每个 side_effect 有配对端点 | `frontend/src/lib/actions.ts`、`frontend/src/components/board/PendingActions.svelte` | 决策 69 / 101 / 105 |
| 长耗时按钮异步：点击即 loading 禁用，SSE 回执后复位 | `frontend/src/lib/actionSubmit.ts` | 决策 69；§9.2 |
| 多游标 resume 必须带 `cursor_id`（省略仅在恰有一条游标时允许） | `frontend/src/lib/actionSubmit.ts` | 决策 91 |
| 列 = 阶段：含并行双轨合并列，sync-check 全站不展示 | `frontend/src/lib/pipeline.ts`、`frontend/src/components/board/BoardColumn.svelte` | 决策 92 / 107 |
| 并行分支按 branch 分拣与着色，动作集按游标独立下发 | `frontend/src/realtime/reduce.ts`、`frontend/src/components/pipeline/BranchPill.svelte` | 决策 84 |
| 脊线站点数字 = 累计到过这一站，框内带 `累计` 词（与列头存量区分） | `frontend/src/components/pipeline/PipelineRail.svelte`、`frontend/src/lib/pipeline.ts` | 决策 197 |
| 看板溢出：宽屏 `merge` / `done` 钉右；窄档初始滚动落在 `merge` + 右缘「还有 N 列」 | `frontend/src/routes/Board.svelte` | 决策 196 |
| 卡片禁拖；任务卡整卡可点进详情 | `frontend/src/components/board/TaskCard.svelte` | 决策 169 沿用的交互骨架；§5.2 |
| 页面导航行四项（对讲台 / 看板 / 指标 / 设置），其余入口从落地页进；**看板只从这一行进**（窄档这一行钉在屏幕底缘，桌面档是顶栏第二行——同一个 `nav` 元素） | `frontend/src/components/layout/TopBar.svelte`、`frontend/src/router.svelte.ts` | 决策 240（修订 198 / 169）；位置由 243 定（窄档在底部） |
| 首屏默认落对讲台：地址栏没写 hash 就 `replaceState` 归一到 `#/talk`（带 hash 的开屏与显式 `#/` 都不动） | `frontend/src/router.svelte.ts` | 决策 241（不修订 240）；`router.test.ts` 的「开屏默认落点」用例 |
| 设置落地页按用途两分（谁能进来 / 怎么跑），各项仍是独立路由 | `frontend/src/routes/SettingsLanding.svelte` | 决策 198 |
| 「手机访问」入口只在本机（来源回环）渲染，非本机不给入口 | `frontend/src/lib/localPage.ts`、`frontend/src/routes/SettingsLanding.svelte` | 决策 190（位子由 198 挪到落地页，行为不变） |
| 阶段配置独立成页，从「模型与密钥」页搬出 | `frontend/src/routes/SettingsStages.svelte`、`frontend/src/components/settings/StageConfigForm.svelte` | 决策 198 / 111 / 170 |
| 不支持的 provider 行降级灰显 + 琥珀标（**标里不带内部编号**） | `frontend/src/routes/SettingsProviders.svelte:297` | 决策 103；决策 199（编号退到 `title`） |
| 状态过滤槽 = 图标 + 词（词取 `FILTER_LABELS`），窄屏只给当前项带词 | `frontend/src/components/layout/TopBar.svelte`、`frontend/src/stores/board.svelte.ts` | 决策 201 |
| pending 数的唯一显示位是「待处理 N」芯片（槽位不再重复这个数） | `frontend/src/components/layout/TopBar.svelte`、`frontend/src/components/layout/StatusLine.svelte` | 决策 92 / 201 |
| 正文不出现内部决策编号（机器门扫面向用户的文案） | `frontend/src/lib/copy-discipline.test.ts` | 决策 199 |
| 对比度门：`--text-3` ≥ 4.5:1（次级必读）；`--text-4` 豁免且不得承载必读信息 | `frontend/src/theme/contrast.ts`、`frontend/src/theme/contract.ts` | 决策 195 |
| token 值与全局样式表互为镜像，逐值双向比对，禁裸十六进制颜色 | `frontend/src/theme/css-parity.test.ts`、`frontend/src/app.css` | 决策 169 |
| 三个模态框：Escape 一律可关、焦点进第一个输入框并关在框内、对话框可被播报 | `frontend/src/components/ui/Modal.svelte`、`frontend/src/components/board/NewTaskDialog.svelte`、`frontend/src/components/task/SplitDialog.svelte`、`frontend/src/components/task/ModelOverrideDialog.svelte` | 决策 169 的交互骨架；票 02 |
| 404 页给一条回看板的路（不留无路可走的死地址） | `frontend/src/App.svelte` | 决策 79 的 v1 范围；票 03 |
| 删除被拒时才说下一步，不再常驻红字 | `frontend/src/routes/SettingsProjects.svelte` | 决策 105；票 04 |
| 依赖任务 ID 用原生 `datalist`（不做完整选择器） | `frontend/src/components/board/NewTaskDialog.svelte` | 决策 79；票 05 |
| 任务级指标入口：`#/metrics?task=<id>` 从任务详情进入，不要求手打 ULID | `frontend/src/routes/Metrics.svelte`、`frontend/src/routes/TaskDetail.svelte`、`frontend/src/lib/metrics.ts` | 决策 130 / 137；票 06 |
| 项目分析入口 `#/settings/projects?project=<id>&analyze=1` | `frontend/src/routes/SettingsProjects.svelte`、`frontend/src/lib/analysis.ts` | 决策 78；票 07 |
| 空态 = 状态 → 下一步 → 可选入口；提到另一个页面必须可点 | `frontend/src/components/ui/EmptyState.svelte` | 票 13 |
| pending dossier 不重复渲染 diff；动作行保留（多处 e2e 依赖它） | `frontend/src/routes/TaskDetail.svelte`、`frontend/src/components/task/DiffReviewPanel.svelte` | 决策 23（无「拒绝」）；票 08 |
| 输入法护栏：回车提交要挡「用回车确认候选词」的那一次 | `frontend/src/lib/enterToSend.ts` | 决策 184 |
| 工头（值班长）的回复里永远没有按钮；时间线上唯一的钮是操作台在提议轮里的确认钮 | `frontend/src/routes/Talk.svelte`、`frontend/src/components/task/PendingDossier.svelte` | 决策 176 / 182 / 207③ |
| 提议轮的渲染判据：过期按 `expires_at` 自己算、过期只变灰而轮仍在、同一个动作已在下发的动作集里就只指路 | `frontend/src/lib/proposals.ts` | 决策 188 / 207；`proposals.test.ts` 逐条钉住 |
| 确认钮按下走既有端点（不新增改状态的路）；成功失败都回灌成一轮，不弹窗不 toast | `frontend/src/routes/Talk.svelte`、`crates/app/src/routes/foreman.rs` | 决策 188 / 207② |
| 权限档位（环境层 auto / ask / deny）是配置项，不是代码常量：阶段配置表单里可改 | `frontend/src/components/settings/StageConfigForm.svelte`、`frontend/src/lib/stageConfigs.ts` | 决策 206；档位在值班长那一行的缺省是 `ask` |
| 对讲台急停轮折叠：两张以上一张都不展开；窄屏改「摘要条 + 输入坞」 | `frontend/src/lib/talkStops.ts`、`frontend/src/routes/Talk.svelte` | 决策 183 / 192；**折叠判据的档位由决策 218 ⑥ 修订**——`≤899px` 起**单张也折**，且那一档摘要条不画名牌、没有急停时整块退场（`forceFold` 的断点在 `frontend/src/lib/talkLayout.ts` 一处） |
| 对讲台折行档（`≤899px`）的页头带子与 ⋯ 班次菜单：`<h1>` 转 visually-hidden、行内 ≥44px 命中区、`aria-expanded`/`aria-controls`、Escape 与方向键能进出、点外关得掉、当前班次是身份行（`aria-current`）不是按钮 | `frontend/src/routes/Talk.svelte` | 决策 218（Q10 / Q12 / Q14）；三条等价断言（⋯ 可点、菜单开得出来、当前条被标出）在 `frontend/e2e/talk.spec.ts` |
| 折行档的断点只有一处定义：JS 的 `forceFold` / `matchMedia` 与 CSS 字面量由静态扫描对着钉 | `frontend/src/lib/talkLayout.ts` | 决策 192 / 215 / 218；`talkLayout.test.ts`（node 环境静态扫 `Talk.svelte`）钉住只有 479 / 899 / 1099 三个断点 |
| 班次落点进 URL（`#/talk?session=<id>`）+ localStorage 兜底、程序改地址一律 `replaceState`、非法值回落并删键 | `frontend/src/router.svelte.ts`、`frontend/src/lib/talkSessions.ts`、`frontend/src/routes/Talk.svelte` | 决策 217③④、218；`router.test.ts` 的 `readQuery` / `writeQuery` 用例 |
| 回话中允许换班次（那把「发送中禁止切换」的 UI 锁已撤），「这一轮回话去哪了」由两枚标记接手 | `frontend/src/routes/Talk.svelte`、`frontend/src/lib/talkSessions.ts` | 决策 220②⑤（修订决策 204③ 的既有自保「回话中先别换班次」） |
| 班次列表两枚标记（每条最多一枚）：**正在回话**（本机发出未落地 ∪ SSE 增量带别的 `session_id`；静默 20s / **落地** / 断流三条收口）/ **有新动静**（`last_active_at` 晚于本机记的看过时刻；当前班次永不算；基线**只在本机一条记录都没有时**立）；回话中优先于有新动静 | `frontend/src/lib/talkSessions.ts`、`frontend/src/realtime/foreman.ts`、`frontend/src/routes/Talk.svelte` | 决策 220①③④ / 222；派生口径由 `talkSessions.test.ts` / `foreman.test.ts` 钉住，跨设备那一组在 `frontend/e2e/talk.spec.ts` |
| 工位回执在折行档默认收起（`<details>` 不写 `open`，内容一个字不删），人手动展开后不被流式增量打回 | `frontend/src/routes/Talk.svelte` | 决策 218 ②（修订决策 182 的「默认展开」在那一档的纪律）；`frontend/e2e/talk.spec.ts` 把 POST 拖住 2.5s 验「不打回」 |
| 输入坞不常驻提示语；传输层断线（`streamStatus === 'error'`）时才有那一行，且是全页**唯一**的断线告知（空闲 88px） | `frontend/src/routes/Talk.svelte` | 决策 218 当日修订②、220④；「值班长正在回话…」那半句已删（流式尾随光标已在说） |
| 窄档（≤479px）**页面导航行钉在屏幕底缘**（`position: fixed; bottom: 0`，四项等分、图标上文字下、页签 `height: 44px`、`padding-bottom: var(--safeb)` 独占安全区、`z-index: 32`）、**铭牌行整行 `display:none`**、**道具栏行只在看板路由露出**（`class:on-board` 判据，**不叫 `.board`**——那是看板页容器的类名）且自带下框；高度按路由两档（看板 52px / 其余 **0px 清零**），下游钉位一律读 `--topbar-h`（**0 也写入**）；状态条 `bottom: var(--nav-h)` 叠页签栏上方，底部让位读 `--sbar-h`（= 42 + `--nav-h`，两层账本） | `frontend/src/components/layout/TopBar.svelte`、`frontend/src/app.css`、`frontend/src/components/layout/StatusLine.svelte` | 决策 243（修订 242 的 ② 导航行位置与高度两档；242 的铭牌行退场、`class:on-board` 判据照旧）；e2e 钉在 `frontend/e2e/pixel-theme.spec.ts`（52 / 58 / 页签贴底缘 / 状态条贴页签上沿 / `scroll-margin` 62）与 `frontend/e2e/talk.spec.ts`（非看板顶栏 0） |
| 「急停」的首现平实说法落在急停摘要条的琥珀标签上（折行档没有急停时那整块退场、词与译文一起不在） | `frontend/src/routes/Talk.svelte` | 决策 200（口径不变）＋ 218 ⑦b（换落点） |
| 值班长没回话的那一轮渲染成失败轮**并显示原因**（后端落的 `system` 账以 `【没跑起来】` 开头），不再是一条只有红轮、无处看原因的静默失败 | `frontend/src/routes/Talk.svelte` | 决策 211④；票 04 |
| 本地等不到回包**不等于**这一轮失败：超时那一类补一句「它在服务端仍在继续」（回话会随流式增量到达，切走再切回本班次也能看到），其余失败照原样说 | `frontend/src/realtime/foreman.ts`、`frontend/src/routes/Talk.svelte` | 决策 223；判据由 `frontend/src/realtime/foreman.test.ts` 钉住（`failureNotice` / `isTimeoutMessage`） |
| 主动播报与回话是两种轮：值守轮自己醒来说的那条名牌写「值班长 · 值守」（后端加的 `【值守播报】` 标记），有人问才说的话仍是「值班长」 | `frontend/src/routes/Talk.svelte` | 决策 209④；票 06 |
| 修复提议的渲染与工具调用不同：显示闸门读数（过了哪几步 / 没过则明说「没有补丁」）与可展开、可复制的 diff；按钮文案是「合入」而不是「执行」 | `frontend/src/routes/Talk.svelte`、`frontend/src/lib/proposals.ts` | 决策 212①；票 12（同一个钮面，不新开第三个） |
| 任务级托管：状态在任务上看得见，且拨得动；终态任务与值班长未接线时不摆那颗钮（两条理由与端点的两种拒绝同一份） | `frontend/src/lib/stewardship.ts`、`frontend/src/routes/TaskDetail.svelte`、`frontend/src/api/types.ts` | 决策 210①；票 08 的端点 / 票 14 的界面；判据由 `stewardship.test.ts` 逐条钉住 |
| 任务停在 pending 时，说明那一句里看得出它在**等修复合入**（原句「为什么停」不丢、`PendingKind` 也不换——它还参与 resume 的原因归类） | `crates/core/src/pipeline/repair.rs`、`crates/core/src/storage/tasks.rs` | 决策 210⑨；票 11 的最后一格 |
| 宽屏矮窗口：状态区上限取「46vh」与「先留给时间线的那一份」的较小者——时间线恒有 160px 下限，确认钮不被挤成一条缝 | `frontend/src/routes/Talk.svelte` | 决策 208；四条几何断言在 `frontend/e2e/talk.spec.ts` 的 `expectProposalReachable` |
| 对讲台的班次 chip 行：桌面挂在**页头那一行右端**（非 sticky、不动页头与顶栏，`nowrap` + 容器内横滚），`≤899px` 收进页头右端的 ⋯ 菜单（页面上没有 chip 行） | `frontend/src/routes/Talk.svelte` | 决策 204③（落点由决策 218 Q15 从「时间线里」改到页头右端）；**三条几何约束已作废**——决策 218 Q15 的原始动机正是「它长在滚动容器里，滚到底时实测在屏幕上方 320.2px」，搬出来之后桌面白得 36px |
| 换班次重置的是对话上下文，看板派生的东西（急停 / 值班板）一样不动 | `frontend/src/routes/Talk.svelte` | 决策 204；`resetSessionState()` 与它旁边那份「不重置」清单 |
| 值班长的增量按会话身份归位；发送窗口里换了班次则不落地 | `frontend/src/realtime/foreman.ts`、`frontend/src/routes/Talk.svelte` | 决策 204 |
| 技能市场：仓名单保存即生效；装前预览三项（去向 / 模式与信任态 / 特征扫描） | `frontend/src/routes/SettingsMarket.svelte`、`frontend/src/components/settings/StageRecommendations.svelte` | 决策 187 / 194 / 181 |
| 未受信任的技能不得以全文模式保存（界面上就地改写信任态） | `frontend/src/components/settings/SkillDeclList.svelte`、`frontend/src/lib/stageConfigs.ts` | 决策 172 / 181 |
| 推荐行三态：未装给「安装」、已装而本阶段未声明给「启用」、已装且本阶段已声明只读——**判据是「本阶段是否声明」，不是「是否安装」** | `frontend/src/components/settings/StageRecommendations.svelte` | 决策 214①；票 16 第 14 行那条验收的界面落点（票 01）；判据 `config::skill_declared_in_stage` |
| 推荐行标出来源：`owner/repo · 技能目录`，是**指针**不带 commit，两段取不到就只省掉这一段（推荐行照旧在） | `frontend/src/components/settings/StageRecommendations.svelte` | 决策 194 / 214④；票 02 |
| 「手机访问」取不到配对令牌就不画二维码 | `frontend/src/routes/Share.svelte`、`frontend/src/lib/sharePairing.ts` | 决策 189 |
| 绑定开关只由回环来源发起；界面说出「这次绑定是谁定的」 | `frontend/src/routes/Share.svelte`、`frontend/src/lib/lanToggle.ts` | 决策 186 |
| 端口不是配置里那个（被别的程序占着，退让到临时端口）时，分享页说出「这次为什么变了」 | `frontend/src/routes/Share.svelte`、`frontend/src/lib/sharePairing.ts` | 决策 213；判定在 `portFallbackNote`，只绑回环时不说 |
| 通知策略：toast 只对 pending / done / failed 弹，同类 5 分钟 cooldown，22–8 免打扰 | `frontend/src/lib/notificationPolicy.ts` | 决策 65 |
| 实时：逐任务开 SSE 流 + 10s 对齐 tick 兜底 refetch | `frontend/src/realtime/connection.ts`、`frontend/src/stores/board.svelte.ts` | 决策 76 |
| 主题切换（夜班靛 / 掌机背光）并入底部状态行 | `frontend/src/components/layout/StatusLine.svelte` | 决策 169 |
| 详情页加载失败 / 换 id：把上一个任务连同它的动作按钮一起收走，并给一颗能按的「重新加载」（「这个 id 没有」与「没读到」分开说） | `frontend/src/stores/taskDetail.svelte.ts`、`frontend/src/routes/TaskDetail.svelte` | 票 01（R2-01）；清空是 `resetTaskContent()`，两条出路是「重试」与「重新加载」 |
| 错误可见、可说、可恢复：错误横幅进 live region（`role=alert`）、表单给字段级 `aria-invalid` + `aria-describedby`、市场刷新失败**保留已列出的列表**并自带重试 | `frontend/src/routes/SettingsMarket.svelte`、`frontend/src/routes/Board.svelte`、`frontend/src/components/settings/ProjectForm.svelte`、`frontend/src/components/settings/ProviderForm.svelte` | 票 02（R2-06 / 07a / 07c） |
| 终端旁路动作（重试 / 归档）失败不静默：写进页面既有的动作错误位并播报 | `frontend/src/routes/TaskDetail.svelte`、`frontend/src/stores/taskDetail.svelte.ts` | 票 02（R2-08）；审计 ②.1 的取证是假阳性（注入的路径对不上），代码结论成立并已修 |
| 对话框动作进「提交中」态并有重入护栏（拆分 / 换模型）：连点两次只发一个请求 | `frontend/src/stores/taskDetail.svelte.ts`、`frontend/src/components/task/SplitDialog.svelte`、`frontend/src/components/task/ModelOverrideDialog.svelte` | 票 03（R2-03）；`dialogInFlight` 与 `busyKey` 两道 |
| 「待处理」下拉是真链接列表而不是假菜单：Escape / 点外关得掉、方向键进得去、面板常驻 DOM 用 `hidden` 收 | `frontend/src/components/layout/TopBar.svelte`、`frontend/src/stores/board.svelte.ts` | 票 04（R2-04）；**降级**掉 `role=menu`（ARIA 1.2 里它是「菜单」，补不起契约就别用它） |
| 全站地标与标题：每页一个 `<main>`、每页有 `<h1>`（看板与 404 也补）、页签是真 `tablist`（方向键 + roving tabindex）、当前项 `aria-current`、每页 `document.title` 各不相同 | `frontend/src/routes/Board.svelte`、`frontend/src/routes/TaskDetail.svelte`、`frontend/src/App.svelte`、`frontend/src/router.svelte.ts` | 票 06（R2-19 / 20） |
| 装饰不进可访问名：按钮上的 `▶` 用 `::before` 画（`clip-path` 三角），DOM 里没有那个字符 | `frontend/src/app.css`、`frontend/src/components/layout/TopBar.svelte` | 票 06（R2-19）；像素纪律「只有 2px 一档描边」由 `frontend/src/theme/css-parity.test.ts` 守 |
| 钉边元素按**变量**让位：动作坞钉在底栏上沿（`--sbar-h`）、档案盒吸顶避开**实测**顶栏高度（`--topbar-h`，由顶栏量出来写回） | `frontend/src/app.css`、`frontend/src/components/layout/TopBar.svelte`、`frontend/src/components/task/PendingDossier.svelte` | 票 05 / 09（R2-02 / R2-11）；两个高度的单一出处都在 `app.css` 的 `:root` |
| 状态行档位：`≤748` 舍三格汇总、`≤560` 再舍 token 量表（数字逐字保留），容器加横滚兜底——**舍格优先、可滚兜底，绝不静默裁切** | `frontend/src/components/layout/StatusLine.svelte` | 决策 215 的档位表；票 08（R2-10） |
| 同名动作不是一个动作：身份 = 动作名 + 游标 + **落点**，渲染层的 each key 与「提交中」态共用同一把尺子 | `frontend/src/lib/actions.ts`、`frontend/src/components/board/PendingActions.svelte`、`frontend/src/components/task/DiffReviewPanel.svelte` | 票 20；`retry_exhausted` 的两条 `goto` 撞 key 会让整块动作区停更 |
| 新建任务按服务端返回的 id 跳转（不靠列表里的第一个去猜） | `frontend/src/stores/board.svelte.ts`、`frontend/src/components/board/NewTaskDialog.svelte` | 票 10（R2-12） |
| 提交前拦下明显非法的值：`base_url` 形状、拆分里空标题的行**指出第几行**、文本域全空给提示而不是静默 no-op | `frontend/src/lib/providers.ts`、`frontend/src/components/task/SplitDialog.svelte` | 票 11（R2-13） |
| 请求有统一超时（值只有一处出处），超时给可读错误而不是内部字眼；值班长发话单独放宽（5 分钟——那一轮不随本地放弃而死，故这只是本地等多久） | `frontend/src/api/client.ts` | 票 12（R2-14）＋ 决策 223；`REQUEST_TIMEOUT_MS` 与 `mapRequestError` |
| 命令输出读不回来就说失败（不再永远「正在加载完整输出…」） | `frontend/src/components/task/CommandLog.svelte`、`frontend/src/stores/taskDetail.svelte.ts` | 票 12（R2-16）；`commandOutputError` 此前无人读 |
| 详情页也有实时断线指示（与看板同一句话）；流未连通时动手要说「回执要等重连」而不是静默等 30 秒 | `frontend/src/stores/taskDetail.svelte.ts`、`frontend/src/routes/TaskDetail.svelte` | 票 13（R2-15） |
| 时间与日期的格式只有一处出处（`lib/format.ts` 之外不许再出现 locale 调用） | `frontend/src/lib/format.ts`、`frontend/src/lib/format.test.ts` | 票 15（R2-18）；静态扫描是这道门的一部分 |
| 同一个 `bind_source` 值只有一个说法（定义收在纯函数里，模板只调它） | `frontend/src/lib/sharePairing.ts`、`frontend/src/routes/Share.svelte` | 票 15（R2-18） |
| 长值不撑破容器（`min-width: 0` + `overflow-wrap: anywhere`）；截断的值能悬停看全（`title`） | `frontend/src/components/render/MetadataCard.svelte`、`frontend/src/components/render/DiffView.svelte`、`frontend/src/components/settings/AnalysisChecklist.svelte` | 票 17（R2-22）；长值的尾巴才是区别所在 |
| toast：hover/focus 时暂停计时（剩余时长**接着算**）、每条 `role="status" aria-atomic`、手机上挪到顶栏下方让开动作坞 | `frontend/src/stores/notifications.svelte.ts`、`frontend/src/components/layout/ToastStack.svelte` | 票 17（R2-23） |
| 没有第二个通道的通知不节流：`failed` 的 toast 是它唯一的通道，夜间免打扰与同类 cooldown 都不吞它 | `frontend/src/lib/notificationPolicy.ts` | 票 17（R2-23 的调整支） |

**决策 216（不可逆动作的确认步与三档量级）与 217（中流状态的地址与本地留存）的实现行，等实现票
[21](../.scratch/ux-audit-2/issues/21-destructive-confirm-impl.md) / [22](../.scratch/ux-audit-2/issues/22-midflow-persistence-impl.md)
落地后补**——按本表第 5 条的规矩，位置必须现在就在磁盘上，而这两条的行为现在**还没有落点**。
同一条规矩下，决策 215 的「详情页 / 对讲台中间档折行 + hero 轨道容器内横滚」也还没有行
（那两格是票 18 / 19），**已经落地的状态行档位在上面**。
