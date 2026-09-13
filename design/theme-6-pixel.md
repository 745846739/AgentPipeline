# 主题六 · 像素机房「夜班流水线」

> 状态：**现行视觉规格**（2026-09-13 由提案升格，决策 169；主题三「终端 · 调度电报」退役，
> 见 [deprecated/](deprecated/README.md)）。本文是前端视觉的**唯一权威**——实现与本规格不一致
> 时以本文为准，改视觉先改本文。
> 交互与信息架构一律沿用 [frontend-design.md](frontend-design.md)（路由、五页签、待办 dossier、
> `allowed_actions` 纯渲染（决策 69/101）、异步按钮、卡片禁拖）；本文只定义该交互骨架上的
> 第六套视觉语言。页面元素与原 `prototype-terminal.html`（现
> [deprecated/prototype-terminal.html](deprecated/prototype-terminal.html)）同一套
> （8 列看板、详情三视图、顶栏过滤、demo 切换器；sync-check 全站不展示，决策 107），
> 并补齐 `frontend-design.md` §4 余下四个路由（项目 / 模型与密钥 / 全局指标 / 手机访问），
> 像素版实现见 [prototype-pixel.html](prototype-pixel.html)（桌面 7 视图）与
> [prototype-pixel-mobile.html](prototype-pixel-mobile.html)（移动 8 视图）。
> 四个台账页的组件映射见 §3.1。

## 1. 设计概念

**一句话：** 开发者是深夜车间的工头（玩家），看板是一台还在运转的 8-bit 流水线——
任务是传送带上的货箱，工位是机器，指示灯报告每台机器的状态，pending 是机器急停、
弹出的对话框正在等你按键。

领域词汇在此主题的转译：任务 = **货箱**，列 = **工位**，轨道 = **传送带**，
状态 = **信号灯**，pending = **急停**，dossier = **操作台对话框**，
折返线 = **回流带**，并行分支 = **双带并轨**。

四条原则：

1. **像素纪律。** 全站唯一字体（缝合像素 12px，见 §2.2），字号只取 12 的整数倍
   （12 / 24 / 36），禁用中间号；圆角恒为 0，描边只有 2px 一档；阴影只能是
   `4px 4px 0` 的硬投影；禁止平滑渐变与柔光——**2×2 棋盘 dithering 是全站唯一的"渐变"**，
   只允许出现在货箱顶盖带与已归档工位两处。
2. **灯即状态。** 沿用全站信号语义（绿 = 执行、琥珀 = 等人、红 = 失败、灰 = 归档、
   蓝 / 紫 = 分支徽章）。灯是实心像素方块，绝不作大面积底色；一张货箱上最多
   一枚灯 + 一种描边色。琥珀仍是全站唯一告警。
3. **急停对话框是主角。** pending 内容渲染成 RPG 对话框（双线框 + ▼ 光标），
   它是全站视觉权重最高、也是唯一"响"的东西；其余一切保持安静。
   boldness 只花在这一处。
4. **帧步进，无缓动。** 一切动画用 `steps()` 离散步进，像素世界的位移不经过中间帧：
   运行中工位的传送带虚线步进、对话框 ▼ 光标与流式输出方块光标闪烁、按钮按压位移。
   `prefers-reduced-motion` 下全部静止。禁止 ease / cubic-bezier 缓动与淡入滑入。

**选型自评（为什么不是那些更省事的像素风）：**
GameBoy DMG 四色绿——与主题三磷光绿同色相，撞车；`Press Start 2P`——像素主题的
默认字体，且无中文；全站 CRT 扫描线 / 玻璃反光——模板装饰，不携带信息；
近纯黑底 + 单酸绿强调——与主题三气质重复。最终选择：夜靛底 + 暖纸白文字 +
中文字体像素化，靠「急停对话框 + 传送带」两个领域转译承载个性。

## 2. Design Tokens

### 2.1 色彩（深色默认「夜班靛」）

