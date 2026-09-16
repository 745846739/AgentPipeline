<script lang="ts">
  import { onMount } from 'svelte';
  import {
    createProvider,
    deleteProvider,
    installSkillForStage,
    listProviders,
    listRecommendedSkills,
    updateProvider,
  } from '../api/client';
  import type { Provider, RecommendedStage, SkillPreview } from '../api/types';
  import ProviderForm from '../components/settings/ProviderForm.svelte';
  import StageRecommendations from '../components/settings/StageRecommendations.svelte';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import {
    API_KEY_MASK,
    buildProviderCreate,
    buildProviderPatch,
    isSupportedAdapter,
    validateProviderDraft,
    type ProviderDraft,
  } from '../lib/providers';

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

  /* ── 技能推荐与一键安装（决策 172①④，票 15 / 16）──
     技能目录（`GET /skills`）只为阶段配置表单的候选读，已随那一段搬去 `#/settings/stages`。 */
  let recommendations = $state<RecommendedStage[]>([]);
  /** 正在安装的 `阶段:技能名`（按钮上的转圈与禁用）。 */
  let installing = $state<string | null>(null);
  let skillError = $state<string | null>(null);
  /** 最近一次一键安装带回来的三项预览（票 11）。 */
  let installPreview = $state<SkillPreview | null>(null);

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
      await loadRecommendations();
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

  onMount(() => {
    void load();
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

  /* 阶段配置那一段（决策 198 裁决③ / 票 21）整体搬去 `#/settings/stages`
     ——两边各留一套编辑器会让同一个 `PUT /stage-configs/{stage}` 互相覆盖。 */
</script>

<div class="page">
  <a class="crumb" href="#/">← 看板</a>
  <div class="p-head">
    <h1 class="p-title">设置 · 模型与密钥</h1>
    <button type="button" class="btn solid" onclick={openNew}>＋ 新增 provider</button>
  </div>

  <p class="hintline">
    provider 行 =（vendor, model, context_window）。api_key 明文存储，读接口只回显 <b>{API_KEY_MASK}</b>；
    密钥明文存于本机 <b>~/.agentpipeline</b>，目录权限 <b>0700</b>。
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
    <!-- 空态（票 13）：状态 → 下一步 → 可选入口。入口指向「设置 · 项目」：
         配好模型之后下一件事就是接入一个本地仓库，那个动作在项目页上。容器沿用本页
         既有的 `.banner`（与加载 / 错误提示同一只盒子），形状来自 `<EmptyState>`。 -->
    <div class="banner">
      <EmptyState
        state="还没有 provider。"
        next="新增一行并填好 model 与 api_key，任务的阶段模型才会被解析。"
        href="#/settings/projects"
        linkLabel="下一步：设置 · 项目"
      />
    </div>
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
                {#if !supported}
                  <!-- 决策 199 的定稿文案：正文只说动作与后果，编号不进正文（追溯见
                       design/frontend-design.md §12.3 的行为映射表）。title 放理由、不夹编号
                       ——`lib/copy-discipline.test.ts` 对 title 与模板文本一视同仁。 -->
                  <span class="warnnote inline" title="厂商不在支持列表（supported_adapters）内">
                    ! 不支持这个厂商，该行已停用
                  </span>
                {/if}
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
  /* 票 12 / 决策 203 裁决③：不受支持的厂商是**规格明文允许**的琥珀用法（「该行已停用」——
     用户得动手处理：换厂商或删掉这一行），故**保留** app.css 里 `.warnnote` 的 `--pending`，
     本页不再覆盖它。收敛掉的是那些回答不了「这里要用户处理什么」的地方（推荐技能的
     「未安装」标签、手机访问入口闸标题），不是这一处。 */
  /* 票 04：破坏性动作不比中性动作轻——定档在 app.css 的 `.btn.danger` 上（全站一处），
     本页不再复制一份局部覆盖（票 04 的诉求是「删除」这一类动作的整体量级，不是某一页）。 */
  /* 技能目录 / 推荐清单的失败提示：不挡整页，只提示那一块降级了 */
  .skills-error {
    margin-bottom: 14px;
    padding: 8px 10px;
    border: 2px solid var(--stop);
    color: var(--stop);
    font-size: 12px;
    line-height: 1.6;
  }
</style>
