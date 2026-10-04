<script lang="ts">
  import { onMount } from 'svelte';
  import {
    clearNotifyChannel,
    clearNotifyPoliteness,
    clearPushSubscriptions,
    deletePushSubscription,
    getNotifySettings,
    listPushSubscriptions,
    saveNotifyChannel,
    saveNotifyPoliteness,
    setNotifyEnabled,
    subscribePushDevice,
    testNotifyChannel,
  } from '../api/client';
  import type { NotifySettings, PushSubscriptionRow } from '../api/types';
  import { formatDateTime } from '../lib/format';
  import {
    CHANNEL_LABELS,
    NOTIFY_SECRET_MASK,
    buildNotifyChannelPayload,
    buildNotifyTestPayload,
    draftFromSettings,
    notifyOriginLabel,
    validateNotifyDraft,
    type NotifyChannelKind,
    type NotifyDraft,
  } from '../lib/notifyChannel';
  import {
    describeCooldown,
    describeQuietHours,
    draftFromSettings as politenessDraftFromSettings,
    parseNotifyPolitenessDraft,
    type NotifyPolitenessDraft,
  } from '../lib/notifyPoliteness';
  import {
    pushFace,
    pushFaceHint,
    pushFaceLabel,
    readPushEnv,
    readLocalSubscriptionId,
    rememberLocalSubscriptionId,
    forgetLocalSubscriptionId,
    urlBase64ToUint8Array,
    type PushFace,
  } from '../lib/pushSubscribe';

  /**
   * 离线通知设置页（`#/settings/notify`，决策 272⑥⑦⑧；284②③⑤ 添礼貌小节；
   * pwa-webpush 02 添第四通道「浏览器推送」）。
   *
   * **一颗总开关 + 两组单元**：开关管整条通道（非每类一颗）；通道四件（类型 + 端点 +
   * password + 收件人）与礼貌两件（节流 + 免打扰）**各自**作为一个整体覆盖 `config.toml`
   * （组内不允许混，两组互不牵动——284②），「交还配置」也是各交各的（照 `#/settings/market`）。
   * 秘密照 provider 的掩码范式：读回 `***`，掩码或留空 = 不改。
   *
   * 礼貌两件此前只住 `config.toml`（272⑥）；284 把它们搬上这一页，**管的是出机器那条线**
   * ——浏览器 toast 另有 `lib/notificationPolicy.ts` 一份固定表，本页不动它（页面写明）。
   *
   * **第四通道「浏览器推送」**（pwa-webpush 02/03）：通道那一格没有任何必填件（订阅行与
   * VAPID 密钥对都在服务端库里，保存通道时自动生成）；选中它之后多出两段——
   * 「订阅此设备」与「已订阅设备」清单。权限弹窗**只在点击手势里**弹（票面 12 号故事），
   * 进页面不弹；四态（未申请 / 已授权 / 已拒绝给系统设置指引 / iOS 非主屏给添加主屏幕
   * 引导）的判据在 `lib/pushSubscribe.ts::pushFace`，本页只做展示。
   *
   * 交互姿态照 `/share`：每一步成功失败都以**重读到的读数**为准（`note ok` / `note
   * bad`），不拿本地猜测冒充结果。与 `/share` 的差异要说明白：那页有 `202 + pending`
   * 那一层，因为改绑会切断自己的连接；这里的每一次变更都不动监听器，应答只有
   * 「成了 / 没成 + 原因」两种，无需重试窗口。
   *
   * 触发面（后端钉住，本页不改）：任务待办与失败走 attention 漏斗（268），值班长
   * 回话完成走第二入口（272②）——`say` 轮要过工具次数的门，值守播报恒通知。
   */

  let settings = $state<NotifySettings | null>(null);
  let draft = $state<NotifyDraft | null>(null);
  let politeness = $state<NotifyPolitenessDraft | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let saving = $state(false);
  let toggling = $state(false);
  let testing = $state(false);
  let savingPoliteness = $state(false);
  let note = $state<{ kind: 'ok' | 'bad'; message: string } | null>(null);

  // ── 浏览器推送（票 03）──
  /** 这台设备的四态（`readPushEnv` 读环境 → `pushFace` 判据）。 */
  let face = $state<PushFace>({ kind: 'prompt', canSubscribe: true });
  /** 服务端清单（`null` = 这一节还没读到 / 读失败，`listError` 说原因）。 */
  let subscriptions = $state<PushSubscriptionRow[] | null>(null);
  let listError = $state<string | null>(null);
  /** 这台浏览器里现在有没有一条活的订阅（`pushManager.getSubscription()`）。 */
  let subscribedHere = $state(false);
  let pushBusy = $state(false);

  /** 礼貌草稿的实时判读：过了描述两句，没过把错摆出来（不拦输入，只提示）。 */
  const politenessPreview = $derived.by(() => {
    if (!politeness) return null;
    const parsed = parseNotifyPolitenessDraft(politeness);
    if (!parsed.ok) return { ok: false, text: parsed.error };
    return {
      ok: true,
      text: `按现在的填写：${describeCooldown(parsed.payload.cooldown_sec)}；${describeQuietHours(
        parsed.payload.quiet_hours,
      )}`,
    };
  });

  /** 通道是浏览器推送时，这一节才摆出来（清单也是那一刻才去读）。 */
  const pushSection = $derived(settings?.channel === 'webpush');

  async function load() {
    loading = true;
    error = null;
    try {
      settings = await getNotifySettings();
      draft = draftFromSettings(settings);
      politeness = politenessDraftFromSettings(settings);
      if (settings.channel === 'webpush') await loadSubscriptions();
      else {
        subscriptions = null;
        listError = null;
      }
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  /** 读清单 + 本机订阅态。清单读不到不炸整页：那一节自己说原因（局域网未配对是最常见的）。 */
  async function loadSubscriptions() {
    face = pushFace(readPushEnv());
    subscribedHere = await localSubscriptionAlive();
    try {
      subscriptions = (await listPushSubscriptions()).subscriptions;
      listError = null;
    } catch (err) {
      subscriptions = null;
      listError = (err as Error).message;
    }
  }

  /** 本机（这台浏览器）有没有一条活订阅——service worker 没注册 / 不支持时就是没有。 */
  async function localSubscriptionAlive(): Promise<boolean> {
    if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return false;
    try {
      const registration = await navigator.serviceWorker.getRegistration();
      const subscription = await registration?.pushManager.getSubscription();
      return Boolean(subscription);
    } catch {
      return false;
    }
  }

  onMount(() => {
    void load();
  });

  async function toggle(enabled: boolean) {
    toggling = true;
    note = null;
    try {
      const res = await setNotifyEnabled(enabled);
      await load();
      note = {
        kind: 'ok',
        message: res.enabled ? '离线通知已开启，出口已重建。' : '离线通知已关闭，出口已摘除。',
      };
    } catch (err) {
      await load();
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      toggling = false;
    }
  }

  function switchChannel(kind: NotifyChannelKind) {
    if (!draft) return;
    draft = { ...draft, channel: kind };
    note = null;
  }

  async function save() {
    if (!draft) return;
    const invalid = validateNotifyDraft(draft);
    if (invalid) {
      note = { kind: 'bad', message: invalid };
      return;
    }
    saving = true;
    note = null;
    try {
      await saveNotifyChannel(buildNotifyChannelPayload(draft));
      await load();
      note = {
        kind: 'ok',
        message:
          draft.channel === 'webpush'
            ? '通道已保存；VAPID 密钥对已在服务端生成（公钥见下方订阅一节）。'
            : '通道已保存，作为整体覆盖配置文件。',
      };
    } catch (err) {
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      saving = false;
    }
  }

  async function probe() {
    if (!draft) return;
    testing = true;
    note = null;
    try {
      const { test } = await testNotifyChannel(buildNotifyTestPayload(draft));
      note = { kind: test.ok ? 'ok' : 'bad', message: test.message };
    } catch (err) {
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      testing = false;
    }
  }

  async function handBack() {
    saving = true;
    note = null;
    try {
      await clearNotifyChannel();
      await load();
      note = { kind: 'ok', message: '已交还配置文件那一级的通道声明。' };
    } catch (err) {
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      saving = false;
    }
  }

  /**
   * 订阅此设备（票面 12 号故事）：**整条链都在这一次点击的手势里**——
   * `requestPermission` → `pushManager.subscribe` → 上报服务端。
   *
   * 权限弹窗只能由用户手势触发：进页面自动弹会被系统记成「骚扰」并可能被永久拒绝
   * （那之后连点按钮都不再弹），故本页**没有**任何自动弹窗的路径。
   */
  async function subscribeThisDevice() {
    if (!settings) return;
    pushBusy = true;
    note = null;
    try {
      const key = urlBase64ToUint8Array(settings.vapid_public_key);
      if (!key) {
        throw new Error('服务端还没有 VAPID 公钥：先保存一次「浏览器推送」通道（保存时自动生成）。');
      }
      if (!('serviceWorker' in navigator)) {
        throw new Error('这个浏览器没有 service worker，用不了浏览器推送。');
      }
      // ① 权限（必须在手势里）
      const permission = await Notification.requestPermission();
      face = pushFace(readPushEnv());
      if (permission !== 'granted') {
        note = {
          kind: 'bad',
          message:
            permission === 'denied'
              ? pushFaceHint('denied')
              : '这次没有授权。想订阅的话再点一次这颗钮即可。',
        };
        return;
      }
      // ② 订阅（浏览器与推送服务打交道——服务端不参与这一步）
      const registration = await navigator.serviceWorker.ready;
      const subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: key,
      });
      // ③ 上报服务端（过配对令牌守卫：`request` 自动带令牌）
      const json = subscription.toJSON();
      const endpoint = json.endpoint;
      const keys = json.keys;
      if (!endpoint || !keys?.p256dh || !keys.auth) {
        throw new Error('浏览器给的订阅缺件（endpoint / p256dh / auth），这次没上报。');
      }
      const saved = await subscribePushDevice({
        endpoint,
        keys: { p256dh: keys.p256dh, auth: keys.auth },
      });
      rememberLocalSubscriptionId(saved.id);
      await load();
      note = { kind: 'ok', message: '这台设备已订阅：流水线的通知会推到它的通知中心。' };
    } catch (err) {
      await load();
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      pushBusy = false;
    }
  }

  /** 退订此设备：本地先退、再按记得的行 id 删服务端那一行（记不到就只本地退）。 */
  async function unsubscribeThisDevice() {
    pushBusy = true;
    note = null;
    try {
      const registration = await navigator.serviceWorker.getRegistration();
      const subscription = await registration?.pushManager.getSubscription();
      await subscription?.unsubscribe();
      const id = readLocalSubscriptionId();
      if (id !== null) {
        await deletePushSubscription(id);
        forgetLocalSubscriptionId();
      }
      await load();
      note = {
        kind: 'ok',
        message:
          id !== null
            ? '这台设备已退订，服务端那一行也删掉了。'
            : '这台设备已退订（服务端那一行会在下次投递收到 410 时自动清掉）。',
      };
    } catch (err) {
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      pushBusy = false;
    }
  }

  async function revokeSubscriptions() {
    pushBusy = true;
    note = null;
    try {
      const res = await clearPushSubscriptions();
      forgetLocalSubscriptionId();
      await load();
      note = {
        kind: 'ok',
        message: `已清空 ${res.removed} 条订阅${subscribedHere ? '（这台设备的浏览器里那份订阅还在，可从浏览器设置里撤销）' : ''}。`,
      };
    } catch (err) {
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      pushBusy = false;
    }
  }

  async function revokeSubscription(id: number) {
    pushBusy = true;
    note = null;
    try {
      await deletePushSubscription(id);
      // 撤掉的正是这台设备那一行时，浏览器里那份订阅也一并退掉（否则本机仍算「已订阅」，
      // 而服务端已经没有它了——清单与事实会对不上）。
      if (readLocalSubscriptionId() === id) {
        forgetLocalSubscriptionId();
        const registration = await navigator.serviceWorker.getRegistration();
        const subscription = await registration?.pushManager.getSubscription();
        await subscription?.unsubscribe();
      }
      await load();
      note = { kind: 'ok', message: '那一台设备已撤销。' };
    } catch (err) {
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      pushBusy = false;
    }
  }

  async function savePoliteness() {
    if (!politeness) return;
    const parsed = parseNotifyPolitenessDraft(politeness);
    if (!parsed.ok) {
      note = { kind: 'bad', message: parsed.error };
      return;
    }
    savingPoliteness = true;
    note = null;
    try {
      await saveNotifyPoliteness(parsed.payload);
      await load();
      note = {
        kind: 'ok',
        message: '礼貌已保存，作为整体覆盖配置文件；出口已按新值重建。',
      };
    } catch (err) {
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      savingPoliteness = false;
    }
  }

  async function handBackPoliteness() {
    savingPoliteness = true;
    note = null;
    try {
      await clearNotifyPoliteness();
      await load();
      note = { kind: 'ok', message: '已交还配置文件那一级的礼貌（节流与免打扰）。' };
    } catch (err) {
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      savingPoliteness = false;
    }
  }
