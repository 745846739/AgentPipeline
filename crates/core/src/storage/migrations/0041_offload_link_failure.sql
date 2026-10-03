-- 外发链路失败读数（票 runner-offload/08）。
--
-- kanban_offload 加一列：最近一次**链路**失败（推分支 / dispatch / 轮询超时 /
-- 拉日志失败）的时间戳。NULL = 从没失败过（界面读数是「无」，不是「0」）。
-- 远端命令本身跑红（conclusion=failure）**不**写这列——那是外发在工作，
-- 不是链路坏（决策 381 的语义边界）；外发成功一轮即清回 NULL。
ALTER TABLE kanban_offload ADD COLUMN last_failure_at TEXT;
