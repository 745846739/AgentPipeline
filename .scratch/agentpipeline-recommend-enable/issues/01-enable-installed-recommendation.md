# 01: 推荐行区分「已装未启用」，就地启用

**What to build:** 在「设置 · 模型与密钥」的推荐面板里，一个**已经躺在技能根、但没被本阶段声明**的
推荐技能，行上出现「启用」；点一下把它写进该阶段配置（复用既有落盘入口的「只写配置」分支），行随即
变成只读标签。装/启用完，面板把三项预览照旧摆出来——票 11 的信任门不被绕过。

本机实机判据：`develop` 与 `test` 两行的 `tdd` 出现「启用」（它们只被 test-design 声明过），
其余八行保持现状；点击后那两行的按钮消失、`declared_in` 含本阶段。

**Blocked by:** None (can start immediately)

**Status:** done（2026-09-17，提交 0142a26；实机验收与闸门读数见文末「实施收尾」）

- [x] `GET /skills/recommendations` 每行多一个**机器可读**的「本阶段是否已声明」判定；`declared_in`
      原样保留（它还有两个别的调用方，界面也在显示「已启用：阶段 X」）
- [x] 界面三态：未装 →「安装」（现状不动）；已装 + 本阶段未声明 →「启用」；已装 + 本阶段已声明 →
      只读标签（现状）
- [x] 「启用」走 `POST /skills/install`（不带 `overwrite`）的「已在技能根里 → 未重新下载，只写配置」
      分支；写进去的声明是 `name` 态 + **未信任**
- [x] 契约用例覆盖三态；组件用例钉住三个分支与点击后的调用参数（`@testing-library/svelte` 已在，
      `src/**/*.test.ts` 自动收录，组件当前没有用例）
- [x] 本机实机验收：`develop` / `test` 两行的 `tdd` 变「启用」→ 点击 → 只读；其余八行一字不变
- [x] 既有锚点不破：`ux-audit.spec.ts` 与 `settings-empty-and-copy.spec.ts` 探的 `.state`「未安装」
      标签、`market-install.spec.ts` 的 `.prev-head` 串
- [x] 闸门：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、
      `npm run check`、vitest、`vite build` 全过（clippy 一项有一处**与本票无关**的既有红，见「实施收尾」）

**Notes（实现提示）:**

- **为什么判定必须是后端给的字段，不能让界面自己算**：`config::declared_skill_where` 回的是**展示串**
  （`阶段 <key>` / `阶段 <key> 节点 <node>`），界面要判断「本阶段」就得 parse 它——那正是票 03 明令禁止
  的（「界面按 `kind` 分支，不要按状态码、更不要按 `error` 里的字样」）。判定算法该和它同族、住在 core
  里（值班长的 `read_skills` 与 `GET /skills` 共用同一处，别在路由里另写一份）。
- **「已装但本阶段未声明」不是异常态，是这条链路最常见的中间态**：本机 10 行推荐里 8 行都处在这个态
  的邻域。票 16 第 14 条那句「已安装的可直接启用」指的就是它，后端分支与用例
  （`one_click_install_enables_an_already_installed_skill`）早已存在，缺的只有界面控件。
- **不要给「已装 + 本阶段已声明」的行加禁用按钮占位**：那会让十行里八行都是死控件，比只读标签更吵。
- `develop` 行的现状最能说明问题：那一行显示「已启用：阶段 test-design」，在 develop 行上读起来是
  误导的。加了「启用」钮之后这行才自洽——tdd 装好了，但 develop 阶段还没启用它。

---

## 实施收尾（2026-09-17）

**落了什么**（提交 `0142a26`，票面文档 `d1dc194`）：

- core：`config::skill_declared_in_stage(configs, stage, name)`——按阶段问「声明没有」，与
  `declared_skill_where` 同源（都走 `declared_skill_decls`，坏来源只作废那一处）。
- 路由：`GET /skills/recommendations` 每行加 `declared_here`（布尔），`declared_in` 原样保留。
- 界面：组件按三态渲染，两个词（安装 / 启用）打同一个回调；**本阶段没启用而别处启用了**的行，
  标签由「已启用」改成「已被引用」（那一行旁边正给着一颗「启用」钮，两个词不能同时出现）。
- 用例：`config.rs` 单测一条（按阶段而非按名字 / 节点级也算 / 坏来源只作废自己）；
  `api_contract.rs::recommendations_report_whether_each_skill_is_declared_in_that_stage` 五格；
  `components/settings/StageRecommendations.test.ts` 六条（三态 + 点击参数 + 忙时禁用）。

**实机验收（真 bundle + 真后端）**：为了让证据对应最终构建，且不碰正在用的那份配置，把真实 home
**复制**一份、用真二进制在新端口起服、用仓里自己的 Chromium 真点：

| 读数 | 结果 |
|---|---|
| 点击前 | 「启用」2 枚（`develop` / `test` 两行的 `tdd`），其余八行只读，无「安装」钮 |
| 点 `develop` 那枚 | 该行变「已启用：阶段 develop、阶段 test-design」，钮消失；全页剩 1 枚 |
| 落库 | `develop` 的 `skills_json` 由 `[resolving-merge-conflicts]` 变**追加**后的两项（原有条目保留） |
| 对照 | 真实那份 home（8788）一字未动 |

**闸门读数**：`cargo test --workspace` 全绿（39 个二进制，退出码 0）；前端 vitest **519**、
svelte-check 0 error / 0 warning、`vite build` 过；playwright 全量 **83 passed / 15 skipped**
（与文档基线一致，`.state`「未安装」等既有锚点未破）。

**一处与本票无关的既有红**：`make check-lint` 的 clippy 一段在 HEAD 上就失败——`crates/core/src/git.rs`
两处常量断言（`IS_DIRTY_TIMEOUT_SEC < GIT_OP_TIMEOUT_SEC` 等，d749f3a 引入）撞 clippy 1.98 的
`assertions_on_constants`。加上 `-A clippy::assertions_on_constants` 后工作区干净（含本票改动的
三个文件）。**没有顺手改它**：那两处断言的形态（`const` 块还是 `#[allow]`）是那一条决策的事，
本票不夹带。

**两处代码评审的收口**（评审两轴跑完，Standards / Spec 各一条落到本票）：

1. 两个按钮分支逐字重复 → 收敛成「一颗钮 + 两个小函数（要不要给钮 / 钮上写哪个词）」。
2. 组件注释原写「按下去只写配置，不重新下载」——**言过其实**：后端只在「技能根里那份就是清单这份」
   （含没有来源记录那种）时才跳过下载；来源记录指着别的仓时它会照常去装，由同名冲突门交用户裁决。
   注释已改成不承诺零网络（行为未变，`market.rs` 早有那两条用例钉住）。

**留给票 03 的**：`design/frontend-design.md` §12.3 的行为映射表加行、`docs/testing.md` 用例目录、
决策落表（「启用」的判据是本阶段是否声明，而非是否安装），以及回填票 16 第 14 条。
