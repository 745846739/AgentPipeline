-- 「管线压缩」设置卡的 DB 覆盖层（long-run-budget 票 02）。
--
-- 单行表 kanban_compaction，与 kanban_server_bind（0008）/ kanban_offload（0040）
-- 同构：应用从不写 config.toml（settings-honesty 定下的边界），界面可改的全局
-- 设置走 DB 覆盖。
--
-- 两列都**可空**：NULL = 没保存过 = 回落 config.toml（最终回落缺省 300_000 / 5）；
-- 非 NULL = 界面覆盖。两列独立记账——将来出现只改其中一个旋钮的入口时，
-- 「保存了 A 不该偷改 B 的出处」仍然成立。
CREATE TABLE IF NOT EXISTS kanban_compaction (
    id                      INTEGER PRIMARY KEY CHECK (id = 1),
    conversation_max_tokens INTEGER,
    keep_recent_rounds      INTEGER,
    updated_at              TEXT NOT NULL
);
