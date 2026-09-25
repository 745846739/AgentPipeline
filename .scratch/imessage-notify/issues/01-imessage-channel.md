# 01: iMessage 通道 + 回话第二触发面 + 设置总开关

**What to build:**（**裁决已落决策 272，本票不重开裁决，只落地**）

三条需求：① iMessage 作为离线通知通道（经 **BlueBubbles** 本地 HTTP 服务）；
② 触发面两个——任务转 pending（**已有，不动**）与**值班长回话完成**（新）；
③ 设置界面能开启/关闭。

**接缝事实（探查取证，2026-09-25；全部由本人核实，非转述）**：

**通知链路（现状）**
- 唯一触发点：`crates/core/src/storage/attention.rs:204-211` —— `note_attention` 落库成功
  且 `kind.wakes()` 才发（这一处不动）。
- 唯一出口：`crates/core/src/notify.rs:193-279` `WebhookNotifier` —— `reqwest` POST 到单个
  `url`，`WEBHOOK_TIMEOUT = 10s`（`notify.rs:127`），`redirect::Policy::none()`（URL 含秘密，
  302 会把它带走），**best-effort 不重试**，失败只 warn 且走 `e.without_url()`。
- 分流点：`notify.rs:164` `payload_for(format, kind, task_id, occurred_at, detail)` ——
  决策 270 明确这是**报文形状**的唯一分流点，政策语义与格式无关。故 iMessage 不是「再加
  一个 format」那么简单：投递方式本来就只有 HTTP 一种，BlueBubbles 恰好仍走 HTTP，
  这才使它成为四个方案里仓内改动最小的那个。
- cooldown 状态：`notify.rs:202` `last_sent: Mutex<HashMap<NotifyClass, DateTime<Utc>>>`
  —— **按键只有类，没有 task_id**（所以「task_id 键要豁免」在本实现里不是问题；
  真正的坑是**复用类**）。
- kind → class：`notify.rs:77-88` `notification_class`；`NotifyClass` 四个成员
  （`Pending` / `Done` / `Failed` / `Cancelled`）。
- 礼貌门：`notify.rs:109-123` `should_notify`；`is_quiet_hours` 按**服务器本地整点**。

**配置与接线**
- `crates/core/src/config.rs:512-538` `NotifyConfig{webhook_url, cooldown_sec, quiet_hours, format}`
  + `Default`；`#[serde(default, deny_unknown_fields)]` ⇒ **撤掉任何字段都会让既有
  config.toml 解析失败、服务起不来**（fail fast，决策 47/103/134）。
- `crates/app/src/serve.rs:380-388`：**启动时建一次**——`webhook_url` 在场才 `set_notifier`。
- `crates/core/src/storage/mod.rs:56 / 163 / 171`：`RwLock<Option<Arc<WebhookNotifier>>>` +
  `set_notifier` / `notifier()`。**今天没有 `clear_notifier`**——关掉开关要新增一条。

**值班长回话完成（新触发面）**
- 不在 attention 漏斗里：`migrations/0019_foreman_attention.sql` 的 `task_id TEXT NOT NULL`
  且 `FOREIGN KEY (task_id) REFERENCES kanban_tasks(id)`；该表语义是「待办」、由值守轮消费
  `consumed_at`。回话是播报，写进去会污染 §2.1 的唤醒判据 ⇒ 走 `respond()` 收口后的新入口。
- 收口点 `crates/core/src/pipeline/foreman.rs`：`traces` 声明 1652、循环 `for _ in 0..round_limit`
  1667、模型调用 1693、**终止判据 `response.tool_calls.is_empty()` 1717-1723**、
  `traces.push(ForemanTrace{tool, args_summary, ok})` 1742-1746、循环收口 1749。
  ⇒ **`traces.len()`（这一轮的工具调用次数）在收口处现成可得，零管道**。
- 无耗时变量：`respond_inner` 内没有 `Instant`/`elapsed`；`say()` 的 `round_started_at`
  只用于失败时作废提议，不进 `respond_inner`。`ForemanTrace` 无时间戳。
- 轮数上限：`FOREMAN_MAX_ROUNDS = 300`（`foreman.rs:612`）；触顶**不再整轮作废**，
  带 `FOREMAN_PARTIAL_TURN_MARK` 落库（`foreman.rs:1756-1766`）⇒ 它 `traces.len()` 逼近 300，
  **自动落进通知面**，不必为它加规则。
- `watch()` 已自己判别静默：`foreman.rs:1838` 二次判 `FOREMAN_NO_ACTION_MARK`，静默走
  `record_watch_wake(…, Silent, …)` 并返回 `Ok(None)`；真播报才 `Ok(Some(turn))`（1854-1858）。
  `say()` 走 `TurnInput::Human`，**结构上不可能命中静默分支**。
- 「值守播报 vs 回话」已是后端一等契约：落库前拼 `FOREMAN_WATCH_MARK`，路由层解析成
  `proactive`（`crates/app/src/routes/foreman.rs:1146-1148`）；前端**被守卫测试禁止**自己
  重实现这个前缀判断（`frontend/src/lib/delegation-scan.test.ts:136-145`）。
