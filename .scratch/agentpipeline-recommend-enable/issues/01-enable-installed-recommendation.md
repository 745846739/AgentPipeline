# 01: 推荐行区分「已装未启用」，就地启用

**What to build:** 在「设置 · 模型与密钥」的推荐面板里，一个**已经躺在技能根、但没被本阶段声明**的
推荐技能，行上出现「启用」；点一下把它写进该阶段配置（复用既有落盘入口的「只写配置」分支），行随即
变成只读标签。装/启用完，面板把三项预览照旧摆出来——票 11 的信任门不被绕过。

本机实机判据：`develop` 与 `test` 两行的 `tdd` 出现「启用」（它们只被 test-design 声明过），
其余八行保持现状；点击后那两行的按钮消失、`declared_in` 含本阶段。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] `GET /skills/recommendations` 每行多一个**机器可读**的「本阶段是否已声明」判定；`declared_in`
      原样保留（它还有两个别的调用方，界面也在显示「已启用：阶段 X」）
- [ ] 界面三态：未装 →「安装」（现状不动）；已装 + 本阶段未声明 →「启用」；已装 + 本阶段已声明 →
      只读标签（现状）
- [ ] 「启用」走 `POST /skills/install`（不带 `overwrite`）的「已在技能根里 → 未重新下载，只写配置」
      分支；写进去的声明是 `name` 态 + **未信任**
- [ ] 契约用例覆盖三态；组件用例钉住三个分支与点击后的调用参数（`@testing-library/svelte` 已在，
      `src/**/*.test.ts` 自动收录，组件当前没有用例）
- [ ] 本机实机验收：`develop` / `test` 两行的 `tdd` 变「启用」→ 点击 → 只读；其余八行一字不变
- [ ] 既有锚点不破：`ux-audit.spec.ts` 与 `settings-empty-and-copy.spec.ts` 探的 `.state`「未安装」
      标签、`market-install.spec.ts` 的 `.prev-head` 串
- [ ] 闸门：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、
      `npm run check`、vitest、`vite build` 全过

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
