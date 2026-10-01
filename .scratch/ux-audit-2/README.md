# AgentPipeline 前端 UI/UX 审计（第二轮 · 深度）

**Status:** done（批内 22 张票全部收口：18 张 done、票 19 superseded、票 18/21/22 于 2026-10-01 以 wontfix 关闭）

**日期:** 2026-09-18　**被测:** `target/debug/agent-pipeline` @ `dc72141`（内嵌 `frontend/dist/assets/index-B-YuEhTB.js`，13:38 构建）
**复现:** `bash scripts/e2e-artifacts.sh && cd frontend && UX_AUDIT2=1 npx playwright test --project=chromium e2e/ux-audit-2.spec.ts`
**证据:** 本目录 13 张 PNG + spec stdout 上逐条可引的数字。截图与 spec 默认 skip（`UX_AUDIT2` 未设时全跳），不进 `make check-e2e`。

---

## 〇、这一轮和第一轮差在哪

第一轮（`.scratch/ux-audit/`，决策 195–203，27 张票）审的是**静息态的画面与文案**：
一页一张截图，逐处看色值、措辞、信息架构、对比度。它交付得很好，但视角有一个盲区——
**它几乎只看了「页面安静地摆在那里」的样子**。没有一条用例把应用推到失败、推窄、
推快、推重复点击，也没有一条读 DOM 语义（`<main>` / `h1` / role / aria）。

第二轮换四个视角，四个都指向同一件事：**界面在「一切正常」之外没有第二副面孔**。

1. **语义与键盘**——地标、标题层级、role、aria-current、live region、菜单键盘；
2. **失败与断线**——请求失败、SSE 断开、加载失败之后用户看到什么、能不能恢复；
3. **窄档与溢出**——480–1240 这个中间档（第一轮只看 1440 与 430 两个端点，中间整段没人量过）；
4. **不可逆动作与重入**——单击即发、双击重入、无确认、无撤销。

方法：先四路静态审查（无障碍 / 表单与破坏性动作 / 加载与失败 / 窄档与文案）出候选，
再写 `e2e/ux-audit-2.spec.ts` 用**真二进制 + 真后端 + 真浏览器**逐条实测或证伪。
本报告的每条都标了证据等级：**实测**（浏览器量到的数字）/ **代码**（读源码可推、未跑）/ **未验证**。

---

## 一、第一轮修复的复核：成立

抽查了第一轮最容易被后续改动碰坏的四条，**都在**，没有回归：

| 第一轮承诺 | 复核事实 | 证据 |
|---|---|---|
| 对比度：`--text-3` 提到 ≥4.5:1 | 深 `#8E8CA5` / 浅 `#636477`，`app.css:28,85` 与契约 `contract.ts:334,363` 一致 | 代码 + `contrast.test.ts` |
| 空列引导句用次级必读档 | `BoardColumn.svelte:171-177` 已是 `--text-3`（第一轮收尾时的那笔欠账已还） | 代码 |
| 决策 201：过滤槽「图标 + 词」+ 计数去重 | `TopBar.svelte:118-140`：每槽有 `.lbl` 词、`aria-pressed`，第 3 槽不画重复计数 | 代码 |
| 模态框键盘（Escape / 焦点 / role） | `Modal.svelte` 的 `$effect` 聚焦 + `handleKey` 的 Escape/Tab 环绕完整 | 代码 |

**一处流程事实值得记**：决策 201（过滤槽）在 `DELIVERY.md` 的对照表里**只有决策票、没有实现票**，
但实现是落了的。下一轮读那张表时别把它当成欠账。

---

## 二、问题清单

### P0 · 会丢数据、会做错事、会点不到

---

#### R2-01 详情页加载失败时，**上一个任务连同它的拍板按钮**留在屏上

**症状.** 打开任务 A，再切到一个不存在的 id（改地址、点过期通知）：页面顶部一条红字
「任务不存在：…」，**红字底下仍然是任务 A 的完整界面**——标题、hero 轨道、页签、档案盒，
以及 **6 颗动作按钮**（含「合入」）。这些按钮提交到的是**新那个坏 id**。

**实测**（`①.3`）：
```
有效任务标题    {"beforeTitle":"实现用户登录接口"}
失败后         {"banner":"任务不存在：01JZZZZZZZZZZZZZZZZZZZZZZZ",
               "stillShowsOldTitle":true, "bodyLen":1130,
               "hasActionButtons":6, "showsRetry":1}
```
截图 `r2-detail-bogus-id.png`。