| Token | 值 | 语义 |
|---|---|---|
| `--bg` | `#1B1D2C` | 夜班靛，页面底色（不是纯黑，带一点蓝紫的厂房夜灯色温） |
| `--panel` | `#232639` | 货箱面 / 对话框底 / hover 行底 |
| `--wash` | `#2B2F47` | 交互再深一档：active tab、dither 用色 |
| `--pane` | `#3E4363` | 2px 描边、传送带未点亮段 |
| `--ink` | `#12131E` | 硬投影专用墨色（比 bg 深两档，仅作 `4px 4px 0` 投影） |
| `--text-hi` | `#F1ECDC` | 暖纸白：标题、可执行物、当前游标（对 bg ≥ 12:1） |
| `--text` | `#C7C3B4` | 正文 |
| `--text-2` | `#918E9F` | 次文本（≥ 4.5:1） |
| `--text-3` | `#6E6C82` | 弱文本、列头；done 的退后灰 |
| `--text-4` | `#55536B` | 占位 / 装饰刻度，不承载必读信息 |
| `--go` | `#55D97C` | 信号灯绿「执行中」；主动作实心钮 |
| `--go-ink` | `#0B2314` | 实心钮前景 |
| `--pending` | `#FFB545` | 急停琥珀：等拍板、stalled、待办计数（全站唯一告警） |
| `--stop` | `#FF6157` | 失败红：打回、破坏性动作 |
| `--done` | `#6E6C82` | 归档灰，刻意退后 |
| `--branch-dev` | `#59A7FF` | develop-design 徽章蓝（仅徽章，不作状态） |
| `--branch-tst` | `#C08BFF` | test-design 徽章紫（同上） |
| `--diff-add` / bg | `#57D97C` / `#16301F` | 货单印刷绿 |
| `--diff-del` / bg | `#FF7B6E` / `#361A20` | 货单印刷红 |
| `--belt-lit` | `#4E5478` | 传送带已通过段（亮度阶，不用色相） |
| `--dither` | 2×2 棋盘 | `--wash` / 透明 相间，4px 周期 |

**规则：** 一张货箱 = 一枚灯 + 一种描边色（pending 琥珀描边 / failed 红描边 / 其余 pane）；
queued / waiting 无灯灰字。层级只靠硬投影与描边亮度，不引入第二套阴影或柔影。

### 2.2 字体

| 角色 | 字体 | 用法 |
|---|---|---|
| 全站唯一 | 缝合像素 12px 等宽（Fusion Pixel 12px Monospaced，zh_hans + latin） | 标题、正文、数字、命令一律同族；`-webkit-font-smoothing: none` 保脆 |

字阶：**12 / 24 / 36**（12 的整数倍，禁止中间号）。基准 12px 行高 1.6；
工位名 / 列头 12px + `letter-spacing: 0.08em`；大数字（时长、token、计数）24px 起步，
配 12px 灰注；wordmark 24px。禁用粗体模拟层级（像素字体无字重轴），
层级靠字号倍数、亮度阶与描边，`<b>` 仅作亮度提升。

### 2.3 形状与密度

> 以下数值与**冻结原型的实测值**对齐（原稿若与原型不符，以原型为准；逐处对账见 §3.2 末表）。

- 圆角 0；描边 2px 一档；硬投影三处：容器 `4px 4px 0 var(--ink)`、小控件
  `3px 3px 0 var(--ink)`、wordmark 文字 `3px 3px 0 var(--ink)`；按压态为对应距离的
  `translate` + 去投影。
- 货箱顶盖带：6px 高的 2×2 dither 条，是「这东西是实体」的材质提示，每张货箱一条。
- 列宽 264px × 8；详情页 `max-width: 1000px`；dossier 右栏 340px（分栏后整体 1240px）。
- 传送带：**6px 高**，`repeating-linear-gradient(90deg, … 0 6px, transparent 6px 12px)`
  画出像素链节（周期 12px）；运行中工位两侧的链节以 `steps(2)` 步进位移。
