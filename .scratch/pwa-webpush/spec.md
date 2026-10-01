# PWA 浏览器推送通知（pwa-webpush）

**Status:** done（票 01–04 全部收口；HTTPS 入口最终走决策 335 的应用内 TLS + 决策 336 全站配对闸门，原 Caddy 方案随决策 335 撤除）

> **来源**：2026-09-29 用户经 grilling 十余问拷问定形——「新增 PWA 的 APNs 通知」。
> 两轮勘察钉住事实（PWA 无 service worker；106 裸 IP + http 无证书；通知出口单漏斗
> `note_attention` + 值班长三处直调；礼貌门在 `dispatch()` 按类记槽），裁决全部来自
> 拷问轮次的用户拍板。
>
> **与既有决策的关系**（落地时按本仓惯例在 `docs/decisions.md` 标新编号并引用）：
> **① 突破决策 65**（「离线通知 v1 只做 SSE；Webhook / 邮件 / 飞书延后 v2」）——
> 这是第四次通道扩面（268 webhook → 270 飞书 → 272 iMessage → 本票 Web Push）。
> **② 沿用决策 272 的互斥格局**：272 明确把「多出口并存」踢给另立票，本票不碰，
> Web Push 是第四个互斥通道。**③ 礼貌语义照决策 284**（出口线的 cooldown / quiet_hours
> 设置页两级解析），不修订 130③。**④ 与决策 167（v1 无鉴权）的关系**：订阅端点挂
> 既有配对令牌守卫是**定点加强**，回环豁免等形状照决策 182⑦，不推翻 167 的其余现状。

## Problem Statement

我人不在电脑前（手机锁屏、离开桌面）时，流水线推进到需要我知道的节点——任务待拍板、
闸门失败、值班长回话完成——只有浏览器里的站内 toast，我全错过，回来才发现流水线停了半
天。现有离线通道各不理想：iMessage 通道要 Mac 开着 BlueBubbles，飞书/webhook 要第三方
服务。我想要 iPhone 锁屏上直接收到像 iMessage 一样的推送通知，点开直达现场。

## Solution

设置页新增**第四个互斥通知通道「浏览器推送」**（与 webhook / 飞书 / iMessage 三选一共
存）：选中后，我在这台设备的浏览器（iOS 上是添加到主屏的 PWA）上点一颗「订阅此设备」
按钮，此后**触发事件集与礼貌规则与 iMessage 通道完全一致**的每一次出站通知，都会以标准
Web Push 推到设备通知中心（iOS 16.4+ 主屏 PWA 走 APNs，Android / 桌面浏览器同一协议顺带
覆盖）。点通知直接落到现场：卡片级事件进那张卡、流水线级进那次运行的看板、值班长回话进
对讲台那条会话。设置页可列出已订阅设备、单个撤销、全部清空。地基：106 上用 mkcert 长效
IP 证书 + Caddy 反代 443，明文旁路关死；本机 localhost 开发不受影响。

## User Stories

