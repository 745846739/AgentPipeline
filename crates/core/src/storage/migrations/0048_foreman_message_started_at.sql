-- 台账行「开工时刻」与「说完时刻」分列（.scratch/foreman-work-record 票 03，决策 422④
-- 的 (b) 版裁决）。
--
-- 事故账（2026-10-08，班次 01M4CDY9EC9M9FSSE236GJXYZC）：assistant 行的 `created_at`
-- 写在**开工**那一刻（`begin_foreman_inflight` 建在途半截行时），收口的 UPDATE 不刷它
-- ——同一对 user / assistant 行只差 10–30ms；394 / 407 两轮各跑 38 / 48 分钟，台账只留
-- 下开工钟点。「谁说的、依据什么」这条可追溯链上，「说的时刻」被记成了「开工的时刻」。
--
-- 形状：另加 `started_at`（开工时刻，`begin_foreman_inflight` 写），`created_at` 归还
-- 「这一行定稿的时刻」（收口的 UPDATE 同时刷成收口时刻）——两个时刻都留，「一轮跑多久」
-- 的读数从两列相减得到。排序照旧按 `created_at`：一轮进行中落下的 system 行（收口时刻
-- 早于本轮说完时刻）自然排到 assistant 行之前，时间序归位。
--
-- **存量行不编数**：旧 `created_at` 里存的恰好就是开工时刻，迁移把它**原样抄进**
-- `started_at`；`created_at` 保持不动——存量行的收口时刻已不可考，不拿 `last_active_at`
-- 之类的近似值冒充（决策 415「读不到不编数」同款姿态）。旧数据「说的时刻」如实缺失，
-- 只剩开工时刻。
ALTER TABLE kanban_foreman_messages ADD COLUMN started_at TEXT;

UPDATE kanban_foreman_messages SET started_at = created_at WHERE started_at IS NULL;
