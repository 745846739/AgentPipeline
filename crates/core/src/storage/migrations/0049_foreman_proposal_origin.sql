-- 决策 427（票 talk-cli-engine 05）：提议的**来路**——它是谁提的。
--
-- `api`：API 值班长进程内工具执行提出的（决策 207 的既有语义，存量行全部落它）；
-- `cli`：CLI 引擎会话经 stdio MCP 代理 + POST /foreman/proposals 进来的（决策 424②）。
--
-- 为什么这一列必须在人按键之前可见：面板上人要分辨「这是机器夜里提的」——CLI 班次的
-- 提议产生在没人盯着的时段，与值班长当面对话里提的案不是同一种信任态。它与
-- `stopped_round`（决策 294）同一族：都是「来路」列，都只加信息、不改执行语义——
-- 执行路径（execute_proposal → run_proposal_tool）按 `(tool, args)` 跑，不看这一列。
--
-- CHECK 与迁移 0046 的 engine 同一姿态：非法值在写入点挡住，读取侧不防坏数据。
ALTER TABLE kanban_foreman_proposals ADD COLUMN origin TEXT NOT NULL DEFAULT 'api'
    CHECK (origin IN ('api', 'cli'));
