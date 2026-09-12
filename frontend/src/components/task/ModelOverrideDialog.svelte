<script lang="ts">
  import type { Provider } from '../../api/types';

  interface Props {
    open: boolean;
    providers: Provider[];
    submitting?: boolean;
    error?: string | null;
    onclose: () => void;
    onsubmit: (providerId: string) => void;
  }
  let { open, providers, submitting = false, error = null, onclose, onsubmit }: Props = $props();

  let providerId = $state('');

  $effect(() => {
    if (open && !providerId) providerId = providers.find((p) => p.enabled)?.id ?? providers[0]?.id ?? '';
  });
</script>

{#if open}
  <div
    class="overlay"
    role="presentation"
    onclick={(e) => {
      if (e.target === e.currentTarget) onclose();
    }}
    onkeydown={(e) => e.key === 'Escape' && onclose()}
  >
    <form
      class="dialog panel"
      onsubmit={(e) => {
        e.preventDefault();
        if (providerId) onsubmit(providerId);
      }}
    >
      <div class="head cond">更换长上下文模型</div>
      <div class="hint">仅影响本任务后续节点（决策 105 / 129），不改全局 stage config。</div>
      {#if providers.length === 0}
        <div class="error">还没有配置 provider，请先到「设置 · 模型与密钥」添加。</div>
      {:else}
        <select class="input" bind:value={providerId}>
          {#each providers as p (p.id)}
            <option value={p.id} disabled={!p.enabled}>
              {p.vendor} · {p.model} · {p.context_window} ctx
            </option>
          {/each}
        </select>
      {/if}
      {#if error}<div class="error">{error}</div>{/if}
      <div class="actions">
        <button type="button" class="btn quiet" onclick={onclose}>取消</button>
        <button type="submit" class="btn solid" disabled={submitting || !providerId}>
          {#if submitting}<span class="spin"></span>{/if}
          应用
        </button>
      </div>
    </form>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: rgba(6, 10, 16, 0.62);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .dialog {
    width: 460px;
    max-width: calc(100vw - 32px);
    padding: 18px 20px;
  }
  .head {
    font-size: 15px;
    margin-bottom: 8px;
  }
  .hint {
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .error {
    color: var(--signal-stop);
    font-size: 12px;
    margin-top: 6px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
</style>