- 像素图元：8×8 工位图标（SVG `shape-rendering: crispEdges`，`currentColor` 随列头状态）
  + 16×16 工头头像（琥珀安全帽 + 绿背心，只出现在 dossier 对话框）+ 8×8 挥锤小人双帧
  （抬起 / 落锤+火花）。图元只允许来自本文的 sprite 表，新增图标需回到本文修订。
  **sprite 表**（`prototype-pixel.html` 的 `SPRITES`）：`flag` / `gem` / `hammer` / `flask`
  / `gear` / `lens` / `shield` / `merge` / `trophy` / `chest` / `alert` + 台账页三枚
  `chart`（指标）/ `key`（模型与密钥）/ `phone`（手机访问）+ `foreman`（16×16 工头头像）。
- **列头小人**：每个工位一名，动画速度与颜色随状态——执行中绿锤 0.6s 快挥、
  急停琥珀锤 1.8s 慢挥、空闲 / 排队灰锤静止站立（帧切换为离散 opacity 翻转）。
- token 量表：16 段像素 HP 条（每段 5×10px，间隙 2px），满格 ≈ 64k tok，颜色随状态灯
  （执行绿 / 急停琥珀 / 失败红 / 归档灰）；顶栏底部状态行的「本时辰」总量共用同一量表。
- **boss 战尝试条**：运行中货箱显示当前阶段尝试进度（20 段宽条，`已用尝试 / 上限` 折算
  点亮段数），最后一次尝试时整条转红——只是放大版量表，不引入第二套数据。
- **顶栏道具栏**：过滤项做成 34px 槽位（共享 2px 边框、图标 + 右下角计数徽章，
  hover 出 title 提示）；当前选中槽位 = 亮描边 + wash 底（游戏物品栏选中框）。
- **任务完成横幅**：任务进入 done 时顶部居中弹出奖杯横幅（trophy sprite + 「任务完成」+
  diff 摘要 + 「收下」按钮），无入场动画，点「收下」关闭。
- wordmark：24px + `3px 3px 0 var(--ink)` 硬投影，是全站唯一带投影的文字。

### 2.4 浅色变体「掌机背光」

`html[data-theme='light']` 语义；原型按主题三惯例出独立文件：
[prototype-pixel-light.html](prototype-pixel-light.html)、
[prototype-pixel-mobile-light.html](prototype-pixel-mobile-light.html)。

| Token | 值 | 语义 |
|---|---|---|
| `--bg` | `#E8E6DC` | 背光灰纸，页面底色 |
| `--panel` | `#F6F4EA` | 货箱面 / 对话框底 |
| `--wash` | `#DEDBCE` | 交互再深一档；dither 用色 |
| `--pane` | `#9A97A8` | 2px 描边、未点亮链节 |
| `--ink` | `#2A2B3A` | 硬投影墨色 |
| `--text-hi` | `#14151F` | 墨青标题（对 bg ≥ 12:1） |
| `--text` | `#2F3040` | 正文 |
| `--text-2` | `#55566A` | 次文本（≥ 4.5:1） |
| `--text-3` | `#7A7B8E` | 弱文本、列头 |
| `--text-4` | `#9A9BAC` | 占位 / 装饰刻度 |
| `--go` / `--go-ink` | `#1F7A3C` / `#F6F4EA` | 执行绿（加深一档）；实心钮前景转纸白 |
| `--pending` | `#8F5B00` | 急停琥珀（加深为可作正文的墨琥珀，全站唯一告警） |
| `--stop` | `#B3271E` | 失败红 |
| `--done` | `#7A7B8E` | 归档灰 |
| `--branch-dev` / `--branch-tst` | `#1D5FA8` / `#6F3BB8` | 分支徽章蓝 / 紫 |
| `--diff-add` / bg | `#1D6B3C` / `#DDEBDD` | 货单印刷绿 |
| `--diff-del` / bg | `#A02C22` / `#F4DEDA` | 货单印刷红 |
| `--belt-lit` | `#7E8094` | 已通过链节（比 pane 深一档：浅色靠墨度不靠亮度） |
| `--hairline` | `#D8D5C8` | 弱分隔（移动款用） |

