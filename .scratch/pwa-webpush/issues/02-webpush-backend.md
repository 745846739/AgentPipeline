# 02: 浏览器推送后端纵切（订阅 + VAPID + 出站）

**What to build:** 通知通道能选中第四个互斥种类「浏览器推送」；服务端接收设备订阅（每设备一行、按 endpoint upsert）、首次启用自动生成 VAPID 密钥；既有通知漏斗（attention 落表 + 值班长三处直调）触发时，走同一道礼貌门，把带深链的 payload 扇出到所有活订阅。全程不需要真浏览器即可验证：curl 造一条指向假推送服务的订阅 → 触发一个 pending → 假服务收到报文，cooldown 内第二条被挡。

**Blocked by:** None (can start immediately)

**Status:** done（已实现，决策 323 / 324；§5 两个新模块 17 条、§6 `notify.rs` 7 条新用例、§7 `api_contract.rs` 6 条全绿）

- [x] 通知通道的种类枚举新增「浏览器推送」，保存/回显/交还配置文件的两级语义与既有三通道一致，互斥格局不变 —— `NotifyFormat::WebPush`（serde 形 `"webpush"`）；两级解析那两处 `resolve_*` 给它 `Some(NotifyTarget::WebPush)`（**没有必填件**）
- [x] 订阅端点按 endpoint upsert（同 endpoint 两次订阅落一行），订阅行含创建时间与 UA；退订删行；列表可读 —— 迁移 `0037_push_subscription.sql` + `storage/push.rs`（`created_at` 只在首插时写；`endpoint_hint` 只回摘要）
- [x] 订阅/退订/列表三端点过配对令牌守卫：局域网无令牌 403（报文提示去配对）、带令牌放行、回环豁免 —— `stream::PUSH_SUBSCRIPTIONS_PREFIX` 那一族**连 GET 也要令牌**（对决策 167 的定点加强，回环豁免与报文照 182⑦）
- [x] VAPID 密钥对首次启用自动生成并入库；公钥接口可读供前端订阅用，私钥读接口回显掩码（对齐 providers 掩码先例） —— `ensure_push_vapid_keys`（幂等，`COALESCE` 防并发覆盖）+ 设置读数里公钥给真值、私钥给 `***`
- [x] VAPID 联系方式为占位 mailto，不暴露真实邮箱 —— `webpush.rs::vapid_subject()` 的占位常量
- [x] 出站复用既有 `dispatch` 礼貌门：按类 cooldown、免打扰时段、failed 恒发、pending 豁免，SlowRun 零出站——判据与 iMessage 通道一字不差 —— 扇出挂在 `dispatch` 之后按 `Delivery` 分流；金丝雀用例 `slow_run_never_pushes`
- [x] 每条活订阅各收到一条 HTTP，payload = title / body / url（深链由服务端按规则拼好）；best-effort 不重试 —— 加密面 = 自实现的 RFC 8291（`aes128gcm`）+ RFC 8292 VAPID，**KAT 用 RFC 的真值**钉住（`webpush.rs`）
- [x] **删行口径（显式修订票面那一句）**：只对 **404 / 410**（订阅已不存在）删行，临时失败（5xx / 超时 / 网络抖动）**留行**——一次抖动清空设备清单是用户看得见的伤害，而清单是唯一能看见「哪台设备订着」的地方（决策 324；用例 `a_gone_subscription_is_deleted_while_a_transient_failure_is_kept`）
- [x] webhook / 飞书 / iMessage 三种既有报文一字不改（url 字段是 push payload 独有） —— `render()` 只多一支 `WebPush`，三种既有格式的载荷逐字未动
- [x] L2 集成测试（TinyHttp + ManualClock + Store::set_notifier 既有先例）与 L3 契约测试全绿；docs/testing.md 用例目录表补行 —— §6 7 条（两台假设备扇出 + 头断言 + **测试自带的解密路径**还原 payload）/ §7 6 条（含守卫矩阵）/ §5 `webpush.rs` 10 条 + `storage/push.rs` 7 条；testing.md §5/§6/§7/§10 行已补