**根因.** `stores/taskDetail.svelte.ts:110`——`load()` 的 `catch` 只写 `this.error`，
**从不清 `this.state.task`**；渲染层 `routes/TaskDetail.svelte:247` 的 `{#if task}` 因此短路了
加载/空分支。同一个洞还有第二种表现：正常 A→B 切换时，B 的 URL 下会先闪 A 的正文。

**证据等级.** 实测（浏览器 + 源码一致）。

---

#### R2-02 移动款：底部动作坞把整条状态行盖住（含深浅切换）

**症状.** 手机上打开一个 pending 任务，底部动作坞（`z-index:31`）与状态行（`z-index:30`）
都钉在 `bottom:0`，动作坞不透明且更高，**把状态行整条压掉**：待处理/执行中/已完成计数、
token 总量、以及**深浅色切换**都看不见也点不到。

**实测**（`①.6`，430×932）：
```
dock   {top:752, bottom:932, h:180, z:31}
status {top:890, bottom:932, h:42,  z:30}
overlapPx: 42        ← 状态行高度 42，重叠 42 = 100%
```
截图 `r2-mobile-detail-dock-vs-status.png`。

**根因.** `app.css:696-705`（`.dock`，`z-index:31`）对 `StatusLine.svelte:95-110`
（`z-index:30`）。同仓已有正确写法可照抄：对讲台的输入坞用
`Talk.svelte:2073-2079` 的 `bottom: var(--sbar-h)` **给状态行让位**，`app.css:629-632`
的注释也只承认对讲台那一处让位——详情页的坞漏了。

**证据等级.** 实测。

---

#### R2-03 「拆分任务」可双击提交，原任务被取消两次、子任务翻倍

**症状.** 点「确认拆分」不会进入任何「提交中」态：按钮不 disabled、没有转圈。慢网络下
再点一次（或用户看不出反应就补点），会发出两次 `POST /tasks/{id}/split`——**原任务被取消，
子任务创建两套**。同类问题在「更换模型」对话框上一样。

**根因.** `stores/taskDetail.svelte.ts:290-313` 的 `submitSplit` / `submitModelOverride`
**从不设置 `busyKey`**（对比 `runAllowedAction:262` 与 `submitReview:318` 都设了），
而两个对话框拿的正是 `submitting={taskDetail.busyKey !== null}` → **恒为 false**；
`SplitDialog.svelte:16-27` 的 `submit` 自己也没有重入护栏。

**证据等级.** 代码（路径确定；harness 无法造出 `context_overflow` 这个 pending 态，
故未在浏览器里跑）。

---

#### R2-04 「待处理」下拉是 `role="menu"`，却没有菜单的任何行为

**症状.** 键盘用户打开一个播报为「菜单」的东西：Escape 关不掉、方向键不动、点空白处也不关，
只有再点一次那个芯片才关。触发钮还缺 `aria-haspopup`／`aria-controls`。

**实测**（`①.2`）：
```
{"opened":1, "afterEsc":1, "afterOutside":1,
 "attrs":{"expanded":"true","haspopup":null,"controls":null},
 "focusedAfterArrow":"BUTTON.chip pending-count pend"}
```
即：打开 1 → Escape 后仍 1 → 点外部后仍 1；ArrowDown 后焦点仍停在触发钮。

**根因.** `TopBar.svelte:143-170`（`role="menu"` / `role="menuitem"`）与
`stores/board.svelte.ts:247-250`（toggle 只认点击）；全文件无 `onkeydown`。
`role="menu"` 是有契约的——要么补齐菜单键盘语义与关闭路径，要么**别用它**（降级成普通弹层）。

**证据等级.** 实测。

---

#### R2-05 480–1240px 这个中间档：详情页与对讲台都不折行，主栏被挤成一条

第一轮只量了 1440（桌面）与 430（移动）两个端点，**中间整段没有断点**。实测三个症状同源：

