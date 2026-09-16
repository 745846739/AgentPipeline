-- 界面上的技能市场来源（决策 187）：把「允许从哪些 registry 装技能」这个选择持久化。
--
-- 与 `kanban_server_bind`（迁移 0008）同构，理由也相同：**「有且只有一条记录」是不变式**
-- ——启动解析、界面保存、清除动作三处都假设它唯一，只靠「代码里记得别插第二行」维持的话，
-- 任何一次新增写入路径都可能悄悄破坏它（多出的一行不会被任何读取读到，表现为「改了没生效」
-- 这种极难定位的现象）。`CHECK (id = 1)` 让数据库自己拒绝第二行。
--
-- **为什么不写回 `config.toml` 的 `[market] allowed_sources`**：那个文件是手写配置
-- （注释即文档），写回必然要整体重排 TOML，用户的注释会静默消失；且本仓的边界由决策
-- 22 / 56 定下——**界面能改的配置不进 config.toml**。故界面改的那份住在这里，
-- `[market]` 仍是「声明式默认」：优先级「本表 > config.toml」，清掉本表即回到配置文件。
--
-- 存 JSON 数组原文而不是逗号分隔串：来源是 origin（可能带端口、可能是 IPv6 字面量
-- `[::1]:8787`），自己定分隔符迟早撞上一个含分隔符的合法值。JSON 也不引新依赖
-- （serde_json 已在依赖里）。

CREATE TABLE IF NOT EXISTS kanban_market_sources (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    sources_json TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
