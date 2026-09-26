<script lang="ts">
  import { onMount } from 'svelte';
  import { getForemanWatch, setForemanWatch } from '../api/client';
  import type { ForemanWatchSettings } from '../api/types';

  /**
   * 值守轮设置页（`#/settings/foreman`，决策 287 / 票 02）。
   *
   * **一颗开关，语义是「关掉跑」**：关掉后值守轮不再自己醒——有待办也不醒、不花钱，
   * 在飞的那一轮不受影响、不被打断；打开后下一趟（值守循环每 10s 看一次）恢复。
   * 保存即活，**不必重启**——后端与界面读的是库里同一行。
   *
   * 文案要给到那句话（票 02）：关掉 = **今晚没人看**——它决定「今晚没人在看」
   * 这件事说不说得出口。它不是「跑着但不吵」：吵不吵归离线通知的总开关与礼貌两件
   * （`#/settings/notify`），两边不造第二份。
   *
   * 节奏五个数（`[pipeline] watch_*`）**只读展示**：它们是 config.toml 那一级的
   * 节奏参数，改它们是深思熟虑的编辑，不是一次点击（本页不开写口，票 02）。
   *
   * 交互姿态照 `/settings/notify`：每一步成功失败都以**重读到的读数**为准。
   */

  let settings = $state<ForemanWatchSettings | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let toggling = $state(false);
  let note = $state<{ kind: 'ok' | 'bad'; message: string } | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      settings = await getForemanWatch();
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
      await setForemanWatch(enabled);
      await load();
      note = {
        kind: 'ok',
        message: enabled
          ? '值守已打开：下一趟（不超过 10 秒）恢复，今晚有人看。'
          : '值守已关掉：今晚没人看——在飞的那一轮不受影响，不会再起新的。',
      };
    } catch (err) {
      await load();
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      toggling = false;
    }
  }

  /** 节奏五个数的人话（只读展示；单位与缺省值都在后端那张表里）。 */
  const cadence = $derived.by(() => {
    if (!settings) return [];
    const c = settings.config;
    return [
      { name: '事件新鲜窗口', value: `${c.watch_event_window_minutes} 分钟`, note: '多久之内发生的事才值得写进待办' },
      { name: '卡住的宽限', value: `${c.watch_owner_stuck_minutes} 分钟`, note: '多长时间没有心跳才算「卡住」' },
      { name: '去抖窗口', value: `${c.watch_debounce_sec} 秒`, note: '窗口内攒批，到期才唤醒一次' },
      { name: '同任务冷却', value: `${c.watch_task_cooldown_minutes} 分钟`, note: '刚处理过的任务，新事件不单独唤醒' },
      { name: '唤醒上限', value: `${c.watch_max_wakes_per_hour} 次/小时`, note: '触顶留一行账，待办不丢' },
    ];
  });
</script>

<main class="page">
  <a class="crumb" href="#/settings">← 设置</a>
  <div class="p-head">
    <h1 class="p-title">设置 · 值守轮</h1>
  </div>

  <p class="hintline">
    值班长每 10 秒看一眼有没有需要人管的活，有就自己醒来说一句（对讲台的「值守台账」）。
    这里决定它<b>今晚起不起</b>；吵不吵（往不往手机送信）在 <a href="#/settings/notify">离线通知</a>里。
  </p>

  {#if error}
    <div class="banner error" role="alert">{error}</div>
    <div class="retry">
      <button type="button" class="btn" disabled={loading} onclick={() => void load()}>重试</button>
    </div>
  {:else if loading}
    <div class="banner">正在加载值守设置…</div>
  {:else if settings}
    <section class="block" aria-labelledby="sw-head">
      <h2 class="sec-title" id="sw-head">值守开关</h2>
      <div class="row">
        {#if settings.enabled}
          <span class="st run">[ON]</span>
          <span class="sec-note inline">
            值守开着（{settings.origin === 'settings' ? '界面保存的' : '缺省开'}）；今晚有人看。
          </span>
          <button type="button" class="btn" disabled={toggling} onclick={() => void toggle(false)}>
            {#if toggling}<span class="spin"></span>{/if}关掉值守
          </button>
        {:else}
          <span class="st dim">[OFF]</span>
          <span class="sec-note inline">
            值守已关（{settings.origin === 'settings' ? '界面保存的' : '缺省开'}）：<b>今晚没人看</b>。
          </span>
          <button
            type="button"
            class="btn solid"
            disabled={toggling}
            onclick={() => void toggle(true)}
          >
            {#if toggling}<span class="spin"></span>{/if}打开值守
          </button>
        {/if}
      </div>
      <p class="sec-note">
        关掉的是「跑」：值守轮不再自己醒（有待办也不醒、不花钱），在飞的那一轮不受影响；
        打开后下一趟（不超过 10 秒）恢复，<b>不必重启</b>。想让它「跑着但别吵」去
        <a href="#/settings/notify">离线通知</a>——那是另一颗钮。
      </p>
    </section>

    <section class="block" aria-labelledby="cad-head">
      <h2 class="sec-title" id="cad-head">值守的节奏</h2>
      <div class="reg">
        <ul class="reg-rows">
          {#each cadence as item (item.name)}
            <li class="reg-row">
              <div class="reg-main">
                <div class="reg-l1"><span class="reg-name">{item.name}</span></div>
                <div class="reg-l2"><span>{item.note}</span></div>
              </div>
              <span class="reg-val mono">{item.value}</span>
            </li>
          {/each}
        </ul>
      </div>
      <p class="sec-note">
        这五个数住在 config.toml 的 <span class="mono">[pipeline]</span> 段（<span class="mono">watch_*</span>），
        本页只展示不改——它们是值守的节奏参数，改它们是深思熟虑的编辑。
      </p>
    </section>

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
  .sec-note.inline {
    margin-top: 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  /* 只读台账盒 = §3.1 的台账基元：2px 描边、行间 --wash 分隔、零圆角。 */
  .reg {
    border: 2px solid var(--pane);
    background: var(--bg);
  }
  .reg-rows {
    list-style: none;
  }
  .reg-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
  }
  .reg-row + .reg-row {
    border-top: 2px solid var(--wash);
  }
  .reg-name {
    color: var(--text-hi);
  }
  .reg-l2 {
    color: var(--text-3);
    font-size: 12px;
    line-height: 1.8;
  }
  .reg-val {
    color: var(--text-3);
    white-space: nowrap;
  }
</style>