**5a 详情页主栏在 480px 只剩 102px**
```
①.4  w=480  main=102  dossier=320  pageOverflow=352
     w=520  main=142  dossier=320  pageOverflow=312
     w=600  main=222  dossier=320  pageOverflow=232
     w=768  main=390  dossier=320  pageOverflow=64
```
`TaskDetail.svelte:474-480` 的 `grid-template-columns: minmax(0,1fr) 320px` 只在
`479px` 以下折成一列（`:678-681`）。截图 `r2-detail-768.png` / `r2-detail-520.png`——
768 下标题与 `architect-design.validate_output` 这类词已经在词中间断行。

**5b hero 轨道固定 812px，从约 830px 起页面就横向滚**
```
①.4  heroScrollW=812 而 heroClientW=390(w=768) / 102(w=480)
```
`.rail.hero`（`PipelineRail.svelte:340-345`）**没有 overflow**，而兄弟 `.rail.spine`
（`:336-338`）明确裁切；轨道几何写死到 `x:798`（`lib/pipeline.ts:332-342`），
`pipeline.ts:327-328` 的注释自己写着「可用 960px」的前提。

**5c 对讲台对话列在 480px 只剩 82px**
```
①.7  w=480 → cols "82px 340px"    w=600 → "202px 340px"    w=768 → "370px 340px"
```
`routes/Talk.svelte:1369` 同构，只在 `479px` 以下折（`:1924-1941`）。截图 `r2-talk-768.png`。

**证据等级.** 实测（几何数字）+ 代码。

---

### P1 · 失败没有出口、错误说不到、语义缺失

---

#### R2-06 全站错误都「说不到」：没有任何错误进 live region

**实测**（`①.1`，11 条路由）：`ariaLive` 恒为 **1**、`roleAlert` 恒为 **0**——那唯一的
live region 是完成横幅的 toast（`ToastStack.svelte:11`）。也就是说：
加载失败横幅、动作提交失败横幅、表单校验错误，**一个都不播报**。

**根因.** 所有 `.banner.error` 都是裸 `<div>`（`Board.svelte:165-173`、`TaskDetail.svelte:239,244`、
`Metrics.svelte:225`、`SettingsMarket.svelte:302,440,532`、`Talk.svelte:898`）；
表单错误同理（`ProjectForm.svelte:89`、`NewTaskDialog.svelte:120`），且没有
`aria-invalid` / `aria-describedby` 把字段与错误关联起来。

**证据等级.** 实测（ariaLive/roleAlert 计数）+ 代码。

---

#### R2-07 「失败之后没有下一步」是三个页面的共同形状

**7a 技能市场：断网刷新把列表和控制一起弄没**
**实测**（`④.1`）：列出技能后断网 → 点刷新 →
```
刷新钮 1 → 0      查看技能钮 0      技能行 0
```
列表被「读不到这个仓」顶掉，页面上**既没有刷新钮、也没有查看技能钮**——唯一出路是换个仓
再切回来或整页刷新。截图 `r2-market-refresh-offline.png`。
根因：`SettingsMarket.svelte:437-442` 的分支顺序把 `listError` 放在 `list` **之前**，
于是 `refresh` 特意保留的 `list`（`:215-236`）根本渲染不到；重试钮长在 `{:else if list}` 分支里（`:444-452`）。

**7b 详情页首次加载失败＝永久死页**
`taskDetail.svelte.ts:108` 的 `streamManager.sync()` 在 `try` 里、抛错就到不了；
`{if task}` 分支又不显示重试钮。服务器恢复后页面不会自愈（`connection.ts:221-228` 的
visibility 恢复遍历的是空连接表）。

**7c 设置五页的错误横幅都没有重试钮**
`SettingsProjects.svelte:223-224`、`SettingsProviders.svelte:174-175`、
`SettingsStages.svelte:164-165`、`SettingsMarket.svelte:301-302`、`Share.svelte:186-187`——
都是 `<div class="banner error">{error}</div>`，`load()` 只在 `onMount` 调。对比：
看板（`board.svelte.ts:203-206` 10s tick）与对讲台（`Talk.svelte:576-580` visibility 重载）
**会自愈**——所以这是三档不一致，不是「全站如此」。

**证据等级.** 7a 实测；7b/7c 代码。

---

#### R2-08 重试 / 归档失败是**完全静默**的

**实测**（`②.1`）：在一个终态任务上注入 `POST /tasks/{id}/retry → 500`，点「重试」：
```
点击后 {"bannerErrors":[], "liveRegions":1}   ← 没有任何横幅、没有任何提示
```
按钮静静复活，用户不知道发生了事。截图 `r2-retry-failure.png`。

