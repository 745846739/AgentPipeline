-- 节点内消息日志（.scratch/node-message-resume 票 01）：agent 节点主循环的**逐条**转录。
--
-- **为什么另起一张表**：`kanban_node_conversations.messages_json` 是循环**退出之后**一次性
-- 写成的一整块（成功路径与失败路径各一处），且写入前经 `truncate_messages_json` 从最旧一端
-- 整条丢消息——切口还可能落在轮中间，留下的孤儿 tool 消息会被上游净化静默吃掉。它服务的是
-- **观测归档**，服务不了「从最后一条已记录的消息接着跑」。本表是**只追加**的原始转录：
-- 一行一条消息，行内 `seq` 定序，压缩一个字都不碰它（没有压缩事件可记，因为没有改写）。
--
-- **与游标的分工**（决策 80 的措辞面由票 05 修订）：游标 = **节点级** checkpoint
-- （已完成的节点不重跑），本表 = **节点内** checkpoint（中断的节点不再整节点重跑）。
--
-- **每个 run 自包含**：一次 attempt 起跑时把承接的转录前缀批量落进来，故「某 run 的全部行」
-- 就是那次尝试的完整转录（含它起始时承接的上下文）。与决策 99「一条 run 至多一条会话行」同形。
CREATE TABLE kanban_node_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 指向 run（默认 NO ACTION）：删 run 之前必须先清本表，顺序见
    -- `storage/catalog.rs::delete_project` 的①组（错序会撞外键 787）。
    run_id INTEGER NOT NULL REFERENCES kanban_node_runs(id),
    task_id TEXT NOT NULL,
    stage TEXT NOT NULL,
    node TEXT NOT NULL,
    -- 续接查找键的第三个分量（与 `latest_own_conversation` 同语义）：只有主 agent 的行进本表，
    -- 伪阶段 / 子代理不写——它们是各自 run 的转录，不参与这个节点的续接。
    agent_type TEXT NOT NULL DEFAULT 'main',
    -- 行内序号：同一 run 内从 0 起严格递增，是唯一的排序依据（也是「哪些是承接前缀」的分界）。
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT,
    -- assistant 行携带的工具调用：`ToolCallWire` 的 JSON 数组，`function.arguments` 是**原始串**
    -- （不做二次序列化，还原时逐字段一致地交回 provider）。
    tool_calls_json TEXT,
    -- tool 行携带的回执身份（与上面那条 assistant 声明的 call id / name 对应）。
    tool_call_id TEXT,
    tool_name TEXT,
    -- 「这条是合成回执」（票 02）：日志断在半轮时补齐的那几条。1 = 进程重启补的（模型没说、
    -- 工具没回），0 = 真发生过的。假读数要能被人一眼认出来——与决策 226③「照实记」同一姿态。
    synthetic INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

-- 还原一个 run 的转录：按 (run_id, seq) 顺序读。
CREATE INDEX idx_kanban_node_messages_run_seq ON kanban_node_messages(run_id, seq);

-- 续接查找：按 (task, stage, node, agent_type) 取 run_id 最大的那一组行。键与
-- `latest_own_conversation` 同源——**不能按游标找**：`goto` 是在同一条游标行上改
-- (stage, node)，按游标会把上一个节点的对话喂给这个节点。
CREATE INDEX idx_kanban_node_messages_lookup
    ON kanban_node_messages(task_id, stage, node, agent_type, run_id);
