# 02: 主题契约模块 + token 层重写 + 字体自托管 + 视觉护栏起始

**What to build:** 打开界面立刻换材质——全站从磷光终端变成像素机房：夜靛（浅色为背光灰纸）
配色、缝合像素单一字族、圆角恒 0、描边只有 2px 一档、阴影只有 `4px 4px 0` 硬投影、
动画离散帧步进。同时立起本 effort 唯一的接缝（主题契约模块）与视觉回归护栏的**起始部分**，
使后续 9 张视觉票从这时起就在自动护栏下工作。

**Blocked by:** 01 参照物冻结点 + 视觉方向决策登记

**Status:** ready-for-agent

- [ ] 新增**主题契约模块**（本 effort 唯一新接缝）：导出三类纯数据——几何常量（描边 2px、
      硬投影 `4px 4px 0`、圆角 0、字阶 `[12,24,36]`、dither 周期 4px、量表 16 段、
      boss 条 20 段、道具槽 34px、列宽 264px、dossier 320px、详情 max 1000px、
      顶盖带 6px、传送带 8px、移动断点 480px）、深浅两套色彩 token 名与值
      （逐行对齐 theme-6-pixel.md §2.1 / §2.4）、sprite 表（15 枚：flag / gem / hammer /
      flask / gear / lens / shield / merge / trophy / chest / alert / chart / key / phone /
      foreman；8×8，foreman 为 16×16；从原型 `SPRITES` 机械搬运）、状态映射
      （灯色 token / 描边 token / 小人节奏 × running / pending / failed / done / queued / waiting）
- [ ] `app.css` 重写为像素 token 契约：保留既有语义命名（`--bg` / `--panel` / `--wash` /
      `--pane` / `--text-hi` / `--text` / `--text-2` / `--text-3` / `--text-4` / `--go` /
      `--go-ink` / `--pending` / `--stop` / `--done` / `--diff-*` / `--input` / `--overlay` /
      `--pending-tint` / `--r-panel` / `--r-pill` / `--rail-col-width` / `--detail-max` /
      `--safeb`），使组件与既有测试的语义口不变；新增 `--ink` / `--belt-lit` /
      `--branch-dev` / `--branch-tst` / `--hairline`；删除终端专属 token（`--br*` / `--lit` /
      `--dash` / `--spin*` / `--mask-bg` / `--rail-band*` / `--head-band` / `--bar-band` /
      `--bracket` / `--go-hi` 中无人消费者）
- [ ] 全站基元：圆角 0；描边 2px 一档；阴影只允许 `4px 4px 0 var(--ink)`；按压
      `translate(4px,4px)` + 去投影；`-webkit-font-smoothing: none`；`b`/`strong` 只提亮不加粗；
      字号只取 12 / 24 / 36；dither 为全站唯一「渐变」且只允许出现在货箱顶盖带与已归档工位
- [ ] 四个字族变量（`--font-code` / `--font-ui` / `--font-cond` / `--font-mono`）全部指向
      同一像素字族；连字继续关闭、数字继续 tabular-nums（可执行物不得退步）
- [ ] 深色为 `:root` 默认，浅色经 `html[data-theme='light']` 覆盖——**属性名与
      `localStorage` 键 `agentpipeline.theme` 不变**，`index.html` 的防闪回内联脚本保留
- [ ] **字体自托管**：Fusion Pixel 12px Monospaced 的 latin 与 zh_hans 两份 CSS 及其引用的
      78 个 woff2 子集入前端静态资产目录，随 `vite build` 进 `dist` 并被编译期内嵌进单二进制
      （决策 155）；`index.html` 移除 Google Fonts 的 Fira Code 预连接与样式表；
      `font-display: block`（本地毫秒级加载，不让像素身份走「先 monospace 再换」）；
      随附许可文本（`MIT AND OFL-1.1`）并注明来源与版本；字体栈保留系统 monospace 作防御性回退
- [ ] **视觉护栏起始**：新增像素主题 e2e spec，先在真应用上断言深浅两套的 `--bg` /
      `--pending` 计算值、圆角 0、2px 描边、`4px 4px 0` 硬投影（本票覆盖 02 已能生效的项，
      12 扩成全量）
- [ ] **vitest 契约测试**：几何常量与 theme-6-pixel.md §2.3 逐项一致；15 枚 sprite 全部有
      viewBox 与非空 rect；状态映射覆盖六种状态；深浅两套覆盖同一组 token 名
- [ ] **vitest 解析护栏**：读出 `app.css` 的 `:root` 与 `html[data-theme='light']` 两块，
      逐 token 比对契约值；扫描全部组件 style 块，**禁止 token 块之外出现裸十六进制颜色**
- [ ] **修既有 e2e 断言**：`pending-resume.spec.ts` 对 `.dtag` 的
      `toHaveCSS('color','rgb(255,180,84)')` 改为断言像素主题的琥珀 token 值
      （深色 `#FFB545`）——「pending 面板是琥珀」这条行为断言不得丢
- [ ] 既有 96 vitest 与 17 playwright 全绿；`svelte-check` 0 error / 0 warning；
      `make check` 全绿

**注意（不得弱化）：** 全站仍无渐变、无柔光、无 ease/cubic-bezier 缓动；`prefers-reduced-motion`
下全部静止。与决策 143 一致——契约只承载视觉数据，不承载状态或业务语义。
