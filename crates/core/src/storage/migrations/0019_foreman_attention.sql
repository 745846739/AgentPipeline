-- 决策 209③ / 票 05：值班长待办（一事件一行）。
--
-- 为什么必须有这张表：调度器每 10 秒的 tick 里**已经有发现器**（超时 / 提醒 / stalled /
-- 慢跑告警），但它们只写日志，而 `TickReport.reminded` 是**内存里的 HashSet**——
-- 重启即失、从不推 SSE。于是「任务停滞超过 `pending_reminder_hours`（默认 24h）」这件
-- 事，今天没有任何人会知道。表是通道，日志不是。
--
-- 落表的第二个理由：它给「这条我处理过没有」一个**可查的答案**。`consumed_at IS NULL`
-- 即未处理，值守轮（票 06）消费它就置位。
--
-- 两处不在票面字段列表里的列，是**实现这条规则本身**所需的：
--   * `occurred_at`：事件的**发生时刻**，也是去重键的一半（唯一索引）。
--     没有它，`created_at`（写待办的时刻）会把两件完全不同的事混成一个数：
--     「同一次 pending 被每一 tick 重复写」与「同一任务 30 分钟内真的 pending 了两次」
--     ——前者会让唤醒刷屏，后者正是「自动修复没治好」的判据（§4.9）。
--   * `detail_json`：事件现场（pending 原文 / run id / 闸门失败种类），播报与诊断都要它。
--
-- 保留期不另设：年龄清理接进既有的每小时维护作业，与 `conversation_retention_days` 同口径
-- （与对讲台其余各表一致的姿态：不做全仓唯一一张不设保留期的表）。

CREATE TABLE IF NOT EXISTS kanban_foreman_attention (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    -- 事件类别（稳定标识，见 `storage::attention::AttentionKind` 的 as_str）。
    kind        TEXT NOT NULL,
    -- 事件发生时刻（RFC3339）。与 kind / task_id 一起构成去重键。
    occurred_at TEXT NOT NULL,
    detail_json TEXT,
    created_at  TEXT NOT NULL,
    consumed_at TEXT,
    FOREIGN KEY (task_id) REFERENCES kanban_tasks(id)
);

-- 同一件事只写一行：同一 (task, kind, occurred_at) 重复写是 no-op。
CREATE UNIQUE INDEX IF NOT EXISTS idx_foreman_attention_event
    ON kanban_foreman_attention(task_id, kind, occurred_at);

-- 值守轮按「未消费 + id 升序」取（票 06）。
CREATE INDEX IF NOT EXISTS idx_foreman_attention_open
    ON kanban_foreman_attention(consumed_at, id);
