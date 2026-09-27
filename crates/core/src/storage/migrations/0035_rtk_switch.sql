-- 决策 297（票 03 / 04 / 05）：命令执行走 rtk 的全局开关。
--
-- 单行表，与 kanban_server_bind（0008）/ kanban_notify_channel（0027）/
-- kanban_foreman_watch（0031）同构：「这台机器上要不要用这个过滤器」是**机器事实**，
-- 不是「这个阶段在干什么」的阶段语义——阶段级正是为后者设的（决策 206 的原话）。
-- 阶段级还要多付一列迁移 + 三层解析 + 前端控件 + 跨语言 spec 表，换来的是今天说不出
-- 谁需要的粒度。
--
-- 行不存在 = 从没碰过设置 = **关**（缺省关：一个会改写命令串的优化器要人显式打开；
-- 「默认关的时候行为逐字等于今天」是票 03 的判据）。
--
-- `path` 是手填的兜底：自动解析（服务进程 PATH → 五个已知目录）都失败时，界面让人填一个
-- 绝对路径。**填了就覆盖自动解析**——装了新旧两个 rtk 的人要能指定用哪个；填错时如实报错，
-- 不静默回落到自动解析（票 04 的判据）。NULL = 从没填过，不是「填了一个空路径」。
CREATE TABLE IF NOT EXISTS kanban_rtk (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    enabled    INTEGER NOT NULL,
    path       TEXT,
    updated_at TEXT NOT NULL
);
