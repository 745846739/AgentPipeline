<script lang="ts">
  import { onMount } from 'svelte';
  import { getOffload, getRtk, setOffload, setRtk } from '../api/client';
  import type { OffloadSettings, RtkSettings } from '../api/types';
  import {
    manualPathVisible,
    normalizeManualPath,
    probeLine,
    rtkState,
    saveNote,
    stateLabel,
  } from '../lib/rtkToggle';

  /**
   * 「命令执行」设置页（`#/settings/tools`，决策 297 / 票 05）。
   *
   * **这一页存在的意义**：让「启用」与「真能用」成为同一件事。命令交给 rtk 改写之后，
   * 桌面壳从 Finder 起时继承的是系统的最小 PATH——「本机装了 rtk」与「这个服务能用 rtk」
   * 是两个答案，而只有后者有意义。故本页的读数不是存下来的那句「已启用」，而是**每次
   * 打开都现做一次**的活体探测（路径 / 版本 / 可用 / 原因）。
   *
   * 交互姿态照 `/settings/foreman` 与 `/settings/notify`：每一步成功失败都以**重读到的
   * 读数**为准；探测失败**不拦保存**（决策 185 / 297）——一个输出优化器不该有权限拦人，
   * 而「先开开关、后装二进制」是常见顺序。失败原样摆出来，不静默成功也不静默失败。
   */

  let settings = $state<RtkSettings | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let note = $state<{ kind: 'ok' | 'bad'; message: string } | null>(null);
  /** 手填框里的草稿（保存时归一：空白 = 回到自动解析）。 */
  let manualDraft = $state('');

  /** 重活外发（票 runner-offload/05）：同一页的第二颗钮，姿态与 rtk 完全同构。 */
  let offload = $state<OffloadSettings | null>(null);
  let offloadBusy = $state(false);
  let offloadNote = $state<{ kind: 'ok' | 'bad'; message: string } | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      settings = await getRtk();
      manualDraft = settings.path ?? '';
      offload = await getOffload();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  /** 拨外发开关：保存即活，探测失败不拦（照 /rtk 的纪律）。 */
  async function saveOffload(enabled: boolean) {
    offloadBusy = true;
    offloadNote = null;
    try {
      const saved = await setOffload({ enabled });
      offload = saved;
      const probe = saved.probe;
      if (enabled && (!probe.gh_authed || !probe.workflow_present)) {
        offloadNote = {
          kind: 'ok',
          message: '已开启。探测有一项没过（见下方读数）——先开开关、后补条件是常见顺序，但开启期间的命令会回退本机执行。',
        };
      } else {
        offloadNote = { kind: 'ok', message: enabled ? '已开启：agent 可把重活外发 GitHub。' : '已关闭：全部命令本机运行。' };
      }
    } catch (err) {
      offloadNote = { kind: 'bad', message: (err as Error).message };
    } finally {
      offloadBusy = false;
    }
  }

  onMount(() => {
    void load();
  });

  /** 一次保存：开关与手填路径一起交出去，随后**重读**（探测是活体的）。 */
  async function save(enabled: boolean, path: string) {
    busy = true;
    note = null;
    try {
      const saved = await setRtk({ enabled, path });
      note = saveNote(saved.enabled, saved.probe);
      await load();
    } catch (err) {
      await load();
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      busy = false;
    }
  }

  // 变量名**不能叫 `state`**：那会让 `$state(...)` 变成「对 `state` 的 store 订阅」，
  // 于是一整页的 runes 全部解析失败（svelte-check 的原话是 `$state used before its declaration`）。
  const mode = $derived(settings ? rtkState(settings) : 'off');
  const showManual = $derived(settings ? manualPathVisible(settings) : false);
</script>

