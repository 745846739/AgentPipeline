# 07: 急停对话框 + 工头头像

**What to build:** 这是全站视觉权重最高、也是唯一「响」的东西：当任务停下来等人决策，
pending 面板渲染成一台操作台的对话框——奶油双线框、压在框沿上的琥珀名牌 tab、闪烁的 ▼ 光标，
左边坐着一名 16×16 工头。恢复动作变成对话框菜单项按钮，主导航键带 `▶`。做完后
「哪里在等我拿主意」这件事不靠读文字就能看出来。

**Blocked by:** 05 看板传送带 + 货箱 + token 量表 + boss 条 + 列头小人

**Status:** done（2026-09-14）

- [x] **急停对话框**：pending 面板（dossier）与看板卡上的 pending 理由渲染成 RPG 对话框——
      双线框 + 压在框沿上的琥珀**名牌 tab**（FF 式）+ ▼ 闪烁光标；琥珀仍是全站唯一告警色
- [x] 对话框左侧 16×16 **工头头像**（琥珀安全帽 + 绿背心 + `--t2` 底座）；
      浅色款按 theme-6-pixel.md §2.4 偏差②脸块固定 `#E3C7A6`（不用 `--text-hi`，否则浅色下变墨块）、
      眼睛保持 `--ink`
- [x] 阻塞原因（`pending_reason.message`）在对话框内呈现，字号与层级沿用像素字阶（12 / 24 / 36）；
      诊断信息（如 provider 报错与「诊断：」行）仍然逐字可见，**不得被对话框样式吞掉**
      （`provider-misconfig.spec.ts` 断言 `.msg` 与 `.ctx` 的内容）
- [x] **恢复动作 = 对话框菜单项按钮**：主动作带 `▶` 前缀；两类动作（resume 类 vs 旁路类）
      视觉分组、旁路动作弱化（决策 69/70）；`requires_input` 的行内输入框保留；
      多游标时的游标选择器保留（决策 91）
- [x] **`allowed_actions` 纯渲染逐字不变**（决策 69/101）：前端不做动作白名单；
      动作集按游标独立下发；`cursor_id` 从所属分支取（决策 91）
- [x] 异步按钮：点击即 loading 禁用、SSE 回执后复位，语义不变
- [x] 分支分组头（`.head-label`）与 05 的分支徽章统一（同一套 token）
- [x] `DeltaReviewPanel` / `ReviewForm` / `PendingActions` 的挂载方式与回调签名不变——
      看板卡与详情 dossier 共用它们
- [x] 既有 vitest（`PendingActions.test.ts`）与 playwright（`pending-resume` /
      `provider-misconfig` / `happy-path` 的 dossier 断言）全绿