**根因.** `routes/TaskDetail.svelte:181-190` 的 `bypass()` 是 `try/finally`，
**没有 `catch`**——`retryTask`/`archiveTask` 的 rejection 无人接（Svelte 的 `onclick`
直接丢弃返回的 promise），而页面上明明有 `actionError` 这条现成的展示路径没被用上。

> 取证注记：该任务停的是 `pending · 重试耗尽`（不是 `failed`），但「重试」钮在，
> 注入的 500 确实没产生任何横幅——两种状态下这条路径都是静默的。

**证据等级.** 实测 + 代码。

---

#### R2-09 不可逆动作一律单击即发：没有确认、没有撤销、默认焦点不在安全项

`合入`（`DiffReviewPanel.svelte:119-129`）、`通过评审` / `打回并附意见`（`ReviewForm.svelte:94-107`）、
看板上的 `强制通过评审` / `忽略失败依赖，继续执行` / `终止任务`（`PendingActions.svelte:49-53`）、
`重置配对`（`Share.svelte:146-155`，会让所有已配对手机失效）——**全部一次点击直接提交**。
全站唯一的确认步是删项目/删 provider 的两步内联确认，而这几处没有。

两处加重：**打回**按钮写着「打回并附意见」，但意见框空着也照样提交
（`ReviewForm.svelte:94-107` 传 `comments.trim() || undefined`）；**破坏性动作的视觉权重被压低**——
`终止任务` 是 `.btn quiet`，而会跳过质量闸的 `强制通过评审` 反而是 `.btn solid`（`PendingActions.svelte:117-142`）。

**证据等级.** 代码（`合入` 在 ①.4 的截图上可见是单击主按钮，形态一致）。

---

#### R2-10 状态行在 480–748px 被裁掉最多 268px，且没有横滚

**实测**（`①.8`）：
```
w=768  clipped=0
w=700  clipped=48     clockRight=748 > viewport=700
w=600  clipped=148
w=480  clipped=268    clockRight=748 > viewport=480
w=430  clipped=0      ← 移动款变体接管
```
`StatusLine.svelte:95-116` 是单行 flex + `white-space:nowrap`，既无 `flex-wrap` 也无 `overflow`；
唯一的适配是 `479px` 以下隐藏若干格（`:165-191`）。组件的注释自己记着这个失败模式与 501px 的实测值，
但**只把下限设在 479、没管中间段**——于是 480–748 之间时钟整块静默消失。

**证据等级.** 实测。

---

#### R2-11 sticky 档案盒被顶栏盖住「等你拍板」铭牌

**实测**（`①.5`，1280×900，滚 500px）：
```
topbarH=78   dossierTop=56   tagTop=42   headerBottom=78   stickyTopRule="56px"
```
档案盒 `top:56px`（`PendingDossier.svelte:227-229`）而顶栏实测 78px 高且不透明
（`TopBar.svelte:209-215`）→ 顶栏压住档案盒上沿 22px，**包括整块 `top:-16px` 的琥珀铭牌**。
应用自己别处就知道顶栏是 78–81px（`Talk.svelte:1372-1373` 的注释），这个 56 是个没对上的旧值。
截图 `r2-detail-dossier-scrolled.png`。

**证据等级.** 实测。

---

#### R2-12 新建任务：对话框里选了别的项目，会跳到**另一个项目的任务**（或哪儿都不去）

**症状.** 看板当前在项目 A，打开「新建任务」把项目改成 B 并创建：任务是**建在 B 里**了，
但页面跳到了 **A 的某个任务**（`this.tasks[0]`）；如果 A 一个任务都没有，`task` 为 `null`，
对话框关掉、人留在看板上——新任务在 B 里，当前视图看不到，**看起来像没创建成功**。

**根因.** `stores/board.svelte.ts:159-169` 的 `createTask` 丢掉 `POST /tasks` 的返回值，
改从 `loadTasks()` 的结果里拿 `this.tasks[0]`，而 `loadTasks` 是按 **`board.projectId`**
（`board.svelte.ts:60-63`）过滤的；对话框的项目是它自己的局部 `projectId`
（`NewTaskDialog.svelte:39-60`）——两个 id 可以不同。

**证据等级.** 代码（`POST /tasks` 返回 `{task}`，见 `crates/app/src/routes/tasks.rs:114`）。

