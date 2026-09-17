# 12: 修复提议（复用提议表 + TTL 与指纹换义 + 前端钮）

**What to build:** 把「一次修复」做成一条**待你按的提议**，复用决策 207 已经造好的那张表。

复用 `kanban_foreman_proposals`（`session_id` / `summary` / `situation_json` / `claimed_at` /
`expires_at` / 四态 `status` / SSE 事件 / 前端确认钮 / 每小时过期清扫 / `claimed_at` 原子占用）
——它的每个字段都对得上修复这件事，新开一张表等于把 TTL、过期、占用、审计全部重写一遍。
**要加的是载荷里多一种「diff」形态**（现在只有 `tool` + `args_json`）。

**但那张表有两个为「当下这一刻」设计的性质，与「等你第二天早上看」直接冲突，必须改：**

1. **TTL 是 10 分钟**（`crates/core/src/storage/proposals.rs:22`）。修复类提议**不设 TTL**，
   只随年龄清理（与 `conversation_retention_days` 同口径 = 30 天）。不改的话，你早上看到的会是
   一排**灰按钮**，还得自己去合。
2. **`situation_fingerprint` 的拒执判据是为「改任务状态」设计的**（任务状态或 `allowed_actions`
   变了就别执行）。修复提议执行的是「合入一个分支」，分支不会因为别的事变迁而失效——
   **指纹的含义要换成「修复分支相对 base 是否还要 rebase、会不会冲突」**。

**Blocked by:** 11

**Status:** ready-for-agent

- [ ] 载荷加「diff」形态（`kind` 区分 `api_call` 与 `repair`），`summary` 仍写人读的那一句
- [ ] 修复类提议**不设 TTL**（`expires_at` 置空或远期），年龄清理进既有每小时维护作业
- [ ] 指纹换义：执行时先走 merge 阶段已有的 `rebase_onto_with_auto_resolve`
      （`crates/core/src/pipeline/executor.rs:577-591`）——能干净 rebase 就合，
      冲突就拒执**并告诉你冲突在哪几个文件**
- [ ] 指纹仍要落库（复用 `situation_json` 这一列，语义在迁移注释里写清）
- [ ] `claimed_at` 原子占用沿用（一次一按）
- [ ] 「拒绝」与「过期」都**保留分支、删 worktree**（票 10）
- [ ] 前端（`frontend/src/routes/Talk.svelte` + `frontend/src/lib/proposals.ts`）：
      内联在对话时间线那一轮里，**复用急停轮的 `.warn` 形态与 `PendingActions` 的渲染**
      （决策 207③），**不新开第三个按钮面**；同一个动作若已在状态区有钮，**只渲染指路**
- [ ] 前端要能展开看 diff（不要求语法高亮，但要求**看得全**、能复制）
- [ ] 闸门读数（lint / test 过没过、耗时）显示在那条提议上
- [ ] 决策 208 的几何断言不许被顶坏（1280×720 下那颗钮**点得到**，
      回归门是 `frontend/e2e/talk.spec.ts` 的 `expectProposalReachable`）
- [ ] 新增用例：修复提议在你「睡过一夜」后**仍是 `pending`、按钮**仍可点
- [ ] 新增用例：base 前进了 → 执行时自动 rebase 后合入成功
- [ ] 新增用例：rebase 冲突 → 拒执，且理由里列出冲突文件
- [ ] 新增 e2e：修复提议的钮在默认视口下可点（并入 `expectProposalReachable`）

## 备注

**没有 TTL 的那条规则要写在明面上**：它让修复提议与别的提议行为不同。同一个界面上两种过期语义，
如果不写清，下次看到一条「过期」的提议会以为修复也一样——而修复恰好是**唯一一条你有意留给
自己第二天早上看的**。
