# 01: 礼貌两件（节流 / 免打扰）上设置页

**What to build:**（裁决已落决策 284，本票不重开裁决，只落地）

三条需求：① `cooldown_sec` / `quiet_hours` 在设置页可改（此前只住 `config.toml`）；
② 与通道单元**各自成立**的第二个两级单元（各自 origin、各交各的）；③ 保存即生效
（出口按新值重建），越界报错不静默。

**接缝事实（2026-09-25 探查；均为本次实读）**：

- 配置面：`config.rs:512-543` `NotifyConfig{webhook_url, cooldown_sec, quiet_hours, format,
  bluebubbles_*}`，`#[serde(default, deny_unknown_fields)]`（撤字段会让既有 config.toml 起不来）。
- 出口：`notify.rs` `WebhookNotifier`——礼貌此前是**两个散值**（`cooldown_sec` + `quiet`），
  三处构造点（`serve.rs:386`、`routes/notify.rs` 的 `apply_target`、L2 测试的 `attach_*`）。
- 存储：单行表 `kanban_notify_channel`（0027）已有「界面那一级」的骨架与
  `NotifySettingsState{enabled, unit}`；`set_notifier` / `clear_notifier` 是活生效的两个动作。
- 界面：`#/settings/notify`（`SettingsNotify.svelte`）此前把礼貌两件**只读展示**，
  并在文案里写明「只住 config.toml，界面不改它们（272⑥）」。
- 前端 toast 的礼貌是另一份固定表（`lib/notificationPolicy.ts` + `tests/fixtures/
  notification_policy.json` 的跨语言 fixture），**本次不动**——它管浏览器弹窗。

**Blocked by:** None（决策 284 已落，`docs/decisions.md`）

**Status:** done（2026-09-25 落地；决策 284 各条逐条在列，落地注记见各节）

## 落地清单

### 一、core：值对象 + 两级解析 + 范围闸
- [x] `NotifyPoliteness { cooldown_sec, quiet_hours }`（`Default` 从 `NotifyConfig::default()`
      取，一处定义）
- [x] `resolve_politeness(config, state)`：单元 > `config.toml`；总开关**不参与**
      （关着也能报「开着会是哪一份」）
- [x] `validate_politeness`：0–`COOLDOWN_SEC_MAX`(86 400) 秒 / 0–23 整点，`Err` 是面向
      用户的中文报文（照 `ping_bluebubbles` 的形状）；`config.toml` 那一级不过这道闸
- [x] `WebhookNotifier` 构造参数收成 `NotifyPoliteness` 一个值对象（散值漏带一份不报警，
      值对象会）+ `politeness()` 只读访问器
- [x] `NotifySettingsState` 添 `politeness: Option<NotifyPoliteness>`

### 二、存储（迁移 0029）
- [x] 三列 `cooldown_sec` / `quiet_start` / `quiet_end`，**同生同死**；`cooldown_sec IS NULL`
      = 整组没保存过
- [x] 半行（只手改过库）按「没保存过」处理：读取侧不崩，也不把半份当生效值
- [x] `set_notify_politeness` / `clear_notify_politeness`；清礼貌**不动**开关与通道单元，
      反之亦然（三件事三个钮）
- [x] `Store::notifier()` 由 `pub(crate)` 放宽到 `pub`：活生效的断言要看得见在飞的那份出口

### 三、端点与活生效
- [x] `GET /notify/settings` 添 `politeness_origin`（通道与礼貌各报各的），
      `cooldown_sec` / `quiet_hours` 改报**生效值**
- [x] `PUT /notify/politeness`（整体覆盖；越界 400 且不落库）/ `DELETE /notify/politeness`
      （交还配置；开关与通道不动）
- [x] 三条写路径（开关 / 通道 / 礼貌）重建出口时都带**解析后的**礼貌——否则保存通道会把
      界面上的礼貌悄悄换回配置那一份
- [x] `serve.rs` 启动那一份同样走解析（启动也不能漏掉界面单元）

### 四、界面
- [x] 设置页新增「礼貌」小节：生效值预填 + origin 标签 + 三个数字输入 + 实时判读一句话
      + 保存 / 交还两颗钮
- [x] `lib/notifyPoliteness.ts`：读数 → 草稿、判读（校验与取值同一处）、两句描述文案
      （判据纯函数化，可单测）
- [x] 页面写明**它管的是出机器那条线**（浏览器 toast 另有自己一份固定表）——257 的归属
      诚实口径

### 五、测试与文档
- [x] L1：`notify.rs` 3 条（解析整体覆盖 / 开关不参与 / 范围闸点名）；
      `storage::notify_channel` 2 条（互不牵动 / 半行）
- [x] L2：`tests/integration/notify.rs` +1 条——出口按它被造出来那份礼貌说话
      （配置级夜里静音 → 换单元那份连夜放行且 `0` 节流不挡）
- [x] L3：`api_contract.rs` +4 条（缺省读数 / 保存后读数 + **在飞出口真的重建** /
      越界不落库 / 交还不动其余两件）
- [x] §9：`lib/notifyPoliteness.test.ts` 4 条 + `routes/SettingsNotify.test.ts` 4 条接线
      （页面自 272 起零测试，这次补上；写它时当场抓到一处真 bug：`type="number"` 绑出来
      的值是 number（空则 `undefined`），判读层按字符串写会在用户第一次输入时抛——
      三个输入框改用 `type="text" inputmode="numeric"`）
- [x] `docs/decisions.md` **284**（显式修订 272⑥）、`design/frontend-design.md` §4.3 / §12.3、
      `docs/operations.md` §12.7、`docs/testing.md` 行

**明确不做（决策 284④ 已记）**：让浏览器 toast 也读这份值（要启动期拉取 + 陈旧态 + 两处
生效时序，而 toast 只在页面开着时可见）；`config.toml` 那一级的范围校验（只有解析期校验，
47 / 103 / 134 姿态不动）；按类开关（`notifyOn` 那颗死开关另立票）。
