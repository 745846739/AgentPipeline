<script lang="ts">
  import { onMount } from 'svelte';
  import {
    createProvider,
    deleteProvider,
    deleteStageConfig,
    listProviders,
    listStageConfigs,
    putStageConfig,
    updateProvider,
  } from '../api/client';
  import type { Provider, StageConfig } from '../api/types';
  import ProviderForm from '../components/settings/ProviderForm.svelte';
  import StageConfigForm from '../components/settings/StageConfigForm.svelte';
  import {
    API_KEY_MASK,
    buildProviderCreate,
    buildProviderPatch,
    isSupportedAdapter,
    validateProviderDraft,
    type ProviderDraft,
  } from '../lib/providers';
  import {
    buildStageConfigPut,
    isPseudoStage,
    stageKeyLabel,
    type StageConfigDraft,
  } from '../lib/stageConfigs';
  import { formatDateTime } from '../lib/format';

  let providers = $state<Provider[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  type Editing = { mode: 'new' } | { mode: 'edit'; provider: Provider };
  let editing = $state<Editing | null>(null);
  let saving = $state(false);
  let formError = $state<string | null>(null);

  let confirmingDelete = $state<string | null>(null);
  let deleteBusy = $state<string | null>(null);
  let rowError = $state<{ id: string; message: string } | null>(null);

  /* ── stage_configs（决策 22 / 46 / 66 / 111 / 129）── */
  let stageConfigs = $state<StageConfig[]>([]);
  let scLoading = $state(true);
  let scError = $state<string | null>(null);
  type ScEditing = { mode: 'new' } | { mode: 'edit'; config: StageConfig };
  let scEditing = $state<ScEditing | null>(null);
  let scSaving = $state(false);
  let scFormError = $state<string | null>(null);
  let scConfirmingDelete = $state<string | null>(null);
  let scDeleteBusy = $state<string | null>(null);
  let scRowError = $state<{ stage: string; message: string } | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      providers = await listProviders();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  async function loadStageConfigs() {
    scLoading = true;
    scError = null;
    try {
      stageConfigs = await listStageConfigs();
    } catch (err) {
      scError = (err as Error).message;
    } finally {
      scLoading = false;
    }
  }

  onMount(() => {
    void load();
    void loadStageConfigs();
  });

  function openNew() {
    formError = null;
    editing = { mode: 'new' };
  }

  function openEdit(provider: Provider) {
    formError = null;
    editing = { mode: 'edit', provider };
  }

  async function submit(draft: ProviderDraft) {
    const invalid = validateProviderDraft(draft);
    if (invalid) {
      formError = invalid;
      return;
    }
    if (!editing) return;
    saving = true;
    formError = null;
    try {
      if (editing.mode === 'new') {
        await createProvider(buildProviderCreate(draft));
      } else {
        await updateProvider(editing.provider.id, buildProviderPatch(editing.provider, draft));
      }
      editing = null;
      await load();
    } catch (err) {
      formError = (err as Error).message;
    } finally {
      saving = false;
    }
  }

  async function remove(provider: Provider) {
    deleteBusy = provider.id;
    rowError = null;
    try {
      await deleteProvider(provider.id);
      confirmingDelete = null;
      await load();
    } catch (err) {
      rowError = { id: provider.id, message: (err as Error).message };
    } finally {
      deleteBusy = null;
    }
  }

  /* ── stage_configs handlers ── */

  function openNewStageConfig() {
    scFormError = null;
    scEditing = { mode: 'new' };
  }

  function openEditStageConfig(config: StageConfig) {
    scFormError = null;
    scEditing = { mode: 'edit', config };
  }

  async function submitStageConfig(draft: StageConfigDraft) {
    if (!scEditing) return;
    const built = buildStageConfigPut(draft);
    if (!built.ok) {
      scFormError = built.error;
      return;
    }
    scSaving = true;
    scFormError = null;
    try {
      await putStageConfig(draft.stage, built.payload);
      scEditing = null;
      await loadStageConfigs();
    } catch (err) {
      // 400 { error }（provider 缺失/禁用/vendor 不支持、persona 不可读、会破坏启动的改动）原样回显
      scFormError = (err as Error).message;
    } finally {
      scSaving = false;
    }
  }

  async function removeStageConfig(config: StageConfig) {
    scDeleteBusy = config.stage;
    scRowError = null;
    try {
      await deleteStageConfig(config.stage);
      scConfirmingDelete = null;
      await loadStageConfigs();
    } catch (err) {
      // 404（无配置）/ 400（删除会破坏启动校验）都回显后端原因
      scRowError = { stage: config.stage, message: (err as Error).message };
      scConfirmingDelete = null;
    } finally {
      scDeleteBusy = null;
    }
  }
</script>

<div class="page">
  <a class="crumb" href="#/">← 看板</a>
  <header class="head">
    <h1 class="cond">设置 · 模型与密钥</h1>
    <button type="button" class="btn solid" onclick={openNew}>＋ 新增 provider</button>
  </header>

  <p class="hint">
    provider 行 =（vendor, model, context_window）（决策 111）。api_key 明文存储，读接口只回显
    <span class="mono">{API_KEY_MASK}</span>；密钥明文存于本机
    <span class="mono">~/.agentpipeline</span>，目录权限 <span class="mono">0700</span>（决策 112 / §12.14）。
  </p>

  {#if editing}
    {#key editing.mode === 'new' ? 'new' : editing.provider.id}
      <ProviderForm
        provider={editing.mode === 'edit' ? editing.provider : null}
        submitting={saving}
        error={formError}
        onsubmit={submit}
        oncancel={() => (editing = null)}
      />
    {/key}
  {/if}

  {#if error}
    <div class="banner error">{error}</div>
  {:else if loading}
    <div class="banner">正在加载 provider…</div>
  {:else if providers.length === 0}
    <div class="banner">还没有 provider。新增一行后，任务的阶段模型才会被解析。</div>
  {:else}
    <ul class="rows">
      {#each providers as p (p.id)}
        {@const supported = isSupportedAdapter(p.vendor)}
        <li class="row panel" class:unsupported={!supported}>
          <div class="main">
            <div class="line1">
              <span class="vendor mono">{p.vendor}</span>
              <span class="model mono">{p.model}</span>
              <span class="status {p.enabled ? 'on' : 'off'}">
                {p.enabled ? 'enabled' : 'disabled'}
              </span>
              {#if !supported}<span class="warn-chip">不受支持 · 决策 103</span>{/if}
            </div>
            <div class="line2 mono">
              <span>ctx {p.context_window.toLocaleString('en-US')}</span>
              <span>base_url {p.base_url ?? '默认'}</span>
              <span>api_key {p.api_key ? API_KEY_MASK : '未设置'}</span>
            </div>
            {#if !supported}
              <div class="warn-text">
                该厂商不在 supported_adapters 内：此行走降级灰显，被 stage_configs 引用时配置加载会拒绝启动。
              </div>
            {/if}
            {#if rowError?.id === p.id}<div class="warn-text stop">{rowError.message}</div>{/if}
          </div>
          <div class="acts">
            {#if confirmingDelete === p.id}
              <span class="confirm">确认删除？</span>
              <button
                type="button"
                class="btn danger"
                disabled={deleteBusy === p.id}
                onclick={() => remove(p)}
              >
                {#if deleteBusy === p.id}<span class="spin"></span>{/if}删除
              </button>
              <button type="button" class="btn quiet" onclick={() => (confirmingDelete = null)}>
                取消
              </button>
            {:else}
              <button type="button" class="btn" onclick={() => openEdit(p)}>编辑</button>
              <button
                type="button"
                class="btn danger"
                onclick={() => {
                  rowError = null;
                  confirmingDelete = p.id;
                }}
              >
                删除
              </button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}

  <!-- stage_configs 编辑器（决策 22 / 46 / 66 / 111 / 129）：GET 列表 / PUT 整条替换 / DELETE 撤销覆盖。 -->
  <section class="stage-configs">
    <header class="sc-head">
      <h2 class="cond">阶段配置（stage_configs）</h2>
      <button type="button" class="btn" onclick={openNewStageConfig}>＋ 新增阶段配置</button>
    </header>
    <p class="hint sc-hint">
      阶段 provider 优先于全局默认（决策 129）。保存为<strong>整条替换</strong>：留空字段清空为默认。写入会跑启动校验，非法配置（provider 缺失/禁用/厂商不支持、persona 不可读、会破坏启动的改动）被拒并回显原因（决策 47 / 103）。
    </p>

    {#if scEditing}
      {#key scEditing.mode === 'new' ? 'sc-new' : scEditing.config.stage}
        <StageConfigForm
          config={scEditing.mode === 'edit' ? scEditing.config : null}
          {providers}
          submitting={scSaving}
          error={scFormError}
          onsubmit={submitStageConfig}
          oncancel={() => (scEditing = null)}
        />
      {/key}
    {/if}

    {#if scError}
      <div class="banner error">{scError}</div>
    {:else if scLoading}
      <div class="banner">正在加载阶段配置…</div>
    {:else if stageConfigs.length === 0}
      <div class="banner">还没有阶段覆盖。所有阶段都在用系统默认配置。</div>
    {:else}
      <ul class="rows">
        {#each stageConfigs as sc (sc.stage)}
          <li class="row panel" class:pseudo={isPseudoStage(sc.stage)}>
            <div class="main">
              <div class="line1">
                <span class="vendor mono">{stageKeyLabel(sc.stage)}</span>
                <span class="model mono">provider {sc.provider_id ?? '默认'}</span>
                <span class="model mono">
                  temp {sc.temperature ?? '默认'} · max_tokens {sc.max_tokens ?? '默认'}
                </span>
              </div>
              <div class="line2 mono">
                <span>persona {sc.persona_path ?? '—'}</span>
                <span>tools {sc.tools_json ? '已配置' : '—'}</span>
                <span>skills {sc.skills_json ? '已配置' : '—'}</span>
                <span>updated {formatDateTime(sc.updated_at)}</span>
              </div>
              {#if scRowError?.stage === sc.stage}
                <div class="warn-text stop">{scRowError.message}</div>
              {/if}
            </div>
            <div class="acts">
              {#if scConfirmingDelete === sc.stage}
                <span class="confirm">确认撤销覆盖？</span>
                <button
                  type="button"
                  class="btn danger"
                  disabled={scDeleteBusy === sc.stage}
                  onclick={() => removeStageConfig(sc)}
                >
                  {#if scDeleteBusy === sc.stage}<span class="spin"></span>{/if}删除
                </button>
                <button type="button" class="btn quiet" onclick={() => (scConfirmingDelete = null)}>
                  取消
                </button>
              {:else}
                <button type="button" class="btn" onclick={() => openEditStageConfig(sc)}>编辑</button>
                <button
                  type="button"
                  class="btn danger"
                  onclick={() => {
                    scRowError = null;
                    scConfirmingDelete = sc.stage;
                  }}
                >
                  删除
                </button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</div>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 20px 24px 60px;
  }
  .crumb {
    display: inline-flex;
    color: var(--text-3);
    font-size: 12px;
    margin-bottom: 10px;
  }
  .crumb:hover {
    color: var(--text-2);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    margin-bottom: 8px;
  }
  h1 {
    font-size: 18px;
    color: var(--text-hi);
  }
  .hint {
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.6;
    margin-bottom: 14px;
  }
  .banner {
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--r-panel);
    color: var(--text-3);
    font-size: 12px;
  }
  .banner.error {
    border-color: var(--signal-stop);
    color: var(--signal-stop);
  }
  .rows {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 4px;
  }
  .row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
  }
  .row.unsupported {
    opacity: 0.72;
    border-color: var(--signal-caution);
  }
  .main {
    min-width: 0;
  }
  .line1 {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .vendor {
    color: var(--text-hi);
    font-size: 12.5px;
  }
  .model {
    color: var(--text-2);
    font-size: 12px;
  }
  .status {
    font-family: var(--font-mono);
    font-size: 10.5px;
  }
  .status.on {
    color: var(--signal-go);
  }
  .status.off {
    color: var(--text-3);
  }
  .warn-chip {
    font-size: 10.5px;
    color: var(--signal-caution);
    border: 1px solid var(--signal-caution);
    border-radius: var(--r-pill);
    padding: 0 5px;
  }
  .line2 {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-3);
  }
  .warn-text {
    margin-top: 5px;
    font-size: 11.5px;
    color: var(--signal-caution);
  }
  .warn-text.stop {
    color: var(--signal-stop);
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .confirm {
    font-size: 11.5px;
    color: var(--text-2);
  }
  .stage-configs {
    margin-top: 26px;
  }
  .sc-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    margin-bottom: 6px;
  }
  .sc-head h2 {
    font-size: 14px;
    color: var(--text-hi);
  }
  .sc-hint {
    margin-bottom: 12px;
  }
  .row.pseudo {
    border-left: 2px solid var(--branch-test);
  }
</style>