<main class="page">
  <a class="crumb" href="#/settings">← 设置</a>
  <div class="p-head">
    <h1 class="p-title">设置 · 命令执行</h1>
  </div>

  <p class="hintline">
    流水线里跑的每一条命令都由<b>这台机器上的 rtk</b> 过一遍：认得的命令换成更省 Token 的跑法
    （读文件、跑测试、查 lint），不认得的原样执行。它只换<b>怎么跑</b>，不换跑的是什么——
    出口策略那道闸照旧按原话判。
  </p>

  {#if error}
    <div class="banner error" role="alert">{error}</div>
    <div class="retry">
      <button type="button" class="btn" disabled={loading} onclick={() => void load()}>重试</button>
    </div>
  {:else if loading}
    <div class="banner">正在加载命令执行设置…</div>
  {:else if settings}
    <section class="block" aria-labelledby="sw-head">
      <h2 class="sec-title" id="sw-head">开关</h2>
      <div class="row">
        <span class="st" class:run={mode === 'ready'} class:dim={mode === 'off'} class:warn={mode === 'unavailable'}>
          {stateLabel(mode)}
        </span>
        <span class="sec-note inline">
          {#if mode === 'off'}
            命令按原样跑（{settings.origin === 'settings' ? '界面关掉的' : '缺省关'}）。
          {:else if mode === 'ready'}
            命令交给 rtk 改写。
          {:else}
            已启用，但本服务用不了它。
          {/if}
        </span>
        {#if settings.enabled}
          <button
            type="button"
            class="btn"
            disabled={busy}
            onclick={() => void save(false, manualDraft)}
          >
            {#if busy}<span class="spin"></span>{/if}关掉
          </button>
        {:else}
          <button
            type="button"
            class="btn solid"
            disabled={busy}
            onclick={() => void save(true, manualDraft)}
          >
            {#if busy}<span class="spin"></span>{/if}打开
          </button>
        {/if}
      </div>
      <p class="sec-note">
        保存即活：下一条命令就按新值走，<b>不必重启</b>；已经在跑的那一条不受影响。
        闸门命令（跑测试 / lint）<b>不改写</b>——它的输出同时是判断依据与留档的证据。
      </p>
    </section>

    <section class="block" aria-labelledby="probe-head">
      <h2 class="sec-title" id="probe-head">本机可用性</h2>
      <p class="sec-note inline">{probeLine(settings.probe)}</p>
      <p class="sec-note">
        这一格是<b>每次打开这一页现问一次</b>这台机器得到的，不是上次存下来的结论。判定要三条
        全过：找得到一个绝对路径、它跑得起 `--version`、它回得出一段可解析的改写。
      </p>

      {#if showManual}
        <label class="field" for="rtk-path">
          <span class="field-label">手填 rtk 的绝对路径</span>
          <input
            id="rtk-path"
            class="mono"
            type="text"
            bind:value={manualDraft}
            placeholder="/usr/local/bin/rtk"
            spellcheck="false"
            autocomplete="off"
          />
        </label>
        <div class="row">
          <button
            type="button"
            class="btn solid"
            disabled={busy}
            onclick={() => void save(true, normalizeManualPath(manualDraft) ?? '')}
          >
            {#if busy}<span class="spin"></span>{/if}保存路径
          </button>
        </div>
        <p class="sec-note">
          填了就<b>以它为准</b>（自动找的那一份不再参与）；填错时如实报错、不会偷偷回落到自动解析。
          留空 = 回到自动找。
        </p>
      {/if}
    </section>

    <section class="block" aria-labelledby="offload-head">
      <h2 class="sec-title" id="offload-head">重活外发 GitHub</h2>
      {#if offload}
        <div class="row">
          <span class="st" class:run={offload.enabled} class:dim={!offload.enabled}>
            {offload.enabled ? '开启：重活外发' : '关闭：全部本机'}
          </span>
          <span class="sec-note inline">
            {#if !offload.enabled}
              {offload.origin === 'settings' ? '界面关掉的' : '缺省关'}——构建、测试、lint 都在本机跑。
            {:else if offload.probe.gh_authed && offload.probe.workflow_present}
              agent 可把全量测试 / clippy / 构建外发到 GitHub runner（只回文本结果）。
            {:else}
              已开启，但探测有缺口：开启期间的命令会<b>回退本机执行并留痕</b>。
            {/if}
          </span>
          {#if offload.enabled}
            <button type="button" class="btn" disabled={offloadBusy} onclick={() => void saveOffload(false)}>
              {#if offloadBusy}<span class="spin"></span>{/if}关掉
            </button>
          {:else}
            <button type="button" class="btn solid" disabled={offloadBusy} onclick={() => void saveOffload(true)}>
              {#if offloadBusy}<span class="spin"></span>{/if}打开
            </button>
          {/if}
        </div>
        <p class="sec-note">
          这一格同样是<b>每次打开现问一次</b>：gh 登录态（{offload.probe.gh_authed ? '✅ 已登录' : `❌ ${offload.probe.gh_reason ?? '未登录'}`}）、
          外发工作流在场（{offload.probe.workflow_present ? '✅ 在场' : '❌ 未见 offload.yml'}）。探测失败<b>不拦保存</b>。
          交互式走查（起服务、看页面）永远在本机——它等不起 runner 的排队。
        </p>
        <p class="sec-note">
          {#if offload.last_failure_at}
            <span class="st warn" role="alert">最近一次链路失败：{offload.last_failure_at}</span>——外发在静默降级，命令实际都在本机跑。
          {:else}
            最近一次链路失败：<span class="st dim">无</span>
          {/if}
          （远端命令跑红不算链路失败；外发成功跑通一轮即清。）
        </p>
        {#if offloadNote}
          {#if offloadNote.kind === 'ok'}
            <div class="banner ok" role="status">{offloadNote.message}</div>
          {:else}
            <div class="banner error" role="alert">{offloadNote.message}</div>
          {/if}
        {/if}
      {:else}
        <div class="banner">正在加载外发设置…</div>
      {/if}
    </section>

    <section class="block" aria-labelledby="skill-head">
      <h2 class="sec-title" id="skill-head">和已装的技能什么关系</h2>
      <p class="sec-note">
        装了 rtk 的技能之后，它讲的「什么时候用 rtk」就成了多余的说明书——命令执行这一层已经
        替你做了这件事。技能配置<b>本页不会自动改</b>：要不要卸、什么时候卸，由你在
        <a href="#/settings/market">技能市场</a>与
        <a href="#/settings/stages">阶段配置</a>里自己决定。
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
  .st {
    font-family: var(--font-mono);
    color: var(--text-3);
  }
  .st.run {
    color: var(--go);
  }
  .st.warn {
    color: var(--stop);
  }
  .st.dim {
    color: var(--text-4);
  }
  .field {
    display: block;
    margin-top: 10px;
  }
  .field-label {
    display: block;
    color: var(--text-3);
    font-size: 12px;
    margin-bottom: 4px;
  }
  .field input {
    width: 100%;
    max-width: 62ch;
    padding: 6px 8px;
    background: var(--bg);
    color: var(--text-hi);
    border: 2px solid var(--pane);
    border-radius: 0;
  }
</style>