- `say()` **没有**「一轮回话完成」的 SSE 事件（`crates/core/src/sse.rs:15-34` 的事件全集里没有
  turn 级事件）；前端靠 POST 回包或 3s 轮询 `GET /foreman/session` 知道完成。
  ⇒ 本次**不动 SSE**；将来要界面上也弹 toast，需先加 SSE 事件，**那是另一张票**。
- 失败收口：超时走 `record_failed_turn`（`foreman.rs:1458` / `2032`），**不是**
  `record_interrupted_turn`（`2113`，**只在 panic 时**由 `crates/app/src/routes/foreman.rs:956` 补账）。
  超时值来自决策 66 那条链——阶段 `foreman` 的 `max_duration_sec` > `node_max_duration_sec`
  （缺省 **1800s**）；`with_turn_timeout`（`1268-1270`）**只有测试在用**。

**BlueBubbles 上游契约（读服务端源码核过）**
- `POST /api/v1/message/text`，认证走 **query 参数** `password`（别名 `guid` / `token`）。
- body 字段（`packages/server/src/server/api/http/api/v1/validators/messageValidator.ts:69-79`，
  处理在 `routers/messageRouter.ts:237-241`）：
  `chatGuid` **required**；`tempGuid` 在 `apple-script` 下 **必填**；正文字段名是
  **`message`**（`present|string`）；`method` ∈ `apple-script` / `private-api`，缺省 `apple-script`。
  **官方文档页给的例子写的是 `text`——那是错的，以源码为准。**
- `tempGuid` 是发送队列的**去重键**：重复直接 `400 Message is already queued to be sent!`
  （`messageValidator.ts:113-116`）⇒ **必须每次唯一**，否则从第二条起全灭。
- `chatGuid` 形如 `iMessage;-;<地址>`（`packages/ui/src/app/components/modals/ScheduledMessageDialog.tsx`
  里就是这么拼的）。

**设置界面与秘密面**
- 设置落地页 `frontend/src/routes/SettingsLanding.svelte` 是**门牌不是表单**（各项独立路由，
  分类法是定稿文案，design §4.3）；今天**没有**通知设置页。
- `frontend/src/lib/notificationPolicy.ts:19-33` 的 `notifyOn` 是**死开关**——无 UI、无写入方、
  无持久化（全仓非测试代码里 `policy` 只有默认值那一处赋值）。后端明确不接它
  （`notify.rs:11` / `104`）。
- 界面开关范式：**单行 DB 表** + `config.toml` 打底 + provenance——
  `storage/server_bind.rs` + `migrations/0008`（决策 186）、`storage/market_repos.rs` + `0010`
  （决策 194/187）。
- 交互最近先例是 `/share` 那颗钮（**两颗按状态二选一的按钮**，不是开关）：
  `frontend/src/routes/Share.svelte:125-154`（成功重读 `ServerInfo` + `note ok`；失败 `note bad`；
  传输失败**不当失败**、以重读为准，`frontend/src/lib/lanToggle.ts:34-35` 8×250ms）；
  `crates/app/src/routes/server_info.rs` 的 `set_lan` / `clear_lan`，三种应答
  （`203` 成功 / `400` 确定失败 / `202 + pending` 超时断线）；
  provenance 中文标签映射在 `frontend/src/lib/sharePairing.ts:60-66`。
- 秘密面先例：provider 的 `api_key` **明文落 SQLite**（`migrations/0001_init.sql:132-142`；
  决策 112 是**权衡后**拒绝加密/keychain），读回**只给常量掩码 `***`**
  （`types.rs:1355-1359`、`catalog.rs:349-356`），提交时**掩码或留空即不改**
  （`crates/app/src/routes/providers.rs:117-122`、`frontend/src/lib/providers.ts:103-126`）。
  存放面 `{home}/data/agentpipeline.db`，目录 0700 / db 及 `-wal`/`-shm` 0600；决策 206 的
  「`data/` 前缀拒」**就是这个目录**（`file_policy.rs:55-89`，doc 称其为「密钥库」）。

**测试骨架**
- `crates/core/tests/integration/notify.rs` 用 `TinyHttp`（真起本地 HTTP 服务）+ `wait_hits`
  轮询数命中次数；`attach_format(server, format)` 一行就能加一支新格式。
  ⇒ 走 HTTP 的方案**骨架一个字不用改**，也不需要新增可测试性接缝。

**Blocked by:** None（决策 272 已落，`docs/decisions.md`）

**Status:** done（2026-09-25 落地；决策 272 的八条形状逐条在列，落地注记见各节）

## 落地清单

### 一、报文第四支
- [x] `NotifyFormat` 加 `BlueBubbles`（`notify.rs:29-38` 旁），`deny_unknown_fields` 下非法值
      解析期拒的既有姿态照旧
