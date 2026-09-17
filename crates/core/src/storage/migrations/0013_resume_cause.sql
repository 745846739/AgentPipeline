-- 决策 205：会话续接改由**原因**驱动——`resumed_from_pending` 这个 bool 换成原因列。
--
-- 为什么要换：续不续接上一段对话以前由阶段 / 节点参数（`stage_configs.resume_continuation`）
-- 决定，而那个判据把**原因**抹掉了——「信息不足被打回」与「合入提案通过」在 bool 眼里
-- 是同一件事，可两者对模型的意义完全不同（前者要接着补，后者是新的一段执行）。
-- 原因表本身不在这里（它是代码里的硬编码表，见 `types.rs::resume_continues`），
-- 这一列只负责把「刚离开 pending 的是哪个原因」如实记下来。
--
-- 换列而不是加列：并存 bool + kind 两列迟早会漂移（一个说置过位、一个说是哪个原因），
-- 而这两列回答的本来就是同一个问题。
--
-- 历史行：迁移前若正好有一次 resume 还没被取走（`resumed_from_pending = 1`，
-- 只在「resume 之后、下一次进入该节点之前」这个窗口里成立），它的原因已经无从考证，
-- 记成 `unknown`——判定表把 `unknown` 放在 **false** 那一档，于是它退回「续接出现之前的
-- 行为」（干净起跑）。宁可少省一点 token，也不让模型带着一段来历不明的历史起跑。

ALTER TABLE kanban_node_cursors ADD COLUMN resumed_from_pending_kind TEXT;

UPDATE kanban_node_cursors
SET resumed_from_pending_kind = 'unknown'
WHERE resumed_from_pending = 1;

-- SQLite 3.35+ 支持 DROP COLUMN（本仓的 sqlx 走 bundled libsqlite3）。
-- 这一列没有索引、没有 CHECK 引用，故可以直接删。
ALTER TABLE kanban_node_cursors DROP COLUMN resumed_from_pending;