1. As a 人不在电脑前的用户, I want 任务待拍板（pending）时手机收到推送, so that 我能及时回来恢复流水线而不是等回家才发现它停了。
2. As a 用户, I want 闸门失败 / 运行失败（failed 类）时手机收到推送, so that 故障发生在我洗澡睡觉前也能被立刻知道——这一类在免打扰时段也照发。
3. As a 用户, I want 任务完成（done）时收到推送, so that 我能第一时间知道可以验收合入。
4. As a 用户, I want 任务被取消（cancelled）时收到推送, so that 我不用白等一个不会来的结果。
5. As a 用户, I want 任务停滞（task_stale）、重试耗尽、上下文超载等"等人处理"的事件都收到推送, so that 流水线不会无声地卡在原地。
6. As a 用户, I want 值班长一轮回话完成（达到工具调用门槛的那类）时收到推送, so that 我能看到它替我干了什么而不用主动去翻对讲台。
7. As a 用户, I want 值班长回话失败 / 中断时收到推送, so that 值守轮悄悄死掉这种事瞒不过我。
8. As a 用户, I want 慢跑（slow_run）这类只记账不出站的事件**不**收到推送, so that 通知面保持"叫我的才是通知"的口径，与 iMessage 通道一字不差。
9. As a 用户, I want 同类事件在节流窗口（默认 300 秒）内只推一条, so that 一次重试风暴不会把我的通知中心轰成雪花屏。
10. As a 用户, I want 免打扰时段（默认 22–8）里除 failed 外的类别静音, so that 半夜不会被一条 done 唤醒，而真故障仍然叫得醒我。
11. As a 用户, I want 在设置页把通道切到「浏览器推送」（第四个互斥 chip）, so that 我明确知道现在是谁在出站，配置不产生"两个通道谁发"的歧义。
12. As a 用户, I want 在设置页点一颗「订阅此设备」按钮就完成权限申请与订阅, so that 权限弹窗永远由我的点击触发——进页面自动弹只会被我永久拒绝。
13. As a iOS 用户, I want 在 Safari 普通标签页（非主屏）访问时看到「请先添加到主屏幕」的引导而不是一颗必然失败的按钮, so that 我知道 iOS 的推送只在主屏 PWA 里可用。
14. As a 权限被我拒过的用户, I want 界面明说"权限已被拒绝"并给出去系统设置恢复的指引, so that 不会对着一颗点了没反应的钮反复试。
15. As a 用户, I want 点开通知直达事件挂载的对象——那张卡 / 那次运行的看板 / 对讲台那条会话, so that 推送的价值止于"到达现场"，不用在看板里人肉搜索。
16. As a 用户, I want 通知正文带上对象名（task_id、事件类别、回话首句这一级）, so that 锁屏预览就足以判断要不要现在点进去。
17. As a 用户, I want 深链不可达时（对象已归档等）降级到看板首页而不是白屏, so that 点开永远有一个能用的落点。
18. As a 用户, I want 设置页列出所有已订阅设备（订阅时间 / UA）, so that 我能看见"幽灵订阅"并解释"为什么那台设备收不到"。
19. As a 用户, I want 单个撤销与一键清空订阅, so that 换手机 / 怀疑被订阅过时我能收回推送面。
20. As a 用户, I want 订阅 / 退订端点受配对令牌守卫（回环豁免）, so that 同网段的别人不能把我的任务动态持续订阅走——读接口是一次性的偷看，订阅是永久的外泄管道。
21. As a 用户, I want 过期或被服务端判死（410）的订阅自动从库里清掉, so that 设备清单里永远是活的订阅。
22. As a 用户, I want VAPID 密钥对首次启用自动生成入库、读接口只回显掩码, so that 零手工配置的同时秘密不落进日志和接口回显。
23. As a 用户, I want 我的 iPhone 和桌面浏览器**各自**订阅、同一条事件全员都推, so that 我在哪个设备上都能接住。
24. As a 用户, I want 106 的所有访问都走 HTTPS、明文 3333 入口关死, so that 订阅动作永远不发生在明文通道上，CA 那套不白装。
25. As a 运维者, I want Caddy 用静态 mkcert 证书反代、后端只绑回环, so that 部署只多一个 systemd 服务和十行配置，续期是三五年后才想起来一次的手动动作。
26. As a 运维者, I want 根 CA 私钥只留在我电脑上、签发与上机命令写进运维文档, so that 公网服务器被拿下也偷不走信任链的根。
27. As a 本机开发者, I want localhost 下一切照旧——无需 CA、无需 Caddy、订阅测试照常跑, so that 日常开发工作流零变化。
28. As a Android / 桌面用户, I want 同一套标准 Web Push 在 Chrome / Edge 里也能用, so that 这不是 iOS 专属功能，我桌面浏览器也能收到。
29. As a 用户, I want Web Push 的触发事件将来随 iMessage 通道一起增减, so that 我不用维护两套"什么该推"的规则——它们本来就是同一套。
30. As a 用户, I want 一条通知都没订阅、通道也没选时系统一切照旧, so that 本功能是纯增量，不开就感知不到它的存在。

## Implementation Decisions

- **触发面零新增**：Web Push 挂进既有唯一漏斗——`note_attention` 落库成功且 `kind.wakes()`
  且该类挂了通知出口才发，外加值班长三处直调（回话完成的工具调用门槛、播报轮恒发、失败
  收口受记账门）。`kind → NotifyClass` 映射表复用，`SlowRun` 照旧一个字节不出站。
  Web Push 将来与 iMessage 同批增减事件。
- **礼貌门零新增**：走既有 `dispatch()` 同一道门——按类 cooldown、quiet_hours、
  `failed` 恒发、`pending` 豁免免打扰，配置值沿用设置页「礼貌」区块的两级解析
  （界面单元 > config.toml），不为 Web Push 另立一份配置。
- **第四互斥通道**：通知通道表沿用单行三选一（整体覆盖、不允许混）的既有形状，种类枚举
  增一个值；选中即出站、不选即静默。与 webhook / 飞书 / iMessage 并存的「多出口」明确
  不做（决策 272 已把它踢给另立票）。