- [x] `payload_for` 加第三支（`notify.rs:164-187`）：`{chatGuid, tempGuid, message, method}`
      —— 正文 `message` 沿用 feishu 已在拼的 `title\nbody`（归因白名单与「`detail` 原文不出网」
      **在分流前共用**，一个字不松动）
- [x] `tempGuid` 用 `Ulid::new().to_string()`（`ulid` 已是 core 依赖，零新依赖）
- [x] `method` 不暴露配置，恒 `apple-script`

### 二、值班长回话第二触发面
- [x] 在 `respond()` 收口后加第二入口（**不**写 attention 表）
- [x] 判据：`say` 轮 `traces.len() >= 常数`（具名常量，**不加配置项**，决策 256 的尺子）；
      **判断必须在 `notify()` 之前**，短轮连 cooldown 槽都不碰
- [x] `watch` 播报恒通知；静默轮恒不通知（判别已现成：`foreman.rs:1838`）
- [x] 失败收口必通知（挂在 `record_failed_turn` / `record_interrupted_turn` 上）
- [x] `task_id` 改 `Option`：`None` 时**从 payload 省略该字段**，**不填哨兵**
      （feishu 的 `[AgentPipeline]` 关键词前缀会把哨兵露在人眼前）
- [x] 正文带回话文本（截断，200 字量级）+ **会话名**

### 三、礼貌
- [x] 新类 `foreman_reply`，**自己的 cooldown 槽**（绝不复用 `done`——
      复用会让回话吃掉 `done` 的配额、把真的 `task_done` 静默掉）
- [x] 受免打扰、**不豁免**（叫醒人的是 attention 那条线；回话是摘要不是警报）
- [x] **不进** `tests/fixtures/notification_policy.json`；照 `cancelled` 先例在 `$comment`
      与**两侧**守卫（Rust 表测试 + `frontend/src/lib/notificationPolicyFixture.test.ts:68-71`）
      显式记为差异

### 四、配置两级与秘密面
- [x] 通道四件（通道类型 + 端点 + password + 收件人）**作为一个整体**：`config.toml` 打底、
      界面那份整体覆盖；**不允许混**
- [x] `origin` 报谁生效（中文标签照 `sharePairing.ts:60-66`）
- [x] `cooldown_sec` / `quiet_hours` **只住 `config.toml`**
- [x] password 照 `***` 掩码范式（读回常量掩码；掩码或留空即不改）
- [x] **`without_url()` 之外任何地方不许打拼好的 URL**（URL 此后是代码拼的）

### 五、设置界面
- [x] 新增设置子页（落地页加入口项；分类法要不要加第三类顺带动 design §4.3）
- [x] **一颗总开关**（整条通道开/关，非每类一颗）
- [x] 开启时 ping `GET /api/v1/ping?password=`；够不着**不当成功**
- [x] 缺必填项**报错不静默**（照 `note bad`，不是 268 的「URL 缺席=整段关死」——
      那条是零配置部署的默认姿态，界面开关是显式动作）
- [x] 交互姿态照 `/share`，含 `202 + pending` 那层语义；文案映射纯函数化（可单测）
      ——**实现注记（2026-09-25）**：`202 + pending` 那一层**不适用**（改绑会切断自己的
      连接才有「没读到结果」的空窗；本页每次变更都不动监听器，应答只有成了/没成两种），
      理由记在 `SettingsNotify.svelte` 头注；其余三层（重读为准 / `note ok` / `note bad`）
      照搬。
- [x] 活生效：切换时重建 notifier + `set_notifier`；**新增 `clear_notifier`**

### 六、测试与文档
- [x] `payload_for` 第四支：断言 `{chatGuid, tempGuid, message}`、`message` 以
      `[AgentPipeline]` 开头、**两次调用 `tempGuid` 不同**
- [x] `traces.len()` 判据的单测（纯整数比较）
- [x] 新类 cooldown 槽与免打扰走既有表测试；fixture 差异两侧守卫
- [x] `config.rs` 通知那 6 条测试同批走
- [x] `docs/operations.md` **§12.7 必改**——它今天最后一句「其余 IM / 邮件仍可由通用
      webhook 转发」对 iMessage 将不再成立；另加 BlueBubbles 的部署前置与
      「消息没到先查 系统设置 → 隐私与安全性 → 自动化」
- [x] `design/frontend-design.md` §4.3 / §12.3 + `frontend/src/lib/behavior-map.test.ts`
      （两行已加；该测试是悬空引用扫描器，新行引用的文件落盘即被覆盖，**测试本体零改动**）
- [ ] 落地前用真服务打一次桩验 `message` 字段名（官方文档与源码不一致）
      ——**未勾**：本机没有 BlueBubbles 实例可打；字段名已对**服务端源码**核实过
      （接缝事实一节），真桩验证是部署时动作，留给使用者

### 明确不做（决策 272 已记）
多出口（与飞书并存另立票）／`private-api`／投递结果持久化／每类开关（`notifyOn` 那颗
死开关另立票）／新增测试接缝／非 macOS 平台门控。