---

### P2 · 一致性、健壮性、可达性打磨

---

#### R2-13 表单校验缺口：非法 `base_url` 直接落库；拆分行能造出空标题任务

**实测**（`③.1`）：`base_url` 填 `not a url` → 创建成功、字段零报错、列表里逐字显示：
```
{"fieldErrors":[], "rowsWithBadUrl":["openai gpt-4o-mini [ON] ctx 128,000 base_url not a url …"]}
```
`validateProviderDraft`（`lib/providers.ts:63-70`）只校验 vendor / model / context_window，
后端也直接落库（`crates/app/src/routes/providers.rs:65,114-115`）。后果延后到任务跑模型时才炸，
离填写现场很远。

同族：`SplitDialog.svelte:18-26` 的 `line.split('|')` 让 `| 说明` 这种行产出
`{title:'', description:'说明'}`——原任务被取消、一个**空标题**子任务被创建；而文本域
全空时点「确认拆分」**什么都不发生也不说为什么**（`if (tasks.length > 0)`，现有单测
`SplitDialog.test.ts:41-51` 正好把这个静默 no-op 钉住了）。

**证据等级.** base_url 实测；拆分部分代码。

---

#### R2-14 没有请求超时，也没有取消：悬挂的请求会留下永久转圈

`api/client.ts:88-94` 的 `fetch` 只透传调用方的 `signal`，而全仓没有一处传 signal，
也没有 `AbortSignal.timeout`；唯一边界是项目分析的 60s（`lib/analysis.ts:37`）。
具体会卡死的：技能安装（`SettingsMarket.svelte:247-276` 的 `installing` 永不复位）、
市场列表读取（`:215-236` 的 `listing`）、指标加载（`Metrics.svelte:56-66`，按钮 `disabled={loading}`）。
TCP 连上但不回包的场景下，用户没有出口，只能刷新整页。

**证据等级.** 代码。

---

#### R2-15 详情页没有断线指示（看板有）

`stores/taskDetail.svelte.ts:70-75` 构造 StreamManager 时只给 `onEvent` / `onRecalibrate`，
**没有 `onStatus`**；而看板给了（`board.svelte.ts:48-50`）并渲染「实时流已断开，正在重连…」
（`Board.svelte:171-173`）。于是任务详情页在服务器重启/网络断掉后**静静停止更新，看起来一切健康**。
更棘手的是 `runAllowedAction` 成功后要等 SSE 回执、30s 兜底（`taskDetail.svelte.ts:273-276`），
流死时「合入」会像没发生一样等 30 秒。

**证据等级.** 代码。

---

#### R2-16 「正在加载完整输出…」是一个能永久停住的谎

命令输出存在文件时（`stdout_path`），若 `GET …/output` 失败，
`CommandLog.svelte:39-45` 的 `outputText` 落到 `'（正在加载完整输出…）'`——**永远这么说**。
错误其实被存过（`stores/taskDetail.svelte.ts:182-190` 的 `commandOutputError`），
但全仓没有任何地方读它（`grep commandOutputError` 只命中写入处）。

**证据等级.** 代码。

---

#### R2-17 市场并发安装共用一个 `installing` 槽，状态会互相清掉

`SettingsMarket.svelte:247-276` 的 `installing: string | null` 是单槽，`finally { installing = null }`
无条件清空。慢网络下先后装两个技能：先完成的那次会把后一个的 pending 态一起清掉，
于是后面那一行的转圈消失、按钮复活，还能再点一次。
（同页 `listing` 也是单槽，同理。）

**证据等级.** 代码（未在浏览器里造并发）。

---

#### R2-18 文案与格式的不一致（三处小账）

- **时间**：市场用裸 `t.toLocaleString()`（`SettingsMarket.svelte:238-241`），
  而全站其余用 `formatDateTime` → `toLocaleString('zh-CN', {hour12:false})`
  （`lib/format.ts:39-44`，如 `SettingsStages.svelte:194`）。非 zh-CN 浏览器上同一页会同时出现
  `2026/9/18 15:04:05` 与 `9/18/2026, 3:04:05 PM`。
- **同一字段两个叫法**：`bind_source === 'settings'` 在 `Share.svelte:237` 叫「界面设置」，
  在 `:360-366` 叫「界面上的选择」（`startup`/配置回落两处措辞一致，只有这一处分叉）。
