<script lang="ts">
  import { untrack } from 'svelte';
  import type { Provider, StageConfig } from '../../api/types';
  import {
    STAGE_KEYS,
    draftFromStageConfig,
    emptyStageConfigDraft,
    stageKeyLabel,
    type StageConfigDraft,
  } from '../../lib/stageConfigs';

  /**
   * stage_configs 编辑表单（决策 22 / 46 / 66 / 111 / 129）。
   * 保存是**整条替换**（PUT）：留空的字段会被清空为默认，不是保持原值——UI 明确标注。
   * 父组件用 {#key} 重挂载，故这里用 props 初始化一次 $state 即可。
   */
  interface Props {
    config: StageConfig | null;
    providers: Provider[];
    submitting: boolean;
    error: string | null;
    onsubmit: (draft: StageConfigDraft) => void;
    oncancel: () => void;
  }
  let { config, providers, submitting, error, onsubmit, oncancel }: Props = $props();

  const isNew = untrack(() => config === null);
  let draft = $state<StageConfigDraft>(
    untrack(() => (config ? draftFromStageConfig(config) : emptyStageConfigDraft())),
  );
  let localError = $state<string | null>(null);
  const shownError = $derived(error ?? localError);

  function submit(e: SubmitEvent) {
    e.preventDefault();
    localError = null;
    onsubmit(draft);
  }
</script>

<form class="sc-form panel" onsubmit={submit}>
  <div class="form-head cond">
    {isNew ? '新增阶段配置' : `编辑阶段配置 · ${config?.stage}`}
  </div>

  <label class="field">
    <span>阶段 / 伪阶段键</span>
    {#if isNew}
      <select class="input mono" bind:value={draft.stage}>
        {#each STAGE_KEYS as key (key)}
          <option value={key}>{stageKeyLabel(key)}</option>
        {/each}
      </select>
    {:else}
      <input class="input mono" value={config?.stage ?? ''} readonly />
    {/if}
  </label>

  <p class="replace-note">
    保存为<strong>整条替换</strong>：留空的字段会被清空为默认值（不是保持原值）。要保留某项，请确认它已填写。
  </p>

  <div class="grid">
    <label class="field wide">
      <span>provider_id（留空 = 使用系统默认；填写须存在、enabled 且厂商受支持）</span>
      <input
        class="input mono"
        list="stage-provider-ids"
        bind:value={draft.provider_id}
        placeholder="provider ID"
      />
      <datalist id="stage-provider-ids">
        {#each providers as p (p.id)}
          <option value={p.id}>{p.vendor} · {p.model}</option>
        {/each}
      </datalist>
    </label>

    <label class="field">
      <span>temperature</span>
      <input class="input mono" type="number" step="0.1" bind:value={draft.temperature} />
    </label>

    <label class="field">
      <span>max_tokens</span>
      <input class="input mono" type="number" min="1" bind:value={draft.max_tokens} />
    </label>

    <label class="field wide">
      <span>persona_path（相对 / 绝对路径，须可读且非空）</span>
      <input
        class="input mono"
        bind:value={draft.persona_path}
        placeholder="prompts/develop/execute.md"
      />
    </label>

    <label class="field wide">
      <span>persona_append（追加指令，可选）</span>
      <textarea class="input" rows="2" bind:value={draft.persona_append}></textarea>
    </label>

    <label class="field">
      <span>tools_json</span>
      <textarea
        class="input mono json"
        rows="4"
        bind:value={draft.tools_json}
        placeholder='&#123;"execute": ["read_file"]&#125; 或 ["read_file"]'
      ></textarea>
    </label>

    <label class="field">
      <span>skills_json</span>
      <textarea
        class="input mono json"
        rows="4"
        bind:value={draft.skills_json}
        placeholder='["rtk"]'
      ></textarea>
    </label>

    <label class="field">
      <span>idle_timeout_sec</span>
      <input class="input mono" type="number" min="0" bind:value={draft.idle_timeout_sec} />
    </label>

    <label class="field">
      <span>max_duration_sec</span>
      <input class="input mono" type="number" min="0" bind:value={draft.max_duration_sec} />
    </label>

    <label class="field wide">
      <span>node_overrides_json</span>
      <textarea
        class="input mono json"
        rows="4"
        bind:value={draft.node_overrides_json}
        placeholder='&#123;"execute": &#123;"idle_timeout_sec": 600&#125;&#125;'
      ></textarea>
    </label>
  </div>

  {#if shownError}<div class="error">{shownError}</div>{/if}

  <div class="actions">
    <button type="button" class="btn quiet" disabled={submitting} onclick={oncancel}>取消</button>
    <button type="submit" class="btn solid" disabled={submitting}>
      {#if submitting}<span class="spin"></span>{/if}
      {isNew ? '创建' : '整条替换'}
    </button>
  </div>
</form>

<style>
  .sc-form {
    padding: 14px 16px;
  }
  .form-head {
    font-size: 13px;
    color: var(--text-hi);
    margin-bottom: 10px;
  }
  .replace-note {
    font-size: 11.5px;
    color: var(--pending);
    line-height: 1.6;
    margin-bottom: 10px;
    padding: 7px 10px;
    border: 1px solid var(--pending);
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
    font-size: 11px;
    color: var(--text-3);
    margin-bottom: 4px;
  }
  textarea.json {
    min-height: 76px;
  }
  .input[readonly] {
    opacity: 0.6;
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin-top: 8px;
    white-space: pre-wrap;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
</style>
