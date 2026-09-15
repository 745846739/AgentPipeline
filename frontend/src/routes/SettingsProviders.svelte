<script lang="ts">
  import { onMount } from 'svelte';
  import {
    createProvider,
    deleteProvider,
    deleteStageConfig,
    installSkillForStage,
    listProviders,
    listRecommendedSkills,
    listSkills,
    listStageConfigs,
    putStageConfig,
    updateProvider,
  } from '../api/client';
  import type {
    Provider,
    RecommendedStage,
    SkillPreview,
    SkillSummary,
    StageConfig,
  } from '../api/types';
  import ProviderForm from '../components/settings/ProviderForm.svelte';
  import StageConfigForm from '../components/settings/StageConfigForm.svelte';
  import StageRecommendations from '../components/settings/StageRecommendations.svelte';
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

  /* ── 技能目录与推荐（决策 172①④，票 15 / 16）── */
  let skills = $state<SkillSummary[]>([]);
  let recommendations = $state<RecommendedStage[]>([]);
  /** 正在安装的 `阶段:技能名`（按钮上的转圈与禁用）。 */
  let installing = $state<string | null>(null);
  let skillError = $state<string | null>(null);
  /** 最近一次一键安装带回来的三项预览（票 11）。 */
  let installPreview = $state<SkillPreview | null>(null);

  async function loadSkills() {
    try {
      skills = await listSkills();
    } catch (err) {
      // 目录取不到不该挡住整页：技能控件退化为「手动输入技能名」
      skillError = (err as Error).message;
    }
  }

  async function loadRecommendations() {
    try {
      recommendations = await listRecommendedSkills();
    } catch (err) {
      // 推荐清单是锦上添花，取不到就整块不显示（票 16：技能不存在时界面降级）
      skillError = (err as Error).message;
      recommendations = [];
    }
  }

  /**
   * 一键安装：装技能 + 写该阶段配置一步完成（票 16）。
   *
   * 失败原因由后端分类给出（技能不存在 / 摘要不符 / 来源未放行 / 网络失败），原样回显。
   * 成功时把 `preview` 交给推荐面板——特征命中当场可见，这是「不绕过票 11 预览」的落点。
   */
  async function installRecommended(stage: string, name: string) {
    installing = `${stage}:${name}`;
    skillError = null;
    try {
      const result = await installSkillForStage(stage, name);
      installPreview = result.preview;
      await Promise.all([loadSkills(), loadRecommendations(), loadStageConfigs()]);
    } catch (err) {
      skillError = (err as Error).message;
    } finally {
      installing = null;
    }
  }

  /** 每行的掩码状态：密钥已配置显示 `***`，未配置显示「未设置」（决策 112）。 */
  function keyText(p: Provider): string {
    return p.api_key ? API_KEY_MASK : '未设置';
  }

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
    void loadSkills();
    void loadRecommendations();
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
  <div class="p-head">
    <h1 class="p-title">设置 · 模型与密钥</h1>
    <button type="button" class="btn solid" onclick={openNew}>＋ 新增 provider</button>
  </div>

  <p class="hintline">
    provider 行 =（vendor, model, context_window）（决策 111）。api_key 明文存储，读接口只回显 <b>{API_KEY_MASK}</b>；
    密钥明文存于本机 <b>~/.agentpipeline</b>，目录权限 <b>0700</b>（决策 112 / §12.14）。
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
    <div class="reg">
      <div class="reg-head">
        <span>provider</span>
        <span class="n">▪ {providers.length}</span>
      </div>
      <ul class="reg-rows">
        {#each providers as p (p.id)}
          {@const supported = isSupportedAdapter(p.vendor)}
          <li class="reg-row row" class:dead={!supported}>
            <div class="reg-main">
              <div class="reg-l1">
                <span class="reg-name mono">{p.vendor}</span>
                <span class="reg-sub mono">{p.model}</span>
                {#if p.enabled}
                  <span class="st run">[ON]</span>
                {:else}
                  <span class="st dim">[OFF]</span>
                {/if}
                {#if !supported}<span class="warnnote inline">! 不受支持 · 决策 103</span>{/if}
              </div>
              <div class="reg-l2 mono">
                <span>ctx {p.context_window.toLocaleString('en-US')}</span>
                <span>base_url {p.base_url ?? '默认'}</span>
                <span>api_key {keyText(p)}</span>
              </div>
              {#if !supported}
                <div class="warnnote">
                  该厂商不在 supported_adapters 内：此行走降级灰显，被 stage_configs 引用时配置加载会拒绝启动。
                </div>
              {/if}
              {#if rowError?.id === p.id}<div class="reg-err">{rowError.message}</div>{/if}
            </div>
            <div class="reg-acts">
              {#if confirmingDelete === p.id}
                <span class="reg-sub">确认删除？</span>
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
    </div>
  {/if}

  <!-- 推荐技能与一键安装（决策 172①，票 16）：清单来自内置常量，装进来的技能默认未受信任。 -->
  <StageRecommendations
    stages={recommendations}
    busy={installing}
    preview={installPreview}
    oninstall={installRecommended}
  />
  {#if skillError}<div class="error skills-error">{skillError}</div>{/if}

  <!-- stage_configs 编辑器（决策 22 / 46 / 66 / 111 / 129）：GET 列表 / PUT 整条替换 / DELETE 撤销覆盖。 -->
  <div class="sub-head">
    <h2>阶段配置</h2>
    <button type="button" class="btn" onclick={openNewStageConfig}>＋ 新增阶段配置</button>
  </div>
  <p class="hintline">
    阶段 provider 优先于全局默认（决策 129）。保存为<b>整条替换</b>：留空字段清空为默认。写入会跑启动校验，非法配置（provider 缺失/禁用/厂商不支持、persona 不可读、会破坏启动的改动）被拒并回显原因（决策 47 / 103）。
  </p>

  {#if scEditing}
    {#key scEditing.mode === 'new' ? 'sc-new' : scEditing.config.stage}
      <StageConfigForm
        config={scEditing.mode === 'edit' ? scEditing.config : null}
        {providers}
        {skills}
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
    <div class="reg">
      <div class="reg-head">
        <span>阶段配置</span>
        <span class="n">▪ {stageConfigs.length}</span>
      </div>
      <ul class="reg-rows">
        {#each stageConfigs as sc (sc.stage)}
          <li class="reg-row row" class:pseudo={isPseudoStage(sc.stage)}>
            <div class="reg-main">
              <div class="reg-l1">
                <span class="reg-name mono">{stageKeyLabel(sc.stage)}</span>
                <span class="reg-sub mono">provider {sc.provider_id ?? '默认'}</span>
                <span class="reg-sub mono">
                  temp {sc.temperature ?? '默认'} · max_tokens {sc.max_tokens ?? '默认'}
                </span>
              </div>
              <div class="reg-l2 mono">
                <span>persona {sc.persona_path ?? '—'}</span>
                <span>tools {sc.tools_json ? '已配置' : '—'}</span>
                <span>skills {sc.skills_json ? '已配置' : '—'}</span>
                <span>updated {formatDateTime(sc.updated_at)}</span>
              </div>
              {#if scRowError?.stage === sc.stage}
                <div class="reg-err">{scRowError.message}</div>
              {/if}
            </div>
            <div class="reg-acts">
              {#if scConfirmingDelete === sc.stage}
                <span class="reg-sub">确认撤销覆盖？</span>
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
    </div>
  {/if}
</div>

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
  .warnnote.inline {
    margin-top: 0;
  }
  /* 技能目录 / 推荐清单的失败提示：不挡整页，只提示那一块降级了 */
  .skills-error {
    margin-bottom: 14px;
    padding: 8px 10px;
    border: 2px solid var(--stop);
    color: var(--stop);
    font-size: 12px;
    line-height: 1.6;
  }
  /* 决策 84：伪阶段用左缘 4px --text-3 亮度阶 + 名称后缀「（伪阶段）」，不用分支色相 */
  .row.pseudo {
    border-left: 4px solid var(--text-3);
  }
</style>