- **订阅存储**：SQLite 新表，每设备一行——`endpoint` 唯一键（按它 upsert）、订阅创建时间、
  UA、push 协议元数据（p256dh / auth）。VAPID 密钥对**首次启用自动生成**、与订阅同生命周期
  存库，读接口掩码回显（对齐 `providers.api_key` 先例）。VAPID 联系方式用占位 mailto，
  不暴露真实邮箱。发送侧对每条活订阅各发一次 HTTP，410 / 失败即删行，best-effort 不重试
  （与既有投递姿态一致）。
- **报文**：复用 `Notice{title, body}` 与其归因白名单（detail 原文不出网的既有约束不变），
  push payload = `{title, body, url}`——**`url` 是 push 独有的新增字段**，深链由服务端拼好
  放进 payload，service worker 只消费；webhook / 飞书 / iMessage 三种既有报文一字不改。
- **深链规则**：卡片级事件（有 task_id）→ 任务详情路由；流水线级 → 该任务看板视图；
  值班长回话 / 失败类（无 task_id）→ 对讲台对应班次会话；不可达降级看板首页。前端是自写
  hash 路由，query 深链（`?task=` / `?project=`）有既有先例。
- **Service worker 最小化**：只做 `push`（收 payload → `showNotification`）与
  `notificationclick`（读 `url` → 开窗 / 聚焦并导航）两件事，**不做任何离线缓存**；作为
  同源静态资产随构建产物分发（安全上下文要求 HTTPS / localhost）。
- **订阅端点与守卫**：`POST` 订阅（upsert）、退订、列出订阅三个端点，挂既有配对令牌
  守卫——局域网来源无令牌 403、回环豁免、报文提示去配对（形状照决策 182⑦）。这是对
  决策 167「v1 无鉴权」的定点加强，理由：读接口是匿名的一次性偷看，订阅是具名的持续性
  外泄通道且产生出网成本。
- **前端（设置页）**：通道区第四个 chip「浏览器推送」+「订阅此设备」按钮（**点击手势内**
  完成 `requestPermission → subscribe → 上报服务端`，不做进页自动弹窗）+ 权限四态显示
  （未申请 / 已授权 / 已拒绝给系统设置指引 / iOS 非主屏给「添加到主屏幕」引导）+
  「已订阅设备」清单区块（时间 / UA / 单个撤销 / 全部清空）。
- **HTTPS 地基（mkcert 路线，用户拍板）**：根 CA 在开发机用 mkcert 生成、**私钥永不上传
  106**；签一张含 `106.12.12.6` IP SAN 的 3–5 年长效证书，证书 / 私钥拷到 106 由 Caddy
  静态加载；Caddy 听 443 反代后端，后端改绑 `127.0.0.1`，**明文 `http://IP:3333` 入口
  拆除**（不留旁路）。到期手动重签，签发命令写进 `docs/operations.md`。已接受的代价：
  每台要用推送的设备装 CA 描述文件；Apple 收紧用户自装 CA 信任的政策变数。
- **本机开发不受影响**：localhost 本身是安全上下文，service worker 与订阅照常工作，
  不需要 CA 与 Caddy。
- **部署联动**：106 部署脚本 / systemd / 部署 skill 需要跟进（装 Caddy、证书上机、后端
  绑定改回环、配对 URL 变 https）——属于落地步骤而非本 spec 的功能面。

## Testing Decisions

**好测试的判据（本仓口径）**：只断言外部可观察行为——「真出站了一条 HTTP」「礼貌门挡没
挡」「报文形状与字段」「端点状态码与落库行」；不断言内部调用路径。**本 effort 零新增
可测试性接缝**，全部落既有缝（与 UX 审计 effort 同姿态）：

- **L2 集成（通知出站）**——先例 `crates/core/tests/integration/notify.rs`：
  `Store::set_notifier` 注入 `WebhookNotifier`（新 `NotifyTarget` 变体），
  **订阅行的 `endpoint` URL 即远端推送服务替换点**（测试把订阅行指到 `TinyHttp`，
  与 webhook URL 指 `TinyHttp` 同一姿势——决策 250「URL 是缝不是 trait」的复用，
  不进 `docs/testing.md` §3.1 权威表新行）。断言：wakes 的 attention 落库 → 假推送服务
  收到 N 条（N=活订阅数）、payload 含 `title/body/url`、cooldown 边界（301 秒放行）、
  免打扰时段 done 静音而 failed / pending 照发、`SlowRun` 零出站、410 响应删行。
  时钟用 `ManualClock`，事件用既有 scheduler 手动 tick / 直接调漏斗。
