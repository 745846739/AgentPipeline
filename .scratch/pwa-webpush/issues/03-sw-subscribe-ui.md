# 03: service worker + 订阅按钮

**What to build:** 用户在设置页把通道切到「浏览器推送」（第四个 chip），点「订阅此设备」——点击手势内完成权限申请、订阅并上报服务端，设备清单当场多出一行。service worker 收到推送出一条通知，点通知按 payload 里的深链开窗导航，深链不可达降级到看板首页。权限四态各有明确文案：未申请、已授权、已拒绝（给系统设置指引）、iOS 非主屏（给「添加到主屏幕」引导）。localhost 上全流程可演示。

**Blocked by:** 02 (浏览器推送后端纵切)

**Status:** done（已实现，决策 323 / 325 / 326；前端单测 + 接线 20 条、e2e `push-subscribe.spec.ts` 4 例全绿）

- [x] 最小 service worker：只做 `push`（payload → 显示通知）与 `notificationclick`（payload url → 开窗/聚焦导航），零离线缓存；作为同源静态资产随构建产物分发并注册 —— `frontend/src/sw.ts`（Vite 多入口 → `dist/sw.js`，固定地址不带 hash；`register('/sw.js')` **不带** `{type:'module'}`——iOS Safari 不支持 module worker，故产物必须是经典脚本，由 e2e 的 `serviceWorker.ready` 兑现）；**不装 `fetch` 处理器**（零拦截面）
- [x] 两个处理器抽成纯函数，vitest 覆盖 payload → 通知形状、payload → 导航目标两族 —— `lib/pushPayload.ts`（`notificationFrom` / `notificationTarget`，6 条）
- [x] 设置页通道区出现第四个 chip「浏览器推送」，选中保存后与后端通道状态一致（前后端类型/校验/文案同步） —— `notifyChannel.ts` 的 `CHANNEL_LABELS.webpush = '浏览器推送'`、`validateNotifyDraft` 对该通道无必填件；保存那一次服务端生成 VAPID 密钥对
- [x] 「订阅此设备」按钮：点击手势内完成 requestPermission → subscribe → 上报服务端；任何情况下不进页自动弹权限 —— 整条链在 `subscribeThisDevice()` 一次点击里（**进页面零自动弹窗**，那是会被系统永久拒绝的形状）
- [x] 权限四态显示：未申请（可点）/ 已授权已订阅（显示已订阅 + 可退订）/ 已拒绝（红色说明 + 去系统设置的指引）/ iOS Safari 非主屏（按钮位替换为「请先添加到主屏幕」引导） —— 判据是纯函数 `pushFace`（`pushSubscribe.ts` 10 条），接线在 `SettingsNotify.svelte`
- [x] playwright 端到端：点按钮 → 服务端订阅清单真多一行；拒绝态与非主屏引导文案断言 —— `e2e/push-subscribe.spec.ts` 4 例（另**独立读一次 `GET /notify/push/subscriptions` 取证**，并钉住完整 endpoint 一处都不出现）；`grantPermissions(['notifications'])` 在 headless 下**不够**（实测 `permissions.query` 报 granted 而 `Notification.permission` 报 denied，平台那一层没有通知服务），故权限与推送服务两处在 `addInitScript` 里注入——注入的是**浏览器那一侧的环境**，页面代码一行不改
- [x] 深链消费与降级在真应用上走通（hash 路由 query 深链既有先例），点击通知落到对应路由 —— `#/task/<id>?run=<n>` 由 `TaskDetail.svelte` 消费（切会话页签 + 选中那一轮 + **消费一次就抹掉参数**，决策 326）；`#/talk?session=<id>` 走既有班次参数；payload 层拼不出可用地址时降级 `#/`（`notificationTarget`）
  - **两处与票面措辞的差异如实记**（决策 326）：① 流水线级没有「该任务看板视图」这个落点（本应用看板是 `#/` 全量、没有按任务收窄的路由），故它也落在**那张卡**上、多带 `?run=`；② 「对象已归档 / 任务不存在」的落点是任务详情页的空态（状态 + 下一步 + 顶栏看板入口），**不做静默重定向**——降级只处理 payload 里拼不出可用地址的情形
