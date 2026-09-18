# 16: 决策：中流状态的留存与深链

**叠:** B-决策（一票只回答「怎么定」；本票不动任何代码）

**来源:** R2-21（代码）

**What to build:** 中流状态全是局部 `$state`，既不持久也不可深链：

- 任务详情页签（`TaskDetail.svelte:26`）——F5 打回时间线；
- 看板状态过滤（`board.svelte.ts:29`）——F5 打回「全部」；
- 对讲台当前班次与输入草稿（`Talk.svelte:129,145`）——F5 回到最近班次、草稿丢失。

全站持久化的只有三把 key（`agentpipeline.project_id` / `.theme` / `.pairing`），
URL 里只有 `?task=`（指标）与 `?project=&analyze=1`（项目）。前进/后退也恢复不了这些。

这一票只回答「怎么定」：**哪些状态进 URL、哪些进 localStorage、刷新与后退各恢复成什么**。
注意取舍：进 URL 会改变可分享性（好处）但也会让地址变长、并可能让「后退」行为出乎意料。

**Blocked by:** None（can start immediately）

**Status:** done（决策落 `docs/decisions.md` 的 **217**）

- [x] 逐个决定：详情页签 / 看板过滤 / 班次选择 / 输入草稿 / 其它（若审查中发现）的去向
- [x] 定下 URL 的编码形态（沿用 `?task=` 那种 query，还是路径段），以及与现有路由的兼容
- [x] 定下刷新后的恢复语义：恢复 vs 显式重置（并说明哪个更不会让人困惑）
- [x] 定下后退/前进要恢复哪些状态（至少不让后退把已看的页签打回默认）
- [x] 定下 localStorage 的键名与清理时机（避免无限增长）
- [x] 改交互规格时按惯例标注；若与既有决策冲突，追加修订决策
- [x] 不改任何代码

## 交付

实现票：[22-midflow-persistence-impl.md](22-midflow-persistence-impl.md)（状态 open，**不在本批范围内**；
验收含「刷新恢复」「后退恢复」「URL 可分享」三类断言）。

## 裁决（决策 217 摘要）

**一条判据**：**「我在哪」进 URL，「我平常怎么用」与没写完的草稿进 localStorage。**

| 状态 | URL | localStorage |
|---|---|---|
| 详情页签 | `?tab=`（缺省 `timeline` 不写） | — |
| 看板过滤 | `?filter=`（缺省 `all` 不写） | `agentpipeline.board_filter` 兜底 |
| 对讲台班次 | `?session=` | `agentpipeline.talk_session` 兜底 |
| 对讲台输入草稿 | **不进** | `agentpipeline.talk_draft`（`{sessionId, text, at}`） |
| 会话页签里选中的 run | **不进** | **不进**（页签内部的滚动位置类状态，且自动联动会程序化改它） |

| 定什么 | 结论 |
|---|---|
| 编码形态 | 沿用既有 **query** 形状（`?task=` / `?project=&analyze=1` 已经钉住），**不用路径段**——路径段要动路由表与所有入口链接；值是短枚举，缺省不写 |
| 刷新后的恢复语义 | **恢复**（地址里有就照地址）；地址里没有就用缺省，**不拿 localStorage 去覆盖**。两条例外是「跨页面的工作语境」（过滤、班次）——从看板点进任务再回来时地址会丢参数，而「我一直在看 pending」不该被重置 |
| 后退/前进 | 用户点页签/切过滤/换班次 = `pushState`（**后退回到上一个页签是想要的**）；程序改地址（触发节点直达、打开产出文件、`?task=` / `?project=&analyze=1` 自动就位）= `replaceState`（否则自动联动灌满历史） |
| localStorage 键名与清理 | 只加三把 `agentpipeline.board_filter` / `.talk_session` / `.talk_draft`；草稿**发送成功即清零**、`at` 早于 7 天装载时清零；取值非法（枚举外 / 库里不存在）回落缺省**并删键**；不新增「清空本地状态」界面 |
| 上界 | 只允许这几类短枚举参数；参数总数 ≥5 或出现自由文本时**回来重新裁决**——地址是给人看、给人抄的 |

草稿**不进 URL** 的理由值得单独记：把半句话塞进地址，分享出去的是一个别人看不懂的 URL，
而地址栏还会在打字时被反复改写。