浅色专属偏差（两处，均已验证）：

1. **wordmark 去投影**：硬投影在浅底高对比下会把 24px 紧排像素字搅成重影，
   浅色款 wordmark 无投影，层级由墨色本身承担；
2. **工头头像肤色固定**：脸块不用 `--text-hi`（浅色下变墨块），固定 `#E3C7A6`，
   眼睛保持 `--ink`。

**生成方式（不再手抄）。** 浅色款是深色款的纯 token 变换：头部三行（color-scheme /
标题 / 注释）+ 色彩 token 块 + 上面两处偏差，别无差异。四份文件由
[scripts/derive-light.mjs](../scripts/derive-light.mjs) 从深色款生成，避免手抄漂移
（仓库根执行）：

```bash
node scripts/derive-light.mjs design/prototype-pixel.html        design/prototype-pixel-light.html        desktop
node scripts/derive-light.mjs design/prototype-pixel-mobile.html design/prototype-pixel-mobile-light.html mobile
```

脚本对每处替换都要求命中，否则报错退出（宁可失败也不产出半成品）。**正确性依据**：
用 git 里的旧深色款跑同一脚本，逐字节还原出既有浅色款——变换清单是完备的。
截图由 `.scratch/shots/capture-pixel.mjs` 重新生成（30 张：桌面 7 视图 × 深浅 + 移动 8 视图 × 深浅）。

## 3. 组件映射

| 组件（frontend-design.md） | 像素机房实现 |
|---|---|
| 顶栏过滤 | **道具栏槽位**：图标 + 计数徽章；选中槽位 = 亮描边 + wash 底 |
| 轨道脊线 | 传送带：站点 = 信号灯方块（12px）+ 工位名；运行段链节步进 |
| 看板卡 | 货箱：2px 描边盒 + 顶盖 dither 带 + 4px 硬投影；meta 行带 token 量表（HP 条质感）；运行中另带 boss 战尝试条；hover = 描边亮一档 |
| 列头 | 8×8 工位 sprite（旗 / 菱形规锤 / 锤 / 烧瓶 / 齿轮 / 放大镜 / 盾 / 汇流 / 奖杯）+ 工位名 + **挥锤小人** |
| pending 卡 | 急停货箱：琥珀描边 + `?!` 灯 + 琥珀 2px 左缘条（与主题三 `! ` 前缀对应） |
| pending 理由 / dossier | **操作台对话框**：奶油双线框 + 压在框沿上的琥珀**名牌 tab**（FF 式）+ ▼ 闪烁光标；dossier 另有 16×16 工头头像坐镇左侧；恢复动作 = 菜单项按钮 |
| 按钮 | 像素钮：2px 描边 + 3px 硬投影；按压 = 位移消影；主动作 = go 实心 + `▶` 前缀 |
| 详情轨道 hero | 同一条传送带放大版；节点状态用灯（亮 / 半亮 / 空 / 琥珀 / 红） |
| Tab | 工位标签盒：active = wash 实底 + 描边上浮；禁用 = dither 底 |
| 底部状态行 | 车间看板条：2px 顶描边；token 总量用量表；分隔符 `▪`（不用中点） |
| 完成反馈 | 顶部居中**任务完成横幅**：奖杯 sprite + 「任务完成」+ diff 摘要 + 「收下」按钮 |
| 设置 / 指标页 | **车间台账**：与看板同一套 `--pane` 描边盒（2px）+ 硬投影；表格行 = 台账行（`reg-row`），行首名称亮纸白、副值灰、末列动作钮。详见 §3.1 |

### 3.1 台账页（设置 · 项目 / 模型与密钥 / 全局指标 / 手机访问）

四个页面共享同一套组件，只换内容（`frontend-design.md` §4 的四个路由）：

