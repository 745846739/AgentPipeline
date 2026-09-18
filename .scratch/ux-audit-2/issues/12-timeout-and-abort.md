# 12: 请求超时与取消，去掉永久转圈

**叠:** A（不动规格）

**来源:** R2-14（代码）+ R2-16（代码）+ R2-17（代码）

**What to build:** 全站没有任何请求超时或取消：`api/client.ts:88-94` 的 `fetch` 只透传调用方的
`signal`，而**没有一处调用方传 signal**，也没有 `AbortSignal.timeout`；唯一边界是项目分析的
60s（`lib/analysis.ts:37`）。TCP 连上但不回包时，用户没有出口，只能刷新整页。

具体会卡死的：

- 技能安装：`SettingsMarket.svelte:247-276` 的 `installing` 永不复位；
- 市场列表读取：`:215-236` 的 `listing` 卡在「正在读 X…」；
- 指标加载：`Metrics.svelte:56-66`，触发钮 `disabled={loading}` 一直禁着。

同族两条：

- **`installing` 是单槽**（`string | null`，`finally` 无条件清空）：慢网络下先后装两个技能，
  先完成的会把后一个的 pending 态一起清掉，那一行按钮复活、还能再点。
- **命令输出「正在加载完整输出…」是个能永久停住的谎**：`CommandLog.svelte:39-45` 的
  `outputText` 在失败时落到这个字面量；错误其实存过
  （`stores/taskDetail.svelte.ts:182-190` 的 `commandOutputError`）但**没有任何地方读它**。

**Blocked by:** None（can start immediately）

**Status:** open

- [ ] `request()` 有默认超时（统一口径），超时给出可识别的错误
- [ ] 长操作（安装 / 市场读取 / 指标加载）暴露「取消」或至少在超时后复位按钮
- [ ] `installing` / `listing` 改成按目标键控的集合，互不覆盖（或加并发护栏）
- [ ] 命令输出失败：把 `commandOutputError` 渲染出来，不再永远说「正在加载」
- [ ] 单测：超时映射成的错误消息；`commandOutputError` 的渲染分支可达
- [ ] e2e：`page.route` 挂起不响应 → 断言超时后按钮复位且有可读的错误
- [ ] e2e：命令输出接口返回 500 → 断言面板显示失败而不是「正在加载」

**边界.** 超时值要有单一出处（别每页各写一个），但不新建抽象层——放在 `api/client.ts` 即可。
