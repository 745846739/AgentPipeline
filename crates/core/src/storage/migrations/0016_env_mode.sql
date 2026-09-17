-- 决策 206：环境层的权限档位（auto / ask / deny）——一层列，两层解析。
--
-- **为什么存在**：决策 188 把值班长的能力扩到能碰文件与命令，而这类动作没有
-- `allowed_actions` 那样的后端下发动作集可依（它的输入是人可以随便打的任意文本）。
-- 决策 206 给出的规则是「按失败代价分档」：环境层的错误落在文件与机器里（可回滚、有日志），
-- 状态机写动作的错误会改变流水线的事实（有依赖边、有 worktree 准入、有合入门）——
-- **前者可配，后者不可配**。这一列就是「前者可配」的落点。
--
-- 两层，**不做节点级覆盖**（决策 206）：
--   1. 全局默认在 `config.toml` 的 `[pipeline] env_mode`（不进 UI，照
--      `allow_dirty_worktree_merge` 那一批今天的做法）；
--   2. 阶段级覆盖 = 本列。NULL = 没配过 → 用全局默认（真实阶段）或该阶段的缺省
--      （值班长缺省 `ask`，见 `EnvMode::default_for`）。
--
-- **缺省等于现状**：全局默认 `auto`、真实阶段一律 `auto` → 什么都不配时，流水线各阶段的
-- 行为与今天逐字相同。这是「落地不改变任何已运行行为」的兑现。
--
-- 取值域由 CHECK 兜住（SQLite 允许在 ADD COLUMN 上带 CHECK）：写入路径（`PUT /stage-configs`）
-- 已经按枚举拒过非法值，这条是**手工改库**那一侧的兜底。读侧对认不出的值退回缺省，
-- 而缺省的方向是安全的：值班长退回 `ask`（收紧），真实阶段退回 `auto`（= 今天）。
ALTER TABLE stage_configs
    ADD COLUMN env_mode TEXT
    CHECK (env_mode IS NULL OR env_mode IN ('auto', 'ask', 'deny'));
