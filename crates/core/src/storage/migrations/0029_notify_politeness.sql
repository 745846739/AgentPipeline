-- 决策 284：「离线通知」的**礼貌两件**（节流 / 免打扰）上设置页——**显式修订 272⑥**。
--
-- 272⑥ 把这两件钉在 `config.toml`，理由是「前端已有同语义的一份表，再开一个口就是
-- 两处能改同一个语义」。本节把「那一份表」的适用范围写窄：前端那份只管**浏览器 toast**，
-- 出机器那条线（webhook / 飞书 / iMessage）的礼貌改由这一页说了算——一个语义一处可改。
--
-- ## 与通道单元同表、**不同单元**
--
-- 三列与通道四件同住一行，但两级关系**各自成立**：`channel` 那组管「送到哪」，
-- 这组管「什么时候准吵」，两边可以一个来自界面、一个来自 `config.toml`——不混的是
-- **组内**（通道四件不许混级别，礼貌两件同样不许）。
--
-- 三列**同生同死**（写入路径只收完整的礼貌单元）：全 NULL = 没保存过 → 读
-- `config.toml` 的 `[notify].cooldown_sec` / `[notify].quiet_hours`（268 的零配置姿态）。
-- 单元的存在性只认 `cooldown_sec`——半行只可能来自手改库，读取侧按「没保存过」处理
-- （见 `storage/notify_channel.rs` 的口径），不让一整页设置因为一行脏数据崩掉。
--
-- 单位都是整数：`cooldown_sec` 是秒（0 = 不节流，仍受免打扰），`quiet_start` /
-- `quiet_end` 是**本地整点**（0–23，含头不含尾、跨零点合法、起止相同 = 全天不静默）。
-- 取值范围在设置端点上校验（报错不静默，284⑤）——这一层不靠 CHECK 拦，与 0027 同姿态。
ALTER TABLE kanban_notify_channel ADD COLUMN cooldown_sec INTEGER;
ALTER TABLE kanban_notify_channel ADD COLUMN quiet_start INTEGER;
ALTER TABLE kanban_notify_channel ADD COLUMN quiet_end INTEGER;