- **字面 Markdown 星号**：`Talk.svelte:913-914` 与 `:1228-1229` 的 `**换过令牌后要重新添加一次**`
  是纯模板文本，Svelte 不解析 Markdown、也没走 `MarkdownView`，所以 `**` 会原样显示。
  只在**未配对/局域网**那一态可见——这也是本轮没在页面上取到它的原因
  （`①.9` 在配对态 `asteriskLines:[]`）。

**证据等级.** 代码（星号一处本轮未在页面复现，已注明）。

---

#### R2-19 可访问名被装饰污染；选择态只有 class；触控目标偏小

- **`▶` 进了可访问名**：`.btn.solid::before`（`app.css:252-254`）、`.chip.on::before`（`:355-358`）、
  `TaskDetail.svelte:580-583`。同文件 `app.css:217` 自己定的规矩是「装饰不进可访问名」，
  `:245-246` 承认这一处进了。读屏会念「▶ 合入」。
- **选择态只有 CSS class**：任务详情页签（`TaskDetail.svelte:349-363`，无 `tablist`/`tab`/`tabpanel`/
  `aria-selected`）、对讲台班次芯片（`Talk.svelte:1048-1057`，无 `aria-pressed`）、
  会话 run 选择（`ConversationViewer.svelte:71-76`）、产出文件（`FileViewer.svelte:50-55`）、
  手机访问地址（`Share.svelte:308-316`）。实测 `①.1` 的 `tablist:0 / tabpanel:0`、
  `ariaCurrent:0`（全站 11 条路由都是 0——顶栏当前项也只靠 CSS）。
- **触控目标**：`.slot` 34px（`TopBar.svelte:285`）、待处理芯片约 24px（`:336-345`）、
  对讲台班次芯片约 27px、`StatusLine.svelte:189` 40px、`TaskDetail.svelte:697,768` 42px——
  都低于 44px，且移动断点只隐藏标签、不改高度（`:542-557`）。

**证据等级.** 实测（tablist/ariaCurrent 计数）+ 代码。

---

#### R2-20 地标与标题层级：10/11 路由没有 `<main>`，看板与 404 没有 `<h1>`

**实测**（`①.1`，逐路由）：

| 路由 | `main` | `h1` | 标题层级 |
|---|---|---|---|
| `#/` 看板 | **1**（唯一） | **0** | 完全没有标题 |
| `#/talk` 对讲台 | 0 | 1 | — |
| `#/metrics` 指标 | 0 | 1 | h1→h2 ✓ |
| `#/settings` 设置落地页 | 0 | 1 | ✓ |
| `#/settings/projects` | 0 | 1 | ✓ |
| `#/settings/providers` | 0 | 1 | ✓ |
| `#/settings/stages` | 0 | 1 | h1→h2 ✓ |
| `#/settings/market` | 0 | 1 | ✓ |
| `#/share` 手机访问 | 0 | 1 | ✓ |
| `#/nope` 404 | 0 | **0** | 无标题 |
| `#/task/:id` 任务详情 | 0 | 1 | **h1 → h4**（跳过 h2/h3） |

外加：`document.title` 在**全部 11 条路由**上都是同一个 `AgentPipeline · 像素机房`
（`index.html:7`，全仓无 `document.title` 赋值），标签页与前进后退无法区分页面。

**证据等级.** 实测。

---

#### R2-21 中流状态既不留存也不可深链

详情页的页签是局部 `$state`（`TaskDetail.svelte:26`）、看板过滤是局部 `$state`
（`board.svelte.ts:29`）、对讲台当前班次与输入草稿是局部 `$state`
（`Talk.svelte:129,145`）。持久化的只有 `agentpipeline.project_id` / `.theme` / `.pairing`
三把 key；URL 里只有 `?task=`（指标）与 `?project=&analyze=1`（项目）两个参数。
后果：F5 会把详情页签打回时间线、看板过滤打回「全部」、对讲台打回最近班次且草稿丢失；
前进/后退也恢复不了这些。

**证据等级.** 代码。

---

#### R2-22 长值撑破容器 / 截断了却看不到全文

- `MetadataCard` 的行是 flex，值列缺 `min-width:0`（`MetadataCard.svelte:54-74`），
  `word-break: break-word` 不会给 min-content 尺寸提供软换行点——一个长路径/ULID 就会把
  卡片（`max-width:760px`，无 overflow）和整个会话栏推出横向滚动。
