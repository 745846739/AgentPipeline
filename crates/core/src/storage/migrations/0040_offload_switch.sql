-- 重活外发 GitHub 的全局开关（票 runner-offload/05）。
--
-- 单行表 kanban_offload,与 kanban_rtk（0035）/ kanban_server_bind（0008）同构:
-- 「这台机器上的重活要不要外发」是**机器事实**,不是阶段语义。
--
-- 行不存在 = 从没碰过设置 = **关 = 一切本机运行**（共识的硬要求:默认本机,
-- 开启则在 GitHub 跑;关着的时候行为逐字等于这个开关出现之前）。
CREATE TABLE IF NOT EXISTS kanban_offload (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    enabled    INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);