| 组件 | 像素机房实现 |
|---|---|
| 页头 | `crumb`（← 看板）+ 24px `p-title` + 右侧动作钮；导入语用 `hintline`（弱灰，内嵌 `<b>` 提亮） |
| 台账表 | `reg` 2px 描边盒 + `reg-head`（列头条，`▪` 分隔计数）+ `reg-row`（`--wash` 行分隔，hover 换 `--panel`）；行内一/二行 meta 用 `reg-l1`/`reg-l2` |
| 行内降级 | 决策 103 的"不受支持 vendor"整行 `--t4` 灰显 + `! 不受支持 · 决策 103` 琥珀标；恢复中的删除确认用 `reg-sub`「确认删除？」 |
| 伪阶段行 | 决策 84：左缘 4px `--t3` 亮度阶 + 名称后缀「（伪阶段）」，不用分支色相 |
| 核对清单 | `checklist` 描边盒，两列网格（移动款单列）；缺项 `mk` 用 `—` + `--t4`，值写「未探测到」而不是假装通过 |
| 指标条形图 | `chart` 盒 + `plot`（绝对定位 `rail-line` 链节）+ `cols`（站点列：标签 / 12px 灯 / 10px `track` 横条 / 数值）；`mdot` 灯与 `fill` 条同色（go / caution / stop / done / dev / test）。移动款 9 站放不进 430px：`plot` 横向滚动，站点不缩不折 |
| 首过率缺数据 | 不画 0 冒充真实值：整条改用一句话说明（`frontend-design.md` §7） |
| 手机访问 | `qrbox`：二维码恒白底（扫描器依赖明暗对比，浅色主题也不例外），旁边 `picked` 地址块 + 复制钮；多网卡地址上下排成 `alt-item`，末位 `tag`「推荐」；仅回环绑定时不画二维码，改用 `gate` 指引块（`--host 0.0.0.0` / `AGENTPIPELINE_LAN=1`，决策 167） |
| 原型 QR | 后端渲染真 QR（决策 167），原型只画一枚固定种子（20260913）的 21×21 像素示意：三角定位符 + 伪随机码点，**前端不引 QR 库** |

### 3.2 实现映射（原型 class ↔ 前端组件）

组件与原型同构：**原型是验收参照**（截图对照），下面是逐项对应关系。实现与原型有意偏离
之处必须在本表登记（末票核对）。

| 原型视图 / 区块 | 前端组件 | 备注 |
|---|---|---|
| 顶栏 `topbar` / `slots` / `slot` / `pnav` | `components/layout/TopBar.svelte` | 道具栏槽位 + 页面导航行；待办计数入口交互不变（决策 92） |
| 底栏 `statusbar` / `tokmeter` | `components/layout/StatusLine.svelte` | 车间看板条；主题切换钮并入此行 |
| 列 `col` / `col-head` / `stn` / `worker` | `components/board/BoardColumn.svelte` | 工位 sprite + 挥锤小人 + 计数 |
| 货箱 `card` / `card-top` / `bossbar` / `segs` | `components/board/TaskCard.svelte` | dither 顶盖带 + 16 段量表 + 20 段 boss 条 |
| 轨道 / 传送带 `belt` / `track` / `stn` | `components/pipeline/PipelineRail.svelte` | 三变奏共用同一套链节与信号灯原语 |
| 分支徽章 `tag` / `dev` / `tst` | `components/pipeline/BranchPill.svelte` | 分支身份 = 色相（蓝 / 紫）+ 文字双编码（决策 84 标注） |
| 急停对话框 `dialog` / `dface` / `dtag` / `dtxt` / `actions` | `components/task/PendingDossier.svelte` + `components/board/PendingActions.svelte` | 双线框 + 琥珀名牌 tab + ▼ 光标 + 工头头像 |
| 完成反馈（奖杯横幅） | 新增 `components/layout/CompletionBanner.svelte` | 奖杯 sprite + diff 摘要 + 「收下」；无入场动画 |
| 台账页 `crumb` / `p-title` / `reg` / `reg-row` / `checklist` | `routes/SettingsProjects.svelte` / `SettingsProviders.svelte` / `Share.svelte` + `components/settings/*` | 见 §3.1 |
| 指标 `chart` / `plot` / `cols` / `mdot` | `routes/Metrics.svelte` + `components/settings/TrackSegmentBars.svelte` | 9 站点列，灯与条同色 |
| 详情 hero / 页签 `detail` / `d-head` / `tabs` / `tab` | `routes/TaskDetail.svelte` + `components/pipeline/PipelineRail.svelte`（hero 变奏） | 五页签 = 工位标签盒 |
| sprite 表 `sp[data-s]` | 主题契约模块的 sprite 表 + `components/render/Sprite.svelte` | 15 枚；只允许来自受控表，新增回本文修订 |
| 量表 `.gauge` | `components/render/Gauge.svelte` | 16 段共用件（底栏总量 / 货箱 meta 行）；`tone` 取四盏信号灯语义 |
| boss 条 `.bossbar` | `components/board/BossBar.svelte` | 20 段；`exhausted` 时整条转红 |
| 主题契约（token / 几何 / sprite / 状态映射） | `frontend/src/theme/contract.ts` | **本 effort 唯一新接缝**（决策 169）；`app.css` 是它的手工镜像，由 `theme/css-parity.test.ts` 锁死 |
| 像素主题 e2e | `frontend/e2e/pixel-theme.spec.ts`（6 条）+ `e2e/screenshots.spec.ts`（真应用截图，默认 skip） | 断言真应用上算出来的样式；截图是证据不是门 |

