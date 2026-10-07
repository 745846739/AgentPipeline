-- 重活外发的白名单模式（决策 398）。
--
-- kanban_offload 加两列：白名单模式的开关与正则。模式开着时，agent 走 run_command
-- 的命令若命中正则（且过防注入结构检查），自动改道外发链路；offload_run 工具
-- （skill 模式）原样保留——两层路由并存，不是二选一。
--
-- whitelist_enabled 缺省 0（行不存在 / 老库升级都不改行为，逐字等于本模式出现之前）；
-- whitelist_pattern NULL = 从没填过正则。
ALTER TABLE kanban_offload ADD COLUMN whitelist_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE kanban_offload ADD COLUMN whitelist_pattern TEXT;