- **L3 API 契约（tower oneshot）**——先例 `crates/app/tests/integration/api_contract.rs`
  与配对端点断言矩阵：订阅 / 退订 / 列表三端点的令牌守卫（局域网无令牌 403、带令牌放行、
  回环豁免）、upsert 幂等（同 endpoint 两次订阅一行）、VAPID 公钥可读而私钥掩码、
  清空后列表为空、通道第四个种类值的保存与回显。
- **L1 单元（纯函数）**——先例 `frontend/src/lib/*.test.ts`：service worker 的
  `push` 处理（payload → 通知 title/body）与 `notificationclick`（payload → 导航目标）
  抽纯函数测；iOS 非主屏检测判据；深链 URL 拼装与既有 `parseRoute` 测试族；
  设置页权限四态状态机。
- **前端 e2e（playwright 既有 harness）**——`context.grantPermissions(['notifications'])`
  驱动「订阅按钮 → 订阅行真的出现在服务端清单 → 撤销后行消失」端到端；权限被拒态与
  非主屏引导文案断言。先例：`settings-notify` 相关既有 spec 与 `pairing` 用例形状。
- **不进自动门（显式声明，照「真 GitHub 冒烟 `#[ignore]`」「截图是证据不是门」的姿态）**：
  **真 APNs / FCM 投递不进任何自动门**。手动验收清单一条：真 iPhone 添加到主屏 → 订阅 →
  触发一个 pending → 通知落在锁屏 → 点开直达那张卡。本地自动门覆盖到「HTTP 出站到假
  推送服务」为止。
- 落地时按本仓惯例在 `docs/testing.md` §5–§7 用例目录表补行（新用例挂既有目录文件，
  不新建测试文件族）。

## Out of Scope

- **多出口并存**（Web Push 与 iMessage 同时出站）——决策 272 已明确另立票，本票维持互斥。
- **按事件类型独立开关**（每类一颗钮）——272 票面显式不做，`notifyOn` 死开关另立票。
- **PWA 离线壳 / 离线缓存**——service worker 只做推送，缓存是另一个功能的活。
- **让浏览器 toast 也读设置页礼貌值**——决策 284 明确不做，两条线各管各的现状不变。
- **真 APNs 投递进自动门**——只做手动验收清单。
- **真域名 / Let's Encrypt IP 短期证书 / Tailscale 三条 HTTPS 路线**——用户拍板 mkcert，
  其余记录在案不实施。
- **通知声音、富媒体（图片 / 按钮）、本地定时通知**——保持最小报文形状。
- **国内 Android 厂商通道（APNs 替代品 / 厂商推送 SDK）**——标准 Web Push 覆盖不到的
  厂商浏览器生态，另议。
- **webhook / 飞书 / iMessage 三种既有报文的任何改动**。
- **桌面壳（局域网绑定）的 TLS**——本票只管 106 与 localhost。

## Further Notes

- **手动验收清单（真机，唯一验 APNs 的路）**：① 开发机签证书、上 106、Caddy 起 443；
  ② iPhone 装 CA 描述文件并信任；③ Safari 开 `https://106.12.12.6` → 添加到主屏；
  ④ 设置页切「浏览器推送」→ 订阅此设备 → 清单里出现一行；⑤ 造一个 pending 事件 →
  锁屏收到 → 点开直达卡片。任一步断在哪个环节，故障域就缩到哪一段（CA / SW / 订阅 /
  投递）。
- **Apple 政策风险已知且接受**：用户自装 CA 在 Safari 的信任姿态是变数；若未来失效，
  退路是补一个真域名走标准证书（拷问轮已论证过该备选）。
- **票面拆分建议（下一步 `/to-tickets`）**：① HTTPS / Caddy 地基（无阻塞边，可先走）；
  ② 后端通道（`NotifyTarget` 变体 + 订阅表 + VAPID + 三端点守卫，阻塞于 ① 的 HTTPS
  仅在真机联调时才需要，纯后端测试不阻塞）；③ service worker + 前端订阅 UI（阻塞于 ②
  的端点与 VAPID 公钥）；④ 设置页设备清单 + 手动验收（阻塞于 ③）。实现若发现某两块确
  能独立推进，允许合并——与 iMessage 票「互相咬合就一张票」同一判断标准。
- **部署 skill（agentpipeline-deploy-106）需要同步更新**：装 Caddy、证书上机、绑定变更
  后的验证命令（`curl https://…`）都要进 skill 文档，否则下一次部署会把明文入口装回来。
