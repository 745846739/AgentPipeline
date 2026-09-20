# 票面状态核查（2026-09-20）

**起因：** 问「还有哪些 ticket 没有实现」。
**方法：** 只读清点 `.scratch/` 下全部票面与各 effort 的 `README.md`，逐条与 `crates/` / `frontend/`
的实际代码、资产、提交对账。**未改动任何实现代码。**

## 〇、先说口径：两个数字会互相打架

| 读法 | 结果 |
|---|---|
| 数复选框（`- [ ]`） | 23 个目录、203 个票文件、**1246 个 `[x]` / 168 个 `[ ]`** |
| 读票级 `**Status:**` | 仅 **2 张** 不是 `done` |

两个数差了三个数量级，因为**绝大部分 `[ ]` 是账没结，不是活没干**。下列判据按可信度排序：

1. **票级 `**Status:**` 行 + 代码/资产在位**——唯一可信。
2. **effort 的 `README.md` 状态行**——**会漂，本次实测已抓到两处硬错**（见 §二）。
3. **复选框**——只是实现者的过程账，落后于实际是常态。

## 一、真正的缺口（只有两条）

### 1. `foreman-watch` 票 11 / 12 —— `partial`，卡在一次授权裁决而非实现

- `issues/11-repair-gate-commit.md`：`**Status:** partial`。闸门与交付全在，**只做了「等合入」那一路**，
  差「目标项目**当场生效**」那一路（理由见该票文末 2026-09-18 补记）。
- `issues/12-repair-proposal.md`：`**Status:** partial`。后端 / 前端 / 线上入口 / e2e 几何量测都已在，
  只差同一条「目标项目当场生效」——票面自己写明「**它不是实现缺口，而是一次授权裁决**」。
- 其余 12 张票 `done`。

**结论：** 这不是「没写代码」，是等一个决定（要不要允许修复直接落进目标项目的工作区）。裁决完即可收口。

### 2. `agentpipeline-v2-l4/issues/02-unknown-tool-fail-fast.md` —— 被取代的旧副本

同目录存在两个同号票：

| 文件 | Status |
|---|---|
| `02-unknown-tool-fail-fast.md` | `ready-for-agent`（**旧副本，已废**） |
| `02-unknown-tool-name-fail-fast.md` | `done（2026-09-17）` |

被取代的旧副本仍留着 `[ ]`，容易被清点当成未实现。实现确已在位：
`crates/core/src/config.rs:1171-1177` 用 `is_known_tool_name` + `unknown_tools_message` 在启动/写入路径报错，
并有 `unknown_tool_names_fail_startup_validation` 用例。

**建议：** 把旧副本改名加 `-superseded` 或在头部加一行「已被 `02-unknown-tool-name-fail-fast.md` 取代」，
否则下一次清点还会再骗人一次。

## 二、README 状态行订正（本次已改）

| effort | 原文 | 实际 | 依据 |
|---|---|---|---|
| `run-command-permissions` | 「四张票均未开工」 | **4/4 票 done** | 票级 Status 全 done；`2aec356`（2026-09-17）交付：`EnvMode`、`gate_decision`（执行/提议/拒绝三分支）、迁移 `0016_env_mode.sql`、`StageConfigForm.svelte` 档位下拉 |
| `agentpipeline-pixel-theme` | `ready-for-agent` | **13/13 票 done** | 票级 Status 全 `done（2026-09-14）`；提交 `612cc07 → b251817 → 12addd6 → fc5f663 → fb9a47e → 8e1ae04`；`frontend/public/fonts/fusion-pixel-12px/`、`design/deprecated/theme-3-terminal.md` 都在位 |

两处 README 已就地订正，并保留了删除线式的原文说明（本项目惯例：不静默改写历史口径）。

## 三、账没结、活已干的（按 effort 汇总）