- 三处 `text-overflow: ellipsis` 且**没有 `title`**，而这些值的**尾巴才是区别所在**：
  分析清单里的探测路径（`AnalysisChecklist.svelte:43,115-120`）、Diff 文件路径
  （`DiffView.svelte:22,56-60`）、toast 消息（`ToastStack.svelte:17,86-92`）。
  对照：`TrackSegmentBars.svelte:59` 是刻意带 `title` 的写法。

**证据等级.** 代码。

---

#### R2-23 逐字重复的大数字与 toast 生命周期（打磨）

- toast 的 TTL 是固定的 8s / 12s，**不因 hover/focus 暂停**，也不 `aria-atomic`
  （`stores/notifications.svelte.ts:44-47`、`ToastStack.svelte:11`）；消息还会被
  `white-space:nowrap` + ellipsis 截断。夜间 22:00–08:00 与同类 5 分钟节流会**静默丢弃**
  非 pending 通知（`lib/notificationPolicy.ts:49-52`）——一条 `task_failed` 可能根本不弹。
- 手机上的 toast（`z-index:70`，`bottom:16px`）会与详情动作坞（`z-index:31`）在同一位置重叠
  8–12 秒，且 toast 主体本身是可点导航按钮——点动作的位置可能跳去另一个任务。

**证据等级.** 代码。

---

## 三、核查后确认**没问题**的部分

免得下一轮再查一遍，也别让报告显得只有吐槽：

- **深色/浅色两套 token 的镜像与对比度**：契约 ↔ `app.css` 逐值一致，`--text-3` 达标，
  `contrast.test.ts` / `css-parity.test.ts` 是有效的门。
- **配对/鉴权的降级路径**：`Share.svelte:45-50,85-94` 把「未配对 / 被拒 / 失败」分开处理，
  无令牌时用指引块替代二维码；对讲台与详情页对 403 都有可点的下一步
  （`Talk.svelte:906-916,1221-1231`）；SSE 每次重连都重读令牌（`connection.ts:110-115`）。
  这一块做得比本轮多数 P1 都扎实。
- **项目分析是有界的**（60s 上限 + 转圈 + 禁用触发，`lib/analysis.ts:37`），
  局域网改绑有重试窗口并回读对齐（`lib/lanToggle.ts:63-108`）。
- **`prefers-reduced-motion` 全局处理**（`app.css:585-595` + `PipelineRail.svelte:508-514`）。
- **`<html lang="zh-CN">` 正确**；**没有缺可访问名的纯图标按钮**（逐个核过 95 个按钮，
  toast 关闭有 aria-label、移动款站点缩略有 aria-label）。
- **焦点环存在**（`app.css:191-194` 的 `:focus-visible`，`outline-offset:2px`）；
  输入框用 `:focus` 覆盖时以边框变色补偿（`app.css:318-321`）。

---

## 四、明确**未验证** / 有意留待

1. **Talk 的字面 `**` 星号**没有在页面上复现——它只在未配对/局域网态的引导段落里，
   本轮 harness 始终是回环配对态（`①.9` 输出 `asteriskLines:[]`）。结论来自源码＋确认无 Markdown 管道。
2. **R2-03（拆分双击）** 没有在浏览器里跑：`context_overflow` 这个 pending 态只有 L4 的
   `tests/e2e/tests/pending.rs` 能造，前端 harness 没有对应播种口。结论来自调用链。
3. **R2-17（并发安装）** 与 **R2-23 的夜间节流**未在运行时造并发/跨时区。
4. 本轮没有审**桌面壳（Tauri/dmg）**、**手机端真实浏览器**（Safari/Chrome for Android 的
   触控与安全区）、以及**真实 provider** 下的长任务体验（harness 全是 mock LLM）。
5. 未做的还有：200% 浏览器缩放的实测（结论按 CSS 视口减半推的），
   以及 `prefers-color-scheme` 跟随、多显示器 DPR。

---

## 五、票

一票一文件在 `issues/`，编号即依赖序；**A 叠**（不动规格的纯实现修正）与
**B 叠**（先出决策票、再出实现票）沿用第一轮的分法。规格与用户故事见 [spec.md](spec.md)。

