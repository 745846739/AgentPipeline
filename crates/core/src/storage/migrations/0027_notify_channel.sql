-- 「离线通知」设置页的存储（决策 272⑥⑦⑧）。
--
-- ## 与 `kanban_server_bind`（0008）/ `kanban_market_repos`（0010）同构的单行表
--
-- 「这台机器把通知发到哪里」是不属于任何任务 / 项目的机器级事实。两级结构（决策 187 /
-- 194 / 186 先例）：本表是**界面那一级**，`config.toml` 的 `[notify]` 是基层——
-- `channel IS NULL` = 没保存过单元（读 `config.toml`），四件一旦保存就**整体覆盖**
-- 基层（不允许混：界面指向 BlueBubbles 而配置说 feishu 不会发生，决策 272⑥）。
--
-- ## 列的形状
--
-- `enabled` 是**一颗总开关**（272⑧：整条通道开/关，非每类一颗）。`CHECK (id = 1)`
-- 让数据库自己拒绝第二行（与 0008 / 0007 同一条不变式进库）。
--
-- `bluebubbles_password` 是**明文**（决策 112 的权衡在此沿用）：DB 目录 0700 /
-- 文件 0600（§12.14），`data/` 整目录对 agent 工具关闭（决策 206 的「密钥库」）；
-- 读回只给 `***` 掩码，掩码或留空 = 不改。

CREATE TABLE IF NOT EXISTS kanban_notify_channel (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    enabled               INTEGER NOT NULL DEFAULT 1,
    channel               TEXT,
    webhook_url           TEXT,
    bluebubbles_url       TEXT,
    bluebubbles_password  TEXT,
    bluebubbles_recipient TEXT,
    updated_at            TEXT NOT NULL
);