</script>

<main class="page">
  <a class="crumb" href="#/settings">← 设置</a>
  <div class="p-head">
    <h1 class="p-title">设置 · 离线通知</h1>
  </div>

  <p class="hintline">
    没人盯着浏览器时，这台机器往手机或机器人送信的两条线：任务待办与失败、值班长回话完成。
    出站是 <b>best-effort</b>——投递失败只记日志，绝不拖累流水线。
  </p>

  {#if error}
    <div class="banner error" role="alert">{error}</div>
    <div class="retry">
      <button type="button" class="btn" disabled={loading} onclick={() => void load()}>重试</button>
    </div>
  {:else if loading}
    <div class="banner">正在加载通知设置…</div>
  {:else if settings && draft}
    {#if settings.config_error}
      <!-- 解析不过要说出原因（报错不静默，272⑧）：页面照常显示，红条给下一步。 -->
      <div class="banner error" role="alert">
        当前通知配置有误：{settings.config_error}
      </div>
    {/if}

    <!-- 总开关（272⑧）：一颗管整条通道。交互照 /share：结果以重读为准。 -->
    <section class="block" aria-labelledby="sw-head">
      <h2 class="sec-title" id="sw-head">总开关</h2>
      <div class="row">
        {#if settings.enabled}
          <span class="st run">[ON]</span>
          <span class="sec-note">离线通知开着；生效通道见下方读数。</span>
          <button type="button" class="btn" disabled={toggling} onclick={() => void toggle(false)}>
            {#if toggling}<span class="spin"></span>{/if}关闭通知
          </button>
        {:else}
          <span class="st dim">[OFF]</span>
          <span class="sec-note">离线通知整条关死：两条触发线都不出站。</span>
          <button
            type="button"
            class="btn solid"
            disabled={toggling}
            onclick={() => void toggle(true)}
          >
            {#if toggling}<span class="spin"></span>{/if}开启通知
          </button>
        {/if}
      </div>
    </section>

    <!-- 生效读数：通道是谁定的（两级结构，272⑥）+ 秘密只回掩码 + 礼貌两件只读。 -->
    <section class="block" aria-labelledby="eff-head">
      <h2 class="sec-title" id="eff-head">现在生效的</h2>
      <div class="reg">
        <ul class="reg-rows">
          <li class="reg-row">
            <div class="reg-main">
              <div class="reg-l1">
                <span class="reg-name">
                  {settings.channel ? CHANNEL_LABELS[settings.channel] : '（还没有配置任何通道）'}
                </span>
                <span class="st dim">{notifyOriginLabel(settings.origin)}定的</span>
              </div>
              <div class="reg-l2 mono">
                {#if settings.channel === 'bluebubbles'}
                  <span>端点 {settings.bluebubbles_url || '未填'}</span>
                  <span>password {settings.bluebubbles_password || '未设置'}</span>
                  <span>收件 {settings.bluebubbles_recipient || '未填'}</span>
                {:else if settings.channel}
                  <span>webhook {settings.webhook_url || '未设置'}</span>
                {/if}
              </div>
            </div>
          </li>
        </ul>
      </div>
      <p class="sec-note">
        这一格只说<b>通道</b>是谁定的；节流与免打扰在下面「礼貌」一节里改，那两件
        各报各的来源——通道来自界面不代表礼貌也来自界面。
      </p>
    </section>

    <!-- 通道四件：整体保存；只有当前选中通道的字段会被送出（其余不送也不查）。 -->
    <section class="block" aria-labelledby="form-head">
      <h2 class="sec-title" id="form-head">通道</h2>
      <div class="form">
        <div class="chips" role="radiogroup" aria-label="通道类型">
          {#each Object.entries(CHANNEL_LABELS) as [kind, label] (kind)}
            <button
              type="button"
              class="chip"
              class:on={draft.channel === kind}
              role="radio"
              aria-checked={draft.channel === kind}
              onclick={() => switchChannel(kind as NotifyChannelKind)}
            >
              {label}
            </button>
          {/each}
        </div>

        {#if draft.channel === 'bluebubbles'}
          <label class="field">
            <span class="lab">BlueBubbles 端点</span>
            <input
              class="mono"
              type="text"
              bind:value={draft.bluebubblesUrl}
              placeholder="http://127.0.0.1:1234"
            />
          </label>
          <label class="field">
            <span class="lab">password</span>
            <input class="mono" type="password" bind:value={draft.bluebubblesPassword} />
            <span class="lab sub">
              已存时显示 {NOTIFY_SECRET_MASK}；掩码或留空 = 沿用已存值，不会把掩码存回去。
            </span>
          </label>
          <label class="field">
            <span class="lab">iMessage 收件地址</span>
            <input
              class="mono"
              type="text"
              bind:value={draft.bluebubblesRecipient}
              placeholder="you@icloud.com 或 +8613800000000"
            />
          </label>
          <div class="acts">
            <button type="button" class="btn" disabled={testing} onclick={() => void probe()}>
              {#if testing}<span class="spin"></span>{/if}测试连接
            </button>
          </div>
          <p class="sec-note">
            前置：Mac 上装好 <b>BlueBubbles</b> 并登录 iMessage，服务端开启；消息没到先查
            系统设置 → 隐私与安全性 → 自动化里有没有被拒的授权。
          </p>
        {:else if draft.channel === 'webpush'}
          <!-- 浏览器推送（pwa-webpush 02）：这一格没有必填件——订阅与 VAPID 密钥对都在
               服务端库里，保存这个动作本身就会把密钥对生成出来。 -->
          <p class="sec-note">
            这一格没有要填的东西：保存一次就会在服务端生成一对 <b>VAPID</b> 密钥（首次
            启用自动生成），然后在下面的「订阅」一节里把这台设备订上。
          </p>
          {#if settings.vapid_public_key}
            <p class="sec-note mono">
              公钥 {settings.vapid_public_key.slice(0, 16)}… · 私钥 {settings.vapid_private_key}
            </p>
          {:else}
            <p class="sec-note bad">还没有 VAPID 密钥——保存一次这个通道就会生成。</p>
          {/if}
        {:else}
          <label class="field">
            <span class="lab">webhook 地址（含 token 的完整 URL）</span>
            <input class="mono" type="text" bind:value={draft.webhookUrl} />
            <span class="lab sub">
              已存时显示 {NOTIFY_SECRET_MASK}；掩码或留空 = 沿用已存值。
            </span>
          </label>
        {/if}

        <div class="acts">
          <button type="button" class="btn solid" disabled={saving} onclick={() => void save()}>
            {#if saving}<span class="spin"></span>{/if}保存通道
          </button>
          {#if settings.origin === 'settings'}
            <button type="button" class="btn quiet" disabled={saving} onclick={() => void handBack()}>
              交还配置文件
            </button>
          {/if}
        </div>
        <p class="sec-note">
          保存的是**整体覆盖**：四件要么全来自界面、要么全来自配置文件，不允许混。
          开关开着时保存即生效；BlueBubbles 会在保存与开启时先探活，够不着不当成功。
        </p>
      </div>
    </section>

    <!-- 浏览器推送的订阅（pwa-webpush 03）：权限四态 + 订阅钮 + 已订阅设备清单。
         只在通道 = 浏览器推送时摆出来——别的通道下这一节没有意义（订阅了也不会被推）。 -->
    {#if pushSection}
      <section class="block" aria-labelledby="push-head">
        <h2 class="sec-title" id="push-head">订阅</h2>
        <p class="hintline">
          推送由浏览器的推送服务转交（iOS 走 APNs），**同一条通知每台订阅设备各收一份**；
          一个设备一份订阅，这里管的就是这份清单。
        </p>
        <div class="form">
          <div class="row">
            <span class="st dim">{pushFaceLabel(face.kind)}</span>
            <span class="sec-note inline">{pushFaceHint(face.kind)}</span>
          </div>
          <div class="acts">
            {#if face.canSubscribe}
              {#if subscribedHere}
                <span class="st run">[已订阅]</span>
                <button
                  type="button"
                  class="btn"
                  disabled={pushBusy}
                  onclick={() => void unsubscribeThisDevice()}
                >
                  {#if pushBusy}<span class="spin"></span>{/if}退订此设备
                </button>
              {:else}
                <button
                  type="button"
                  class="btn solid"
                  disabled={pushBusy || !settings.vapid_public_key}
                  onclick={() => void subscribeThisDevice()}
                >
                  {#if pushBusy}<span class="spin"></span>{/if}订阅此设备
                </button>
                {#if !settings.vapid_public_key}
                  <span class="sec-note inline">先保存一次「浏览器推送」通道。</span>
                {/if}
              {/if}
            {:else if face.kind === 'ios-needs-install'}
              <span class="banner-inline">请先添加到主屏幕</span>
            {/if}
          </div>
        </div>

        <!-- 已订阅设备清单（票 04）：时间 / UA / 单个撤销 / 全部清空。 -->
        <div class="reg">
          {#if listError}
            <div class="banner error" role="alert">{listError}</div>
          {:else if subscriptions === null}
            <div class="banner">正在读取已订阅设备…</div>
          {:else if subscriptions.length === 0}
            <p class="sec-note">
              还没有设备订阅。在这台设备上点上面的「订阅此设备」——或者在手机上打开
              同样的地址（HTTPS）再订一次。
            </p>
          {:else}
            <ul class="reg-rows">
              {#each subscriptions as sub (sub.id)}
                <li class="reg-row">
                  <div class="reg-main">
                    <div class="reg-l1">
                      <span class="reg-name mono">{sub.endpoint_hint}</span>
                      <span class="st dim">{formatDateTime(sub.created_at)}</span>
                    </div>
                    <div class="reg-l2 mono">
                      <span>{sub.user_agent || '（浏览器没报 UA）'}</span>
                    </div>
                  </div>
                  <button
                    type="button"
                    class="btn quiet"
                    disabled={pushBusy}
                    onclick={() => void revokeSubscription(sub.id)}
                  >
                    撤销
                  </button>
                </li>
              {/each}
            </ul>
            <div class="acts">
              <button
                type="button"
                class="btn quiet"
                disabled={pushBusy}
                onclick={() => void revokeSubscriptions()}
              >
                全部清空
              </button>
            </div>
          {/if}
        </div>
        <p class="sec-note">
          清单只显示 endpoint 的**摘要**（它是能往那台设备推报文的能力地址，不该整条摆出来）；
          服务端发现某条订阅已失效（推送服务回 410）时会自动把它从清单里删掉。
        </p>
      </section>
    {/if}

    <!-- 礼貌两件（284②）：与通道单元**各自成立**的第二组——同构的整体覆盖 + provenance。 -->
    {#if politeness}
      <section class="block" aria-labelledby="polite-head">
        <h2 class="sec-title" id="polite-head">礼貌</h2>
        <p class="hintline">
          什么时候<b>准吵</b>：节流窗口与免打扰时段。这两件管的是<b>出机器那条线</b>
          （webhook / 飞书 / iMessage）；浏览器里的弹窗另有自己的一份固定表，不在这里改。
        </p>
        <div class="form">
          <div class="row">
            <span class="st dim">{notifyOriginLabel(settings.politeness_origin)}定的</span>
            <span class="sec-note inline">
              下面是<b>生效值</b>（预填）：保存即整体覆盖配置文件那一份。
            </span>
          </div>
          <label class="field">
            <span class="lab">节流（秒）</span>
            <!-- 用 text + inputmode 而不是 `type=number`：number 输入绑出来的值是 number
                 （空则是 undefined），判读那一层按字符串写就会当场抛——手机上的数字键盘
                 由 inputmode 给，越界由判读给（有话说，不被浏览器静默吞掉）。 -->
            <input
              class="mono"
              type="text"
              inputmode="numeric"
              bind:value={politeness.cooldownSec}
            />
            <span class="lab sub">同类通知在这个窗口内只出一条；0 = 不节流。</span>
          </label>
          <div class="hours">
            <label class="field">
              <span class="lab">免打扰开始（整点）</span>
              <input
                class="mono"
                type="text"
                inputmode="numeric"
                bind:value={politeness.quietStart}
              />
            </label>
            <label class="field">
              <span class="lab">结束（整点）</span>
              <input
                class="mono"
                type="text"
                inputmode="numeric"
                bind:value={politeness.quietEnd}
              />
            </label>
          </div>
          {#if politenessPreview}
            <p class="sec-note" class:bad={!politenessPreview.ok}>{politenessPreview.text}</p>
          {/if}
          <p class="sec-note">
            免打扰期间除<b>待办</b>之外不出站；失败静音、只累计，时段结束后补一条摘要
            （等人那条豁免；<span class="mono">failed</span> 夜间不再恒发，前端 toast
            那份表不受影响）；起止填同一个数 = 全天都送，跨零点直接写
            <span class="mono">22 → 8</span>。按<b>服务器本地时间</b>算。
          </p>
          <div class="acts">
            <button
              type="button"
              class="btn solid"
              disabled={savingPoliteness}
              onclick={() => void savePoliteness()}
            >
              {#if savingPoliteness}<span class="spin"></span>{/if}保存礼貌
            </button>
            {#if settings.politeness_origin === 'settings'}
              <button
                type="button"
                class="btn quiet"
                disabled={savingPoliteness}
                onclick={() => void handBackPoliteness()}
              >
                交还配置文件
              </button>
            {/if}
          </div>
        </div>
      </section>
    {/if}

    {#if note}
      {#if note.kind === 'ok'}
        <div class="banner ok" role="status">{note.message}</div>
      {:else}
        <div class="banner error" role="alert">{note.message}</div>
      {/if}
    {/if}
  {/if}
</main>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 20px 24px 60px;
  }
  .banner {
    padding: 10px 12px;
    border: 2px solid var(--pane);
    color: var(--text-3);
    font-size: 12px;
  }
  .banner.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  .banner.ok {
    border-color: var(--go);
    color: var(--go);
  }
  /* 权限态里那种「一句话代替按钮」的标牌（iOS 非主屏）：与 .st 同一套基元形状，
     颜色取 --pending（「等你动手」那一档，不新增 token）。 */
  .banner-inline {
    padding: 6px 10px;
    border: 2px solid var(--pending);
    color: var(--text-hi);
    font-size: 12px;
  }
  /* 设备清单每行右侧那颗「撤销」钮：标题行与它同行、正文行在下面。 */
  .reg-row {
    align-items: flex-start;
    gap: 10px;
  }
  .retry {
    margin: 8px 0 12px;
  }
  .block {
    margin-bottom: 22px;
  }
  .sec-title {
    font-size: 12px;
    color: var(--text-hi);
    margin-bottom: 6px;
  }
  .sec-note {
    color: var(--text-3);
    line-height: 1.8;
    max-width: 86ch;
    margin-top: 8px;
  }
  /* 判读没过的那一行与红条同一档颜色（不新增 token）。 */
  .sec-note.bad {
    color: var(--stop);
  }
  .sec-note.inline {
    margin-top: 0;
  }
  .hours {
    display: flex;
    gap: 12px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 60ch;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .lab {
    font-size: 12px;
    color: var(--text-3);
  }
  .lab.sub {
    line-height: 1.8;
  }
  .acts {
    display: flex;
    gap: 8px;
  }
  .chips {
    display: flex;
    gap: 0;
    flex-wrap: wrap;
  }
  /* 通道选择 = 台账基元的选中语汇：2px 描边、相邻共享边、零圆角、选中亮描边 + wash 底
     （与 §4.4 状态过滤槽同一套形状纪律；不新增颜色）。 */
  .chip {
    padding: 8px 12px;
    border: 2px solid var(--pane);
    background: var(--bg);
    color: var(--text);
    font-size: 12px;
    margin-left: -2px;
  }
  .chip:first-child {
    margin-left: 0;
  }
  .chip:hover {
    background: var(--panel);
  }
  .chip.on {
    border-color: var(--text-hi);
    background: var(--wash);
    color: var(--text-hi);
  }
</style>
