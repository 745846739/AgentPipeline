# 06: 渲染守卫——系统消息与思考步的正文不该躺在 DOM 里

**What to build:** `frontend/src/components/task/SceneTimeline.svelte` 有五种可折叠步，
但折叠的实现**不一致**：

| 步 | 现状 | 正文是否进 DOM |
| --- | --- | --- |
| 阶段 prompt | `{#if promptOpen[step.key]}` | 展开才进 |
| 工具回执 | `{#if toolOpen[step.key]}` | 展开才进 |
| 命令回执 | `{#if expandedCmd === cmd.id}` | 展开才进 |
| **系统消息** | `<details class="sys">` 里直接放 `<pre class="sysbox">{step.text}</pre>`（`:240-244`），**无守卫、也无状态绑定** | **一直进** |
| **思考步** | `<details ... open={thinkOpen[step.key] ?? false}>` 里直接放 `<pre class="rm-body mono">{step.text}</pre>`（`:250-260`），**有状态但无 `{#if}`** | **一直进** |

后两类恰是**最长的文本**——代码注释自陈系统段「常以万字计」、思考「常比回话长一个量级」。
也就是说：收起来的东西其实全部解析、排版、进了 DOM，只被 `<details>` 视觉隐藏。再叠加每条
assistant 正文无条件过 `MarkdownView`（`:246`），手机上就是实打实的布局开销。

修法：给这两处补 `{#if}` 守卫，与另外三处对齐。系统消息需要一个新状态（如 `sysOpen`）+
`onclick` 切换——它现在连 `open` 绑定都没有。

**Blocked by:** 01, 02, 03, 04（批一验收通过后动工；见 spec 决议 5）

**Status:** ready-for-agent

## 落点

- `frontend/src/components/task/SceneTimeline.svelte`：`:240-244`（system）与 `:250-260`
  （thinking）加 `{#if ...}`；system 补状态与切换函数（照 `togglePrompt` / `toggleThink`
  的写法，含 `e.preventDefault()` 的受控展开手法）。
- `frontend/src/components/task/SceneTimeline.test.ts`：补「折叠态下正文不在 DOM」的断言。

## 验收

- [ ] 折叠态下系统消息与思考步的正文**不在 DOM**（`container.textContent` 不含该段文本）；
      展开后到达并可见
- [ ] 直播期间的思考步仍带 ticker 摘要（`thinkTicker`）——摘要行不受守卫影响
- [ ] 既有 `SceneTimeline.test.ts` / `taskScene.test.ts` 全绿（**注意**：若有断言依赖
      「折叠态正文也在 DOM」，按本票的新口径订正，而不是保留旧行为）
- [ ] 深链落点（`highlightRunId` 找 `article[data-run]`）与流式贴底跟随不受影响
- [ ] `npm test` / `npm run check` / `npm run build` 全绿

**明确不做**：场景时间线的虚拟滚动；把 `DEFAULT_PAGE = 50` 降到更小（决策 349 / 359 的
「整条时间线摆得开、直播贴底、深链落点」边界；且 106 只有 23 轮、本机最坏 48 轮，窗口
收益为零）；改归约（`buildTaskScene` 的判断一个字不动）。

**来源：** `.scratch/scene-read-path-perf/spec.md` 决议 6；现状见
`frontend/src/components/task/SceneTimeline.svelte:240-260`。
