<script lang="ts">
  import type { Provider } from '../../api/types';
  import Modal from '../ui/Modal.svelte';

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

  function submit(e: SubmitEvent) {
    e.preventDefault();
    if (providerId) onsubmit(providerId);
  }
</script>

<Modal
  {open}
  width={460}
  title="更换长上下文模型"
  submitLabel="应用"
  {submitting}
  submitDisabled={!providerId}
  {onclose}
  onsubmit={submit}
>
  <!-- 正文只说影响范围（决策 199：编号退场，只留动作与后果）。 -->
  <div class="hint">只影响本任务后续节点，不改全局阶段配置。</div>
  {#if providers.length === 0}
    <div class="error" role="alert">还没有配置 provider，请先到「设置 · 模型与密钥」添加。</div>
  {:else}
    <select class="input" bind:value={providerId}>
      {#each providers as p (p.id)}
        <option value={p.id} disabled={!p.enabled}>
          {p.vendor} · {p.model} · {p.context_window} ctx
        </option>
      {/each}
    </select>
  {/if}
  {#if error}<div class="error" role="alert">{error}</div>{/if}
</Modal>

<style>
  .hint {
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin-top: 6px;
  }
</style>
