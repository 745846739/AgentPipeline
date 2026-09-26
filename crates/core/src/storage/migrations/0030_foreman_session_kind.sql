-- 决策 286（票 foreman-unbounded 01）：值守轮有自己的班次身份——同一套表加一个类型列。
--
-- 形态裁决：**不另起一套表**。列表、消息、提议、SSE 的 `session_id` 路由、归档全都按
-- session_id 组织，加一列即可全部复用；新建表要把这些路径整份抄一遍。
--
-- 取值：`talk`（人的班次，现状语义）/ `watch`（值守台账）。存量行全部落 `talk`
-- ——配合裁决 12「存量不回填」：历史的播报留在原会话里，只有新产生的进值守时间线；
-- 旧行的 `proactive` 派生布尔已能让界面标对（决策 252 的另一半）。
--
-- CHECK 约束与 role 同一姿态：非法值在写入点就被挡住，读取侧不必防坏数据。
ALTER TABLE kanban_foreman_sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'talk'
    CHECK (kind IN ('talk', 'watch'));
