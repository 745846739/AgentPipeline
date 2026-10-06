<script lang="ts">
  import { onMount } from 'svelte';
  import { getCompaction, setCompaction } from '../api/client';
  import type { CompactionSettings } from '../api/types';

  /**
   * 管线压缩设置页（`#/settings/compaction`，long-run-budget 票 02）。
   *
   * **两个旋钮，管的是同一个算术**：L3 按轮压缩的触发线（token 硬底）与
   * 触发后保留几轮。改小 = 压得早、上下文丢得多；改大 = 上下文连续、
   * 单次请求驮得多。保存即活，**不必重启**——流水线每个 attempt、值班长
   * 每轮都重读库里那一行，下一轮就按新值走。
   *
   * `conversation_max_chars`（会话落库截断，缺省 20 万字符）**不在本页**：
   * 它管「一条会话行留多少痕」，不管模型上下文（票 02 的分家裁决），
   * 改它走 config.toml。
   *
   * 交互姿态照 `/settings/foreman`：每一步成功失败都以**重读到的读数**为准。
   */

  let settings = $state<CompactionSettings | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let saving = $state(false);
  let note = $state<{ kind: 'ok' | 'bad'; message: string } | null>(null);
  let tokensInput = $state('');
  let roundsInput = $state('');

  async function load() {
    loading = true;
    error = null;
    try {
      settings = await getCompaction();
      tokensInput = String(settings.conversation_max_tokens);
      roundsInput = String(settings.keep_recent_rounds);
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void load();
  });

  async function save(e: SubmitEvent) {
    e.preventDefault();
    const tokens = Number(tokensInput);
    const rounds = Number(roundsInput);
    if (!Number.isInteger(tokens) || tokens <= 0 || !Number.isInteger(rounds) || rounds <= 0) {
      note = { kind: 'bad', message: '两个值都必须是正整数。' };
      return;
    }
    saving = true;
    note = null;
    try {
      await setCompaction(tokens, rounds);
      await load();
      note = { kind: 'ok', message: '已保存：下一轮 attempt / 值守就按新值走，不必重启。' };
    } catch (err) {
      await load();
      note = { kind: 'bad', message: (err as Error).message };
    } finally {
      saving = false;
    }
  }

  function originLabel(origin: 'default' | 'settings'): string {
    return origin === 'settings' ? '界面保存的' : 'config 值';
  }
</script>

<main class="page">
  <a class="crumb" href="#/settings">← 设置</a>
  <div class="p-head">
    <h1 class="p-title">设置 · 管线压缩</h1>
  </div>

  <p class="hintline">
    节点干活时上下文越滚越大，超线就按轮压缩（丢掉老历史、留摘要）。
    这里决定<b>多长才压、压后留几轮</b>；改小省 token、丢上下文，改大上下文连续、单次请求更贵。
  </p>

  {#if error}
    <div class="banner error" role="alert">{error}</div>
    <div class="retry">
      <button type="button" class="btn" disabled={loading} onclick={() => void load()}>重试</button>
    </div>
  {:else if loading}
    <div class="banner">正在加载压缩设置…</div>
  {:else if settings}
    <form class="block" aria-labelledby="floor-head" onsubmit={save}>
      <h2 class="sec-title" id="floor-head">触发线</h2>
      <div class="field">
        <label class="lab" for="tokens-input">
          token 硬底（当前 {settings.conversation_max_tokens}，{originLabel(settings.conversation_max_tokens_origin)}）
        </label>
        <input
          id="tokens-input"
          class="input mono"
          type="number"
          min="1"
          step="1"
          required
          bind:value={tokensInput}
        />
        <p class="sec-note">
          转录的 token 估算超过这个数就强制压缩，不看模型窗口登记的脸色。缺省 300000——
          审计级走查（单次请求最大约 9 万）够不着这条线，等于不压。
        </p>
      </div>
      <div class="field">
        <label class="lab" for="rounds-input">
          压缩保留轮数（当前 {settings.keep_recent_rounds}，{originLabel(settings.keep_recent_rounds_origin)}）
        </label>
        <input
          id="rounds-input"
          class="input mono"
          type="number"
          min="1"
          step="1"
          required
          bind:value={roundsInput}
        />
        <p class="sec-note">压缩发生时，最近几轮原样保留，更老的压成摘要。缺省 5。</p>
      </div>
      <button type="submit" class="btn solid" disabled={saving}>
        {#if saving}<span class="spin"></span>{/if}保存
      </button>
    </form>

    <p class="sec-note">
      config 层的值：token 硬底 {settings.config_conversation_max_tokens}、保留轮数
      {settings.config_keep_recent_rounds}（config.toml 的 <span class="mono">[pipeline]</span> 段）。
      另有一个 <span class="mono">conversation_max_chars</span>（缺省 200000）管「一条会话行落库留多少痕」，
      不在本页——它不管模型上下文。
    </p>

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
    display: flex;
    flex-direction: column;
    gap: 14px;
    align-items: flex-start;
  }
  .sec-title {
    font-size: 12px;
    color: var(--text-hi);
    margin-bottom: 2px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    width: 100%;
    max-width: 480px;
  }
  .lab {
    color: var(--text);
    font-size: 12px;
  }
  .input {
    border: 2px solid var(--pane);
    background: var(--bg);
    color: var(--text-hi);
    padding: 6px 10px;
    font-size: 12px;
  }
  .input:focus {
    outline: none;
    border-color: var(--text-3);
  }
  .sec-note {
    color: var(--text-3);
    line-height: 1.8;
    max-width: 86ch;
    margin: 0;
    font-size: 12px;
  }
</style>
