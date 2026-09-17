-- 决策 188 / 207：值班长的**提议**——写动作不直接执行，先落一条「打算做什么」，由人按下。
--
-- 为什么必须有这张表（决策 188 的第一约束）：`allowed_actions` 由后端下发、**LLM 的判断绝不
-- 直接接进状态机**。值班长扩到能碰文件与系统接口之后，那条接缝靠这张表维持——模型能做的
-- 只是**提议**，提议要经后端校验、由人按下「执行」、再走既有的那条端点。库里存的是意图，
-- 不是已经发生的事。
--
-- 四件事的落点（决策 207）：
--   * TTL 10 分钟（比照 resume cooldown 的姿态）：`expires_at` 由写入方算好；
--   * **重启后保留**：落库天然持久，不另立「重启即作废」规则——故表里没有任何进程代际列；
--   * **过期只让按钮变灰**：状态从 pending 变 expired，**行不删**，那一轮留在时间线里
--     （审计价值与 briefing_json / traces_json 同一理由：值班长当时提议过什么必须可追溯）；
--   * **不做去重**：连着提两次就是两条、人各按一次——去重会让「我拒了它、它又提了一次」
--     这个真实情形不可表达。
--
-- 两列不在决策 207 的字段列表里，是**实现这条规则本身**所需的：
--   * `situation_json`：提议成立时的态势指纹（任务状态 + 后端此刻下发的动作集）。
--     拒执判据「现在的情况已经不是它当时说的那样」要有东西可比，否则这句话无从成立；
--   * `claimed_at`：`execute` 的原子占用。四个 `status` 是**结果**的词汇表，不是「正在执行」
--     的词汇表——把在途状态塞进 status 会让「执行失败后回到 pending」看起来像状态抖动。
--
-- 保留期：不在这张表上另设一套——过期清扫与年龄清理都接进既有的每小时维护作业，
-- 年龄清理与 `conversation_retention_days` 同口径（对讲台不做全仓唯一一张不设保留期的表）。

CREATE TABLE IF NOT EXISTS kanban_foreman_proposals (
    id             TEXT PRIMARY KEY,
    -- 提议挂在会话上（决策 204 的兑现点）：一次值班 = 一个会话，提议是这一班里提的。
    session_id     TEXT NOT NULL REFERENCES kanban_foreman_sessions(id),
    tool           TEXT NOT NULL,
    args_json      TEXT NOT NULL,
    -- 一句话说明（人读的那句：要动什么、为什么）。执行前给人看的就只有它和参数摘要。
    summary        TEXT NOT NULL,
    -- 态势指纹（见上）。只对参数里带 task_id 的提议成立——其余提议没有「态势」可判。
    situation_json TEXT,
    -- execute 的原子占用时间戳。非空 = 这一次执行正在跑（或刚跑完还没落 status）。
    claimed_at     TEXT,
    created_at     TEXT NOT NULL,
    expires_at     TEXT NOT NULL,
    status         TEXT NOT NULL,
    resolved_at    TEXT,
    CHECK (status IN ('pending', 'executed', 'rejected', 'expired'))
);

-- 时间线按会话取（`GET /foreman/session` 把提议行并进轮次里），清扫按过期时间扫。
CREATE INDEX IF NOT EXISTS idx_foreman_proposals_session
    ON kanban_foreman_proposals(session_id, id);
CREATE INDEX IF NOT EXISTS idx_foreman_proposals_expiry
    ON kanban_foreman_proposals(status, expires_at);
