<script lang="ts">
  import { onMount } from 'svelte';
  import {
    deleteStageConfig,
    listProviders,
    listSkills,
    listStageConfigs,
    putStageConfig,
  } from '../api/client';
  import type { Provider, SkillSummary, StageConfig } from '../api/types';
  import StageConfigForm from '../components/settings/StageConfigForm.svelte';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import {
    buildStageConfigPut,
    isPseudoStage,
    stageKeyLabel,
    type StageConfigDraft,
  } from '../lib/stageConfigs';
  import { formatDateTime } from '../lib/format';
  import { router } from '../router.svelte';

  /**
   * 阶段配置页（`#/settings/stages`，`route.name === 'settings-stages'`；决策 198 / design §4.3）。
   *
   * **内容整体从「模型与密钥」页搬出**（决策 198 裁决③）：provider 台账与密钥提示留在原处，
   * 这里只装「每个阶段用哪个 provider、带哪些工具与技能」那一件事。搬出来不是重写——
   * 台账盒、整条替换的语义、启动校验的回显、伪阶段左缘亮度阶全部逐字保留，只换了位置与
   * 小节标题的字阶（票 09：小节标题不再与页面标题同为 24px，落到 12px 字距档）。
   *
   * 表单仍由 `StageConfigForm`（S 名下）承担；本页只负责取数、接线与台账渲染。
   * provider 台账与技能目录是为表单的候选（datalist / 技能声明控件）读的：读不到就降级为
   * 手工输入，不挡这一页（与「模型与密钥」页同姿态）。
   */

  let stageConfigs = $state<StageConfig[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  type Editing = { mode: 'new' } | { mode: 'edit'; config: StageConfig };
  let editing = $state<Editing | null>(null);
  let saving = $state(false);
  let formError = $state<string | null>(null);

  let confirmingDelete = $state<string | null>(null);
  let deleteBusy = $state<string | null>(null);
  let rowError = $state<{ stage: string; message: string } | null>(null);

  /** 表单的候选项（读失败不挡页：控件退化为手工输入）。 */
  let providers = $state<Provider[]>([]);
  let skills = $state<SkillSummary[]>([]);

  async function load() {
    loading = true;
    error = null;
    try {
      stageConfigs = await listStageConfigs();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  async function loadCandidates() {
    try {
      providers = await listProviders();
    } catch {
      // 候选取不到：provider_id 仍可手工输入
    }
    try {
      skills = await listSkills();
    } catch {
      // 目录取不到：技能控件退化为「手动输入技能名」
    }
  }

  onMount(() => {
    void load();
    void loadCandidates();
  });

  function openNew() {
    formError = null;
    editing = { mode: 'new' };
  }

  function openEdit(config: StageConfig) {
    formError = null;
    editing = { mode: 'edit', config };
  }

  async function submit(draft: StageConfigDraft) {
    if (!editing) return;
    const built = buildStageConfigPut(draft);
    if (!built.ok) {
      formError = built.error;
      return;
    }
    saving = true;
    formError = null;
    try {
      await putStageConfig(draft.stage, built.payload);
      editing = null;
      await load();
    } catch (err) {
      // 400 { error }（provider 缺失/禁用/vendor 不支持、persona 不可读、会破坏启动的改动）原样回显
      formError = (err as Error).message;
    } finally {
      saving = false;
    }
  }

  async function remove(config: StageConfig) {
    deleteBusy = config.stage;
    rowError = null;
    try {
      await deleteStageConfig(config.stage);
      confirmingDelete = null;
      await load();
    } catch (err) {
      // 404（无配置）/ 400（删除会破坏启动校验）都回显后端原因
      rowError = { stage: config.stage, message: (err as Error).message };
      confirmingDelete = null;
    } finally {
      deleteBusy = null;
    }
  }
</script>

<div class="page">
  <div class="crumbs">
    <!-- 设置子页给一条回落地页的路（design §4.3），与既有的「← 看板」并列 -->
    <a class="crumb" href="#/settings" onclick={() => router.navigate('/settings')}>← 设置</a>
    <a class="crumb" href="#/" onclick={() => router.navigate('/')}>← 看板</a>
  </div>
  <div class="p-head">
    <h1 class="p-title">设置 · 阶段配置</h1>
  </div>

  <div class="stage-head">
    <!-- 小节标题：12px 字距档，不与页面标题（24px）同级（票 09） -->
    <h2 class="block-title cond">阶段配置</h2>
    <button type="button" class="btn" onclick={openNew}>＋ 新增阶段配置</button>
  </div>
  <p class="hintline">
    阶段配置优先于全局默认。保存为<b>整条替换</b>：留空字段清空为默认。写入会跑启动校验，非法配置
    （provider 缺失/禁用/厂商不支持、persona 不可读、会破坏启动的改动）被拒并回显原因。
  </p>

  {#if editing}
    {#key editing.mode === 'new' ? 'sc-new' : editing.config.stage}
      <StageConfigForm
        config={editing.mode === 'edit' ? editing.config : null}
        {providers}
        {skills}
        submitting={saving}
        error={formError}
        onsubmit={submit}
        oncancel={() => (editing = null)}
      />
    {/key}
  {/if}

  {#if error}
    <!-- 读不到要有出路（票 02 / R2-07c）：`load()` 只在 onMount 调，没有这颗钮就只能整页刷新。 -->
    <div class="banner error" role="alert">{error}</div>
    <div class="retry">
      <button type="button" class="btn" disabled={loading} onclick={() => void load()}>重试</button>
    </div>
  {:else if loading}
    <div class="banner">正在加载阶段配置…</div>
  {:else if stageConfigs.length === 0}
    <EmptyState
      state="还没有阶段覆盖。"
      next="所有阶段都在用系统默认配置。要按阶段换模型或加技能，点「＋ 新增阶段配置」。"
    />
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
              {#if rowError?.stage === sc.stage}
                <div class="reg-err">{rowError.message}</div>
              {/if}
            </div>
            <div class="reg-acts">
              {#if confirmingDelete === sc.stage}
                <span class="reg-sub">确认撤销覆盖？</span>
                <button
                  type="button"
                  class="btn danger"
                  disabled={deleteBusy === sc.stage}
                  onclick={() => remove(sc)}
                >
                  {#if deleteBusy === sc.stage}<span class="spin"></span>{/if}删除
                </button>
                <button type="button" class="btn quiet" onclick={() => (confirmingDelete = null)}>
                  取消
                </button>
              {:else}
                <button type="button" class="btn" onclick={() => openEdit(sc)}>编辑</button>
                <button
                  type="button"
                  class="btn danger"
                  onclick={() => {
                    rowError = null;
                    confirmingDelete = sc.stage;
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
  .crumbs {
    display: flex;
    gap: 12px;
  }
  /* 区块头：12px 小节标题 + 右侧动作钮（与台账页基元同形，只是标题落到字距档） */
  .stage-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    margin: 8px 0 6px;
  }
  .block-title {
    font-size: 12px;
    color: var(--text-hi);
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
  /* 错误横幅下的出路（票 02）：横幅与它的重试钮是同一件事。 */
  .retry {
    margin: 8px 0 12px;
  }
  /* 伪阶段用左缘 4px --text-3 亮度阶 + 名称后缀「（伪阶段）」，不用分支色相 */
  .row.pseudo {
    border-left: 4px solid var(--text-3);
  }
</style>
