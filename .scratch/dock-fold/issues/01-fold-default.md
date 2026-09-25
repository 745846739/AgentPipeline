# 01: 坞默认收成一行手柄

**What to build:** `PendingDossier` dock 模式默认收成一行手柄（`aria-expanded` 翻转，
▲/▼ 闪烁光标沿用对话框语汇），展开层内容一字不动；`app.css` 加 `.dock-head`
（44px 触控、光标伪元素收在坞语汇那一块）；`TaskDetail` 的 `--dock-h` 兜底值跟着
收起态改小；e2e `pending-dossier.spec.ts` ④ 改为「手柄直接可见、点开即见合入，
收起态动作面不在 DOM」；新增单测 `PendingDossier.test.ts` 钉收起 / 展开 /
动作接线 / 桌面档案盒不受折叠影响。

**Blocked by:** 无

**Status:** done（2026-09-25）

**要点：**

- 决策 281。被否的备选：把 `.dock` 上限从 72vh 压到 40vh（挡屏仍在，只是少一点）；
  说明挪回正文流（转写 3 原文那样）——动不了按钮与 textarea 的份量，坞仍 250px 起。
- 坞语汇（琥珀顶框、闪烁 ▼）不丢：收起态 ▲（往上展开）、展开态 ▼（沿用原那笔）。