| # | 票 | 叠 | 挡谁 |
|---|---|---|---|
| [01](issues/01-detail-state-hygiene.md) | 详情页失败时清空上一个任务，并给重试出口（R2-01 / 7b） | A | — |
| [02](issues/02-error-surfacing.md) | 错误可见可说可恢复：live region + 重试出口（R2-06 / 07a / 07c / 08） | A | — |
| [03](issues/03-reentrancy-guard.md) | 拆分/换模型进提交中态，杜绝双击重入（R2-03） | A | — |
| [04](issues/04-pending-dropdown-menu.md) | 待处理下拉：补齐菜单键盘语义，或降级掉 `role=menu`（R2-04） | A | — |
| [05](issues/05-mobile-dock-stacking.md) | 移动款动作坞给状态行让位（R2-02） | A | — |
| [06](issues/06-semantics-landmarks.md) | 全站语义：`<main>`、看板/404 的 h1、页签 tablist、aria-current、（R2-19 / 20） | A | — |
| [07](issues/07-narrow-band-layout.md) | 中间档版面：详情/对讲台在 480–1240 折行、hero 轨道不撑破页（R2-05） | **B** | — |
| [08](issues/08-statusline-clipping.md) | 状态行在 480–748 不再静默裁切（R2-10） | A | — |
| [09](issues/09-dossier-sticky-offset.md) | 档案盒 sticky 偏移改用顶栏真实高度（R2-11） | A | — |
| [10](issues/10-new-task-project.md) | 新建任务按服务端返回的 id 跳转（R2-12） | A | — |
| [11](issues/11-validation-gaps.md) | 校验缺口：`base_url` 形状、拆分空标题、提交前拦截（R2-13） | A | — |
| [12](issues/12-timeout-and-abort.md) | 请求超时与取消，去掉永久转圈（R2-14 / 16 / 17） | A | — |
| [13](issues/13-connection-indicator.md) | 详情页补实时断线指示（R2-15） | A | — |
| [14](issues/14-destructive-confirm.md) | 不可逆动作的确认、后果与安全默认（R2-09） | **B** | — |
| [15](issues/15-copy-and-format.md) | 文案与格式一致性：时间、绑定来源叫法、字面星号（R2-18） | A | — |
| [16](issues/16-persistence-deeplink.md) | 中流状态留存与深链（R2-21） | **B** | — |
| [17](issues/17-overflow-and-truncation.md) | 长值不撑破、截断可回看（R2-22 / 23） | A | — |

**票 07 开出的实现票**（决策 215 的交付段要求「定了之后另开实现票」；本批原本没有对应票，
故单列在这里，编号接在 17 之后——**它们不在本批的实现范围内**，07 只负责裁决）：

| # | 票 | 叠 | 挡谁 |
|---|---|---|---|
| [18](issues/18-detail-midband-fold.md) | 详情页中间档折行 + hero 轨道不再撑破页面（R2-05 5a/5b） | A | 07 |
| [19](issues/19-talk-midband-fold.md) | 对讲台中间档折行（R2-05 5c） | A | 07 |

**实现中现场发现的缺陷**（不在上面 23 条里，票内有取证）：

| # | 票 | 叠 | 状态 |
|---|---|---|---|
| [20](issues/20-action-key-identity.md) | 同名动作撞 key，整块动作区停更（`each_key_duplicate`） | A | 本批一并修完（实现 + 组件级单测 + 真应用 e2e） |

**票 14 / 16 开出的实现票**（沿用 07 的惯例：决策票只裁决，实现另开票）：

| # | 票 | 叠 | 挡谁 |
|---|---|---|---|
| [21](issues/21-destructive-confirm-impl.md) | 不可逆动作的确认步与三档量级（R2-09） | A | 14 |
| [22](issues/22-midflow-persistence-impl.md) | 中流状态的留存与深链（R2-21） | A | 16 |

**本批的实现状态**（2026-09-18 收口）：A 叠 01–06、08–13、15、17 已实现并各自带实施记录；
B 叠 07 / 14 / 16 只出裁决（决策 215 / 216 / 217），实现留给 18 / 19 / 21 / 22 ——
**这四张仍是 open，不在本批范围内**。18 / 19 挡着 07，21 挡着 14，22 挡着 16。
`17` 有一条子项**没做**（toast 的键盘关闭路径，理由写在票内实施记录末条）。