| effort | 未勾 | 票级 Status | 实况 |
|---|---|---|---|
| `talk-mobile-space` | **92** | 10/10 `done` | 票 10 逐条对上源码：`Talk.svelte:581` 的 `switchTo` 已无 `sending \|\| busy` 锁、两枚标记（`sendingSid` / 已读时刻表）都在。**纯过程账落后** |
| `ux-audit-2` | 36 | 18 `done` / 4 `open` | 票 06 的 9 个空框逐条已落地：`aria-current`（TopBar.svelte:304）、12 处 `<main>`、看板与 404 的 `<h1>`（Board.svelte:147 / App.svelte:91）、每路由 `document.title`（App.svelte:50）。4 张 `open` 是真未做，见 §四 |
| `agentpipeline-github-market` | 12 | 4/4 `done` | 12 个空框全在 `background.md`（**背景材料，不是验收条件**）——无票面勾选负担 |
| `ux-audit` | 12 | 27/27 `done` | 其中 13 条是「不做 / 无法判定」类（见 §五），其余为真缺的覆盖形式（见 §四） |
| `foreman-watch` | 1 | 12 `done` / 2 `partial` | 那 1 条目在 `partial` 票 11 里，与 §一 同源 |
| `resume-semantics` | 3 | 4/4 `done` | 票 01 两条（超时耗尽后人工按键=\>续接、冲突等待/依赖失败的自动放行）缺测试钉住；票 04 一条缺 `allowed_actions` 断言 |
| `agentpipeline-v2-l4` | 8 | 2 `done` / 1 `ready` | 8 个空框全在被取代的旧副本里（见 §一.2） |
| `agentpipeline-recommend-enable` | 1 | 3/3 `done` | 唯一一条已划删除线并写明「做不到，理由就地留痕」 |
| `foreman-capabilities` | 1 | 7/7 `done` | `tests/foreman.rs` 断言改写为清单驱动 + forbidden 反向断言——已在后续批次重写，属账未结 |
| `agentpipeline-v1` | 1 | 22/22 `done` | 「Vite 脚手架对齐 §3.1」标注**无法判定（形态已被取代）** |
| `agentpipeline-v1-closeout` | 1 | 18/18 `done` | 「本票不夹带行为变更」标注**无法判定**（对当时过程的断言，非产品行为） |
| 其余 8 个 effort | 0 | 全 `done` | 账已结清 |

## 四、真缺的「活」（量很小，且多是覆盖形式而非功能）

### 4a. `ux-audit-2` 四张 `open` 票

| 票 | 内容 | 核实 |
|---|---|---|
| 18 | 详情页 480–819px 中间档折行 + hero 轨道不撑破页面 | 前端搜不到 `1100px` 断点，**未实现** |
| 19 | 对讲台 480–899px 中间档折行 | 同上，**未实现** |
| 21 | 不可逆动作的确认步与三档量级 | 搜不到确认组件，**未实现** |
| 22 | 中流状态留存与深链（页签 / 过滤 / 班次 / 草稿） | 搜不到对应持久化，**未实现** |

18 / 19 是**缺陷类**（页面横向被挤破 / 撑出滚动），21 / 22 是**规格已有、实现未跟**（决策 216 / 217 已定形态）。

### 4b. `ux-audit` 里的覆盖率缺口（功能可用，缺票面要求的断言形式）

- 票 06 / 07：从任务详情**点进去**的 e2e——现用例直接 `goto('#/metrics?task=…')`，
  点击链路只有单测钉 `href`。
- 票 13：空态 e2e 只断言文字与 `href`，**从未点击**；窄屏版面无一断言。
- 票 05：逗号分隔多依赖的候选只对正在输入那一段生效——**明确「未做」**（原生 `datalist` 按整串过滤）。
- 票 12 / 27：收敛**后**的截图证据未拍（现存图是改动**前**的，重跑会覆盖票 01 的对照物）。

## 五、按裁决「有意不做」的（不应计入未实现）

13 条 `[ ]` 自带理由，属 **unreachable-by-design**：

- **裁决票与实现票同一提交，机器隔离不出**（`ux-audit` 票 20 / 22 / 24 / 26；`agentpipeline-v1-closeout` 票 03；
  `agentpipeline-v1` 票 20）——对**过程**的断言，不是对产品的断言。
- **票面写「每个」，裁决收窄了**（`ux-audit` 票 25：12 个词里 6 个有屏上译文，另一半是决策 200④
  明文不翻译；票 05 保留 `role="menu"` 那条已降级为 disclosure）。
- **做不到 / 暂时不给**（`ux-audit-2` 票 17 的 toast 关闭钮；`agentpipeline-recommend-enable` 票 02 的 null 情形）。

这些框**不该打勾**（打了就是伪造成果）。建议统一改成 `` - [~] 不适用：… ``，
让「未实现」的计数只统计真正待办。本次未改（会动 13 个文件，且 issue-tracker.md 只定义了 `[ ]` / `[x]` 两态）。

## 六、建议的下一步（按性价比排序）

1. **裁决 foreman-watch 那条「目标项目当场生效」**——它是唯一挡住两票收口的决定，不裁决就一直是 `partial`。
2. **清 `agentpipeline-v2-l4` 的旧副本**——一次改名，永绝后患。
3. **`ux-audit-2` 票 18 / 19 优先**——缺陷类，用户能直接撞上（窄窗口页面横向被挤破）。
4. **把 §三 的账一次性结掉**——按票级 Status 回填勾选或就地划删除线，让「168」这个数掉回真实待办。
5. **`issue-tracker.md` 增第三态 `[~]`（不适用）**——否则这三类东西永远混在一个计数里。
