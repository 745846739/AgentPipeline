-- 浏览器推送（PWA Web Push，spec `.scratch/pwa-webpush/` 票 02）：订阅行 + VAPID 密钥对。
--
-- ## 为什么是两张东西（一列两行 + 一张新表）
--
-- 「订阅」是**每台设备一行**的具名事实：同一个浏览器每次 `pushManager.subscribe()`
-- 都产出一个**能力 URL**（endpoint，拿到它就能往那台设备推）+ 一对密钥（p256dh / auth）。
-- 它是唯一键——同一设备重复订阅只会拿到同一条 endpoint，故 upsert 按它落一行
-- （`endpoint UNIQUE`：去重由数据库强制，不靠代码约定）。
--
-- VAPID 密钥对则是**整台机器一对**（RFC 8292：服务端身份，与订阅同生命周期）——它
-- 不是每设备一份，故住 `kanban_notify_channel` 那行单行表（`CHECK (id = 1)`，0027 的
-- 不变式原样沿用），与 `channel = 'webpush'` 同生。**首次启用自动生成**：没有它就没法
-- 订阅（浏览器要 `applicationServerKey`），而手工配置一对 P-256 密钥不是用户该做的事。
--
-- ## 秘密面
--
-- `p256dh` / `auth` 是**秘密**吗？——`auth` 是（拿到它 + endpoint 就能伪造推给那台设备的
-- 报文）；`endpoint` 与 `p256dh` 一起才构成能力，单独一条不足以推送。三者一律只入本表
-- （DB 目录 0700 / 文件 0600，§12.14；`data/` 整目录对 agent 工具关闭，决策 206），
-- 读接口只回**摘要**（前缀 + 尾 6 位）而不是原文——对齐 providers `api_key` 的掩码先例
-- （决策 112），读接口是「看得见的清单」，不是「可以顺手拿走的能力包」。
--
-- `vapid_private_key` 同理：读接口只回 `***` 掩码（`webhook_url` / `bluebubbles_password`
-- 同一档）；公钥不是秘密（浏览器订阅时本来就要拿到它），原样回显。
--
-- ## 410 之后的行
--
-- 推送服务对已失效订阅回 404 / 410（RFC 8030 §5.4），发送侧据此**删行**——设备清单里
-- 因此只剩活订阅。删行是本表的唯一清理路径（不做保留期：一条订阅就是一台设备，
-- 表就那么几行）。

ALTER TABLE kanban_notify_channel ADD COLUMN vapid_public_key TEXT;
ALTER TABLE kanban_notify_channel ADD COLUMN vapid_private_key TEXT;

CREATE TABLE IF NOT EXISTS kanban_push_subscription (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 推送服务给这台设备的地址（能力 URL）。唯一键：按它 upsert。
    endpoint   TEXT NOT NULL UNIQUE,
    -- 浏览器的公钥（P-256 未压缩点，base64url）与鉴权秘密（16 字节，base64url）。
    p256dh     TEXT NOT NULL,
    auth       TEXT NOT NULL,
    -- 订阅那一刻的 User-Agent：设置页要能认出「这是哪台设备」。
    user_agent TEXT,
    created_at TEXT NOT NULL
);

-- 清单按「订阅时间升序」读（设置页的排序就是它）。
CREATE INDEX IF NOT EXISTS idx_push_subscription_created
    ON kanban_push_subscription(created_at, id);
