<script lang="ts">
  import { onMount } from 'svelte';
  import {
    createProject,
    deleteProject,
    getProjectAnalysis,
    listProjects,
    startProjectAnalysis,
    updateProject,
  } from '../api/client';
  import type { Project, ProjectAnalysis } from '../api/types';
  import AnalysisChecklist from '../components/settings/AnalysisChecklist.svelte';
  import ProjectForm from '../components/settings/ProjectForm.svelte';
  import {
    ANALYSIS_POLL_TIMEOUT_MS,
    analysisPollDelayMs,
    isTerminalAnalysisStatus,
    shouldContinuePolling,
  } from '../lib/analysis';
  import {
    buildProjectCreate,
    buildProjectPatch,
    type ProjectDraft,
  } from '../lib/projects';

  let projects = $state<Project[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  type Editing = { mode: 'new' } | { mode: 'edit'; project: Project };
  let editing = $state<Editing | null>(null);
  let saving = $state(false);
  let formError = $state<string | null>(null);

  let confirmingDelete = $state<string | null>(null);
  let deleteBusy = $state<string | null>(null);
  let rowError = $state<{ id: string; message: string } | null>(null);

  /** 当前正在分析 / 展示分析结果的项目（后者在轮询结束后保留）。 */
  let analyzingId = $state<string | null>(null);
  let analysisFor = $state<string | null>(null);
  let analysis = $state<ProjectAnalysis | null>(null);
  let analysisError = $state<string | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      projects = await listProjects();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  onMount(() => void load());

  function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  function openNew() {
    formError = null;
    editing = { mode: 'new' };
  }

  function openEdit(project: Project) {
    formError = null;
    editing = { mode: 'edit', project };
  }

  async function submit(draft: ProjectDraft) {
    if (!editing) return;
    saving = true;
    formError = null;
    try {
      if (editing.mode === 'new') {
        await createProject(buildProjectCreate(draft));
      } else {
        await updateProject(editing.project.id, buildProjectPatch(draft));
      }
      editing = null;
      await load();
    } catch (err) {
      // 创建校验失败（非 git 仓库 / unborn HEAD / 路径不存在）与 PATCH 错误都走这里
      formError = (err as Error).message;
    } finally {
      saving = false;
    }
  }

  async function remove(project: Project) {
    deleteBusy = project.id;
    rowError = null;
    try {
      await deleteProject(project.id);
      confirmingDelete = null;
      if (analysisFor === project.id) analysisFor = null;
      await load();
    } catch (err) {
      // 有活跃任务时后端 409，message 即拒绝原因（决策 101）
      rowError = { id: project.id, message: (err as Error).message };
      confirmingDelete = null;
    } finally {
      deleteBusy = null;
    }
  }

  /** POST /projects/analyze → 202，然后按退避轮询 GET /projects/{id}/analysis 至终态。 */
  async function analyze(project: Project) {
    analyzingId = project.id;
    analysisFor = project.id;
    analysis = null;
    analysisError = null;
    const startedAt = Date.now();
    let attempt = 0;
    try {
      await startProjectAnalysis(project.id);
      for (;;) {
        try {
          analysis = await getProjectAnalysis(project.id);
        } catch (err) {
          // 202 后偶发 404（尚无记录）可继续轮询；超时则放弃并报错
          if (!shouldContinuePolling('running', Date.now() - startedAt)) throw err;
        }
        const status = analysis?.status ?? 'running';
        if (!shouldContinuePolling(status, Date.now() - startedAt, ANALYSIS_POLL_TIMEOUT_MS)) {
          if (!isTerminalAnalysisStatus(status)) {
            analysisError = '分析超时，请稍后重试。';
          }
          break;
        }
        await sleep(analysisPollDelayMs(attempt));
        attempt += 1;
      }
    } catch (err) {
      analysisError = (err as Error).message;
    } finally {
      analyzingId = null;
    }
  }
</script>

<div class="page">
  <a class="crumb" href="#/">← 看板</a>
  <header class="head">
    <h1 class="cond">设置 · 项目</h1>
    <button type="button" class="btn solid" onclick={openNew}>＋ 新建项目</button>
  </header>

  <p class="hint">
    本地路径是项目唯一事实来源（决策 29）。创建时立即校验 git 仓库（决策 61）；删除有活跃任务的项目会被拒绝并给出原因（决策 101）。
  </p>

  {#if editing}
    {#key editing.mode === 'new' ? 'new' : editing.project.id}
      <ProjectForm
        project={editing.mode === 'edit' ? editing.project : null}
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
    <div class="banner">正在加载项目…</div>
  {:else if projects.length === 0}
    <div class="banner">还没有项目。新建一个本地 git 仓库后才能创建任务。</div>
  {:else}
    <ul class="rows">
      {#each projects as p (p.id)}
        <li class="row panel">
          <div class="main">
            <div class="line1">
              <span class="name">{p.name}</span>
              <span class="branch mono">{p.default_branch}</span>
            </div>
            <div class="path mono">{p.local_path}</div>
            <div class="line2 mono">
              <span>lang {p.language ?? '—'}</span>
              <span>test {p.test_framework ?? '—'}</span>
              <span>lint {p.lint_command ?? '—'}</span>
              <span>AGENTS.md {p.agents_md_path ?? '—'}</span>
            </div>
            {#if rowError?.id === p.id}<div class="row-err">{rowError.message}</div>{/if}
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
                class="btn"
                disabled={analyzingId === p.id}
                onclick={() => analyze(p)}
              >
                {#if analyzingId === p.id}<span class="spin"></span>{/if}分析
              </button>
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

  {#if analysisFor}
    {#if analysisError}
      <div class="banner error">{analysisError}</div>
    {:else if analysis}
      <AnalysisChecklist {analysis} onclose={() => (analysisFor = null)} />
    {:else}
      <section class="analysis panel">
        <div class="running"><span class="dot"></span>正在触发 project_analysis 伪阶段…</div>
      </section>
    {/if}
  {/if}
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
    margin-top: 10px;
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
  .main {
    min-width: 0;
  }
  .line1 {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .name {
    color: var(--text-hi);
    font-size: 13px;
    font-weight: 500;
  }
  .branch {
    color: var(--text-3);
    font-size: 11px;
  }
  .path {
    color: var(--text-2);
    font-size: 11.5px;
    margin-top: 2px;
    word-break: break-all;
  }
  .line2 {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-3);
  }
  .row-err {
    margin-top: 5px;
    font-size: 11.5px;
    color: var(--signal-stop);
    white-space: pre-wrap;
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
  .analysis {
    padding: 12px 14px;
    margin-top: 10px;
  }
  .running {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-2);
  }
  .running .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--signal-go);
    animation: breath 1.4s ease-in-out infinite;
  }
</style>
