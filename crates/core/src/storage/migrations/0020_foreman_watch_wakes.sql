-- 决策 209⑤ / 票 07：值守轮的**唤醒账**。
--
-- 为什么需要它（而不是「数一数会话行」）：
--   * 全局唤醒上限（默认每小时 12 次）要数的是**花钱的次数**，而「判定无需处理」那一轮
--     **不落会话行**（§2.4 的静默规则）却照样烧 token。数会话行会漏掉它——上限就形同虚设。
--   * 票面要求「这周值守花了多少」答得出来：token 记在播报那一行上（人的回话也记在
--     同一种行上），两者只有在这里才分得开。
--   * 触顶时写的那条「本小时已达上限，N 条待办未播报」也要有个地方记，且**同一小时内
--     只记一次**——否则触顶本身会变成新的刷屏源。
--
-- `outcome` 三态（稳定标识）：
--   * `broadcast`：播报了一轮（会话里有一条 `【值守播报】` 的 assistant 行）；
--   * `silent`：醒了、判定无需处理（不落会话行，但花了 token）；
--   * `capped`：触顶，**没醒**（`event_count` 是那批未播报的待办条数；token 恒为 0）。
--
-- 保留期与其余各表同口径（`conversation_retention_days`），年龄清理在每小时维护作业里。

CREATE TABLE IF NOT EXISTS kanban_foreman_watch_wakes (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id        TEXT,
    outcome           TEXT NOT NULL,
    event_count       INTEGER NOT NULL DEFAULT 0,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL,
    CHECK (outcome IN ('broadcast', 'silent', 'capped'))
);

CREATE INDEX IF NOT EXISTS idx_foreman_watch_wakes_time
    ON kanban_foreman_watch_wakes(created_at);
