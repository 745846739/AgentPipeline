-- 决策 212① / 票 12：修复提议——复用提议表，加两种载荷形态与一条**没有 TTL** 的例外。
--
-- 为什么复用 `kanban_foreman_proposals`（决策 212 的原话）：它的每个字段都对得上修复这件事
-- ——`session_id` / `summary` / `situation_json` / `claimed_at` / 四态 `status` / SSE 事件 /
-- 前端确认钮 / 每小时过期清扫。新开一张表等于把 TTL、过期、占用、审计全部重写一遍。
--
-- 两列是「修复」这一形态所需的：
--   * `kind`：`api_call`（默认，原先唯一的那种）与 `repair`。执行点的分派按它走——
--     修复提议执行的不是一次工具调用，而是「合入一个分支」；
--   * `payload_json`：修复的现场（worktree / 分支 / 基准 / 闸门读数 / diff 路径）。
--     它**不是** `args_json`：那份是「工具的调用参数」，而修复没有对应的工具端点。
--
-- **两个与「等你第二天早上看」冲突的性质就地改掉**（决策 212①）：
--   1. **修复类提议不设 TTL**。`expires_at` 列是 NOT NULL，故用一个远期值
--      （`FOREMAN_PROPOSAL_NO_TTL_DAYS` = 100 年）表示「不按时间过期」，
--      真正的生命周期交给年龄清理（与 `conversation_retention_days` 同口径）。
--      不改的话，你早上看到的是一排**灰按钮**，还得自己去合。
--   2. **指纹换义**（写在 `situation_json` 的注释里，语义由代码实现）：
--      普通提议的指纹是「任务状态 + 动作集有没有变」，而修复提议执行的是「合入一个分支」
--      ——分支不会因为别的事变迁而失效。它的指纹换成「**修复分支相对基准还要不要 rebase、
--      会不会冲突**」：执行时先走 merge 阶段已有的 `rebase_onto_with_auto_resolve`，
--      能干净 rebase 就合、冲突就拒执并告诉你冲突在哪几个文件。

ALTER TABLE kanban_foreman_proposals ADD COLUMN kind TEXT NOT NULL DEFAULT 'api_call';
ALTER TABLE kanban_foreman_proposals ADD COLUMN payload_json TEXT;
