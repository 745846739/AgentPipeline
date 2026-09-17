-- 决策 205：`resume_continuation` 整层退场——续不续接由**原因表**决定（`types.rs::resume_continues`）。
--
-- 这一列（迁移 0006 加的阶段级开关）与节点级覆盖 `node_overrides_json[node].resume_continuation`
-- 回答的是「谁来决定开不开」这个问题。裁决是：不该有这个问题——它是个产品判断，
-- 不是配置项（用户的诉求原话：「不应该展示在设置里面，而是在代码中定义好」）。
--
-- 注意这与「0009 的死表留着」是两回事：那条说的是**表**删不得（历史迁移的 checksum 与
-- 数据都在），这里删的是**列**——读它的代码一并删掉，没有任何东西再引用它。
-- `node_overrides_json` 的**其他**键（skills、超时）不受影响：那是个通用覆盖表，只摘一个键，
-- 而且摘的是**读法**（`config.rs` 的两个函数），不是表里的历史数据。

ALTER TABLE stage_configs DROP COLUMN resume_continuation;
