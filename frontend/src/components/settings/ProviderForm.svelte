<script lang="ts">
  import { untrack } from 'svelte';
  import type { ConnectionTestResult, Provider } from '../../api/types';
  import { testProvider } from '../../api/client';
  import {
    API_KEY_MASK,
    SUPPORTED_ADAPTERS,
    buildProviderTest,
    draftFromProvider,
    emptyProviderDraft,
    isSupportedAdapter,
    type ProviderDraft,
  } from '../../lib/providers';

  /**
   * provider 新增 / 编辑表单（决策 103 / 111 / 112）。
   * 父组件用 `{#key}` 重挂载，因此这里用 props 初始化一次 $state 即可。
   */
  interface Props {
    provider: Provider | null;
    submitting: boolean;
    error: string | null;
    onsubmit: (draft: ProviderDraft) => void;
    oncancel: () => void;
  }
  let { provider, submitting, error, onsubmit, oncancel }: Props = $props();

  const isNew = untrack(() => provider === null);
  // 只取初始值：父组件以 {#key} 重挂载表单，provider 变更即重新初始化。
  let draft = $state<ProviderDraft>(
    untrack(() => (provider ? draftFromProvider(provider) : emptyProviderDraft())),
  );

  const supported = $derived(isSupportedAdapter(draft.vendor.trim()));
  const keyPlaceholder = $derived(
    isNew ? 'sk-…（可留空，本机代理可匿名）' : `已保存；保持 ${API_KEY_MASK} 即不修改`,
  );

  let localError = $state<string | null>(null);
  const shownError = $derived(error ?? localError);

  // 「测试连接」（决策 160）：建任务前验证密钥 / 模型 / 地址，不必保存草稿
  let testing = $state(false);
  let testResult = $state<ConnectionTestResult | null>(null);
  let testError = $state<string | null>(null);

  async function runTest() {
    localError = null;
    testResult = null;
    testError = null;
    testing = true;
    try {
      testResult = await testProvider(buildProviderTest(draft, provider?.id ?? null));
    } catch (err) {
      testError = (err as Error).message;
    } finally {
      testing = false;
    }
  }

  function submit(e: SubmitEvent) {
    e.preventDefault();
    localError = null;
    onsubmit(draft);
  }
</script>

<form class="prov-form panel" onsubmit={submit}>
  <div class="form-head cond">{isNew ? '新增 provider' : `编辑 provider · ${provider?.id}`}</div>

  <div class="grid">
    <label class="field">
      <span>厂商 vendor</span>
      <input class="input mono" list="supported-adapters" bind:value={draft.vendor} />
      <datalist id="supported-adapters">
        {#each SUPPORTED_ADAPTERS as a (a)}<option value={a}></option>{/each}
      </datalist>
    </label>

    <label class="field">
      <span>模型 model</span>
      <input class="input mono" bind:value={draft.model} placeholder="gpt-4o / claude-… / deepseek-chat" />
    </label>

    <label class="field">
      <span>上下文窗口 context_window</span>
      <input
        class="input mono"
        type="number"
        min="1"
        bind:value={draft.context_window}
      />
    </label>

    <label class="field">
      <span>base_url（可选）</span>
      <input class="input mono" bind:value={draft.base_url} placeholder="留空使用官方默认" />
    </label>

    <label class="field wide">
      <span>api_key</span>
      <input class="input mono" bind:value={draft.api_key} placeholder={keyPlaceholder} autocomplete="off" />
    </label>
  </div>

  {#if !supported && draft.vendor.trim()}
    <div class="warn">
      厂商 <span class="mono">{draft.vendor.trim()}</span> 不在 supported_adapters
      （{SUPPORTED_ADAPTERS.join(' / ')}）内：该行会降级灰显，被 stage_configs 引用时配置加载会拒绝启动（决策 103）。
    </div>
  {/if}

  <label class="enabled">
    <input type="checkbox" bind:checked={draft.enabled} />
    启用（enabled）
  </label>

  <p class="hint">
    密钥明文存于本机 <span class="mono">~/.agentpipeline</span>，目录权限
    <span class="mono">0700</span>。保存时未改动则不会回传掩码。
  </p>

  {#if shownError}<div class="error">{shownError}</div>{/if}

  {#if testError}<div class="error">测试连接失败：{testError}</div>{/if}
  {#if testResult}
    <div class="test-result" class:ok={testResult.ok} class:fail={!testResult.ok}>
      <b>{testResult.ok ? '✓' : '✗'} {testResult.message}</b>
      <span class="mono dim">（{testResult.latency_ms}ms）</span>
      {#if testResult.raw}
        <div class="mono dim">诊断：{testResult.raw}</div>
      {/if}
    </div>
  {/if}
  <div class="actions">
    <button
      type="button"
      class="btn quiet"
      disabled={testing || submitting || !supported || !draft.model.trim()}
      onclick={runTest}
    >
      {#if testing}<span class="spin"></span>{/if}
      测试连接
    </button>
    <button type="button" class="btn quiet" disabled={submitting} onclick={oncancel}>取消</button>
    <button type="submit" class="btn solid" disabled={submitting}>
      {#if submitting}<span class="spin"></span>{/if}
      {isNew ? '创建' : '保存'}
    </button>
  </div>
</form>

<style>
  .prov-form {
    padding: 14px 16px;
    margin-bottom: 14px;
  }
  .form-head {
    font-size: 12px;
    letter-spacing: 0.08em;
    color: var(--text-hi);
    margin-bottom: 12px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px 14px;
  }
  .field {
    display: block;
  }
  .field.wide {
    grid-column: 1 / -1;
  }
  .field > span {
    display: block;
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 4px;
  }
  .warn {
    margin-top: 10px;
    padding: 7px 10px;
    border: 2px solid var(--pending);
    color: var(--pending);
    font-size: 12px;
    line-height: 1.6;
  }
  .enabled {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 10px;
    font-size: 12px;
    color: var(--text-2);
  }
  .hint {
    margin-top: 8px;
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.6;
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin-top: 8px;
  }
  /* 「测试连接」结果行（决策 160）：绿 = 通，红 = 不通，琥珀 = 未定 */
  .test-result {
    margin-top: 8px;
    padding: 7px 10px;
    font-size: 12px;
    line-height: 1.6;
    border: 2px solid var(--pending);
  }
  .test-result.ok {
    border-color: var(--go);
  }
  .test-result.fail {
    border-color: var(--stop);
  }
  .test-result .dim {
    color: var(--text-4);
    font-size: 12px;
    word-break: break-all;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }

  @media (max-width: 479px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
