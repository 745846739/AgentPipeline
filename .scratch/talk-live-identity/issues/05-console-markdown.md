# 05: 操作台轮型文本过 MarkdownView

**What to build:** 操作台名下的轮型文本与值班长回话同待遇——提议轮 summary
（`frontend/src/routes/Talk.svelte:2138`）、提问轮 content 与 question（2229 / 2231）、
急停轮的 message 组句（1984）、任务页 `PendingDossier.svelte:85` 的 `reason.message`
统一过 `MarkdownView`。判据：这些字段的作者多半是模型，「模型不写 markdown」是无法
执行的纪律；与决策 274（值班长产出按含格式预期渲染）同一方向。

**Blocked by:** None（可与 01 并行，改的都是模板插值行）

**Status:** done（已实现，决策 317）

- [x] 提议轮 / 提问轮 / 急停轮 / `PendingDossier` 的四处插值改 `MarkdownView`
- [ ] 视觉回归核对：`MarkdownView` 的 80ch 宽度与段距在提议轮的紧凑版式里不破格
      （必要时容器内收窄，参照 `MessageBubble.svelte:110` 的既有做法，不改公共宽度）
- [ ] 快照 / e2e 里涉及的文案断言更新（纯文本 → 渲染后 DOM）

## Comments

- 2026-09-29 实施完毕（决策 317）。四处插值过 `MarkdownView`（对讲台提议 / 提问 / 急停 +
  `PendingDossier`；console 记账同待遇、失败报文照旧纯文本）；`.msg` 字色档退场。
  快照 / e2e 断言核对：全仓 vitest 983 支全绿，无文案断言受影响。
