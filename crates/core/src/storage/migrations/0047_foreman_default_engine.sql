-- 决策 424④（票 talk-cli-engine 04）：对讲台的**默认引擎**（机器级一行表）。
--
-- 形态裁决：与 kanban_foreman_watch（迁移 0012 族，id=1 单行）同一姿态——机器级
-- 设置一行表，不在 kanban_settings 里塞弱类型键值。只放「默认引擎」这一格：
-- 引擎在**建会话时选定**（kanban_foreman_sessions.engine，迁移 0046），这里定的
-- 只是「+ 新班次」按钮不带参数时的缺省值。会话内锁定不归这张表管——锁定是形状
-- （会话行没有改 engine 的写路径）。
CREATE TABLE IF NOT EXISTS kanban_foreman_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    default_engine TEXT NOT NULL DEFAULT 'api' CHECK (default_engine IN ('api', 'cli')),
    updated_at TEXT NOT NULL
);
