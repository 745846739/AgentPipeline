<script lang="ts">
  import { onMount } from 'svelte';
  import {
    clearNotifyChannel,
    clearNotifyPoliteness,
    getNotifySettings,
    saveNotifyChannel,
    saveNotifyPoliteness,
    setNotifyEnabled,
    testNotifyChannel,
  } from '../api/client';
  import type { NotifySettings } from '../api/types';
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

  /**
   * 离线通知设置页（`#/settings/notify`，决策 272⑥⑦⑧；284②③⑤ 添礼貌小节）。
   *
   * **一颗总开关 + 两组单元**：开关管整条通道（非每类一颗）；通道四件（类型 + 端点 +
   * password + 收件人）与礼貌两件（节流 + 免打扰）**各自**作为一个整体覆盖 `config.toml`
   * （组内不允许混，两组互不牵动——284②），「交还配置」也是各交各的（照 `#/settings/market`）。
   * 秘密照 provider 的掩码范式：读回 `***`，掩码或留空 = 不改。
   *
   * 礼貌两件此前只住 `config.toml`（272⑥）；284 把它们搬上这一页，**管的是出机器那条线**
   * ——浏览器 toast 另有 `lib/notificationPolicy.ts` 一份固定表，本页不动它（页面写明）。
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

  async function load() {
    loading = true;
    error = null;
    try {
      settings = await getNotifySettings();
      draft = draftFromSettings(settings);
      politeness = politenessDraftFromSettings(settings);
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
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
      note = { kind: 'ok', message: '通道已保存，作为整体覆盖配置文件。' };
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
            免打扰期间除<b>待办与失败</b>之外不出站（失败恒发、等人那条豁免——与前端
            toast 同一张表的语义）；起止填同一个数 = 全天都送，跨零点直接写
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