**实现期与原型的有意偏离（已登记）**：实现与冻结原型（参照物冻结点提交 `612cc07`）的差异如下，
均为"原型措辞/示例与规格正文冲突，取规格正文与冻结原型的一致解释"：

| 处 | 原型 | 实现 | 依据 |
|---|---|---|---|
| 传送带链节高 | **6px**（`.belt{height:6px}`、6px 亮 / 6px 暗） | 6px | 规格 §2.3 原文写 8px 是原型定稿前的措辞，§5 移动款独立写 6px；**冻结原型为准**，§2.3 已同步改写 |
| dossier 右栏宽 | **340px**（`.detail.split{grid-template-columns:1fr 340px}`） | 340px | 规格 §2.3 原文写 320px，冻结原型为 340px；**原型为准**，§2.3 已同步改写 |
| 硬投影档位 | 容器 `4px 4px 0`、小控件 `3px 3px 0`、wordmark 文字 `3px 3px 0` 三处 | 同 | 规格原文「阴影只有 4px 一档」与原型不符；原型实际有三处，**原型为准**，§2.3 已同步改写 |
| 刻度盘段色 | `.mdot.dev` 用 `--t2` 而 `fill.dev` 用 `--belt-lit`（同一图元两色） | 灯与条同色（`--belt-lit`） | 规格 §3.1 明确要求「灯与横条同色」；原型此处自相矛盾，**规格为准** |
| 详情当前游标 | 只有实心绿灯（无滑动圆点元素） | 绿灯 + 离散心跳 | 规格 §5「当前游标保留滑动圆点」是主题三残留措辞；冻结原型用灯 + 心跳表达，**原型为准** |
| boss 条分母 | 原型写死 `尝试 2/3` | 契约镜像 `retryLimitMirror: 3` + 后端 `retry_exhausted` 权威转红 | 该值无端点下发且本 effort 不改端点（规格 Out of Scope）；代价与将来改法写在 `contract.ts` |


## 4. 选型注意

- 与主题三的边界：同为深色，但主题三是「字符终端」（单字族等宽、字符线路行、无盒体），
  本主题是「像素游戏机」（中文像素字体、实体货箱、对话框）；二者不可混用元素——
  字符线路行（○ ● ◆）在本主题内一律替换为像素灯与链节。
- 与主题五的边界：主题五用纸白 + 硬投影印刷隐喻；本主题沿用硬投影手法但材质换成
  像素盒（dither 顶盖、描边亮度阶），且底色为夜靛非墙灰。
