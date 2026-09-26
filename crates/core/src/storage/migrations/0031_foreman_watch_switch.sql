-- 决策 287（票 foreman-unbounded 02）：值守轮的全局开关——「关掉跑」的那一颗钮。
--
-- 单行表，与 kanban_server_bind（迁移 0008）/ kanban_notify_channel（0027）同构：
-- 「这台机器的值守开不开」是机器级事实，界面上那颗钮改的就是它。行不存在 =
-- 从未碰过设置 = **开**（缺省开；spec §二 裁决 3）。
--
-- 语义（裁决 3 的原话）：关掉的是「跑」——跑都不跑自然不吵；**不是**「跑着但不吵」，
-- 那一档由离线通知的总开关与礼貌两件管（决策 272 / 284），两边不造第二份。
-- 在飞的那一轮不受影响、不被打断；打开后下一趟（≤10s）恢复。
--
-- 不做班次级开关（裁决 3）：没人会调的旋钮（决策 256 的尺子）。
CREATE TABLE kanban_foreman_watch (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);