- 动画预算全站只有四处（链节步进、▼ 光标、方块光标、列头小人挥锤 / 摆臂），
  帧切换一律为离散 opacity 翻转；任务完成横幅无入场动画。新增动画位需回到本文修订。
  **实现期登记的一处补记**：hero 的当前游标另有「心跳微光」（`heartbeat`，1.2s `steps(2)`，
  §3.2 偏离表第 4 行——原型用灯 + 心跳表达当前游标，取代主题三的滑动圆点 + 柔光）。
  它是同一条「当前游标」动画位的实现形式，不新增第五个语义位。
  「琥珀闪烁」（急停灯 / `.warn-dot` / 待办计数）是同一处「信号灯闪烁」位在各宿主的复用。

## 5. 移动版（深色）

桌面原型：[prototype-pixel.html](prototype-pixel.html)；
移动原型：[prototype-pixel-mobile.html](prototype-pixel-mobile.html)。

移动版不是把桌面压窄，而是把**横向传送带竖过来**：屏幕一个方向只放一站，
轨道从横向链节带改为纵向链节脊线。内容与状态标记与桌面逐字相同
（9 张货箱、8 站轨道、同一套 sprite 与量表）。

| 视图 | 结构 |
|---|---|
| 0 传送带（看板） | 顶栏铭牌行 + 信号灯缩略条（点灯跳段）→ 页面导航行（指标 / 项目 / 模型与密钥 / 手机访问）→ 道具栏过滤行（横向滚动）→ 8 段纵向站点带（链节脊线 + 站头：sprite + 小人 + 计数 + 货箱）→ 任务完成横幅 → 底部载波行 |
| 1 待处理 | 按等待时长收拢的收件箱（最久在最上）；此页无脊线，琥珀左缘保留在货箱上 |
| 2 详情 · 执行中 | 纵向轨道 hero（当前站带挥锤小人）→ 分段页签 → 内容；命令表两行制 |
| 3 详情 · 待审批 | 纵向轨道 hero → 工头对话框 → Diff 页签 → **常驻底部动作坞**（对话框式，琥珀顶框 + ▼） |
| 4 项目 / 5 模型与密钥 / 6 指标 / 7 手机访问 | 台账页同桌面款（§3.1）：页头行（← 看板 + 标题）→ 页面导航行 → 单据盒。差异只有三处：台账行由横向左右栏折成纵向（动作钮另起一行均分）；指标 9 站条形图 `plot` 横向滚动、站点不缩不折；手机访问的二维码与地址纵向堆叠、地址项改 44px 触控行。默认 `#v-board` 无高亮，台账页以 `.pnav.on` 标出当前位置 |

五条移动专属转写：

1. **传送带竖转。** 站点脊线 = 6px 纵向链节（`repeating-linear-gradient(180deg …)`），
   三态与桌面链节同色：未点亮 pane、已通过 belt-lit、急停琥珀半透明；
   站头信号灯（12px）压在脊线上。
2. **道具栏横滚。** 槽位 34px 不缩，放不下的槽位横向滚动；筛选时空站收拢但脊线保持连续
   （与主题三移动版同一交互，决策沿用）。
3. **pending 动作坞。** dossier 窄屏无法常驻右侧：对话框内容（含工头头像）留在正文流，
   恢复动作下沉为固定底部动作坞；异步按钮点击即禁用、回执后坞内收尾。
4. **小人随站。** 挥锤小人挂在站头（桌面的列头位置），节奏与颜色规则不变；
   详情页只在当前站出现小人。
5. **页面导航进顶栏。** 桌面款把四个台账页签铺成顶栏下的第二行；移动款折成 `.pagenav`
   横向滚动行，**与顶栏同 sticky**（顶栏因此长到 138px）。连带两处定值：站点带
   `scroll-margin-top` 96 → 148px、任务完成横幅 `top` 104 → 148px，否则横幅会压在顶栏下。

