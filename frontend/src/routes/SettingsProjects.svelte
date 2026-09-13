<script lang="ts">
  import { onMount } from 'svelte';
  import {
    createProject,
    deleteProject,
    getProjectAnalysis,
    listProjects,
    listTasks,
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

  /**
   * 各项目的活跃任务数（决策 101：有活跃任务的项目不可删）。
   * 后端删除时会再判一次并 409——这里先判是为了**在按钮上就禁掉并说明原因**，
   * 而不是让用户点一次才知道。取数失败时留空（不误禁，后端仍是最后一道闸）。
   */
  const ACTIVE_STATUSES = new Set(['queued', 'waiting', 'running', 'pending']);
  let activeCounts = $state<Record<string, number>>({});

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

  async function loadActiveCounts() {
    try {
      const tasks = await listTasks({ include_archived: false });
      const counts: Record<string, number> = {};
      for (const t of tasks) {
        if (ACTIVE_STATUSES.has(t.status)) counts[t.project_id] = (counts[t.project_id] ?? 0) + 1;
      }
      activeCounts = counts;
    } catch {
      // 活跃计数取不到不阻断列表；删除仍由后端 409 兜底
      activeCounts = {};
    }
  }

  async function load() {
    loading = true;
    error = null;
    try {
      projects = await listProjects();
      await loadActiveCounts();
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
  <div class="p-head">
    <h1 class="p-title">设置 · 项目</h1>
    <button type="button" class="btn solid" onclick={openNew}>＋ 新建项目</button>
  </div>

  <p class="hintline">
    本地路径是项目唯一事实来源（决策 29）。创建时立即校验 git 仓库（决策 61）；<b>删除有活跃任务的项目会被拒绝并给出原因</b>（决策 101）。
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
    <div class="reg">
      <div class="reg-head">
        <span>项目</span>
        <span class="n">▪ {projects.length}</span>
      </div>
      <ul class="reg-rows">
        {#each projects as p (p.id)}
          {@const active = activeCounts[p.id] ?? 0}
          <li class="reg-row row">
            <div class="reg-main">
              <div class="reg-l1">
                <span class="reg-name">{p.name}</span>
                <span class="reg-sub mono">{p.default_branch}</span>
              </div>
              <div class="reg-path mono">{p.local_path}</div>
              <div class="reg-l2 mono">
                <span>lang {p.language ?? '—'}</span>
                <span>test {p.test_framework ?? '—'}</span>
                <span>lint {p.lint_command ?? '—'}</span>
                <span>AGENTS.md {p.agents_md_path ?? '—'}</span>
              </div>
              {#if active > 0}
                <div class="reg-err">该项目有 {active} 个活跃任务，不能删除（决策 101）。</div>
              {/if}
              {#if rowError?.id === p.id}<div class="reg-err">{rowError.message}</div>{/if}
            </div>
            <div class="reg-acts">
              {#if confirmingDelete === p.id}
                <span class="reg-sub">确认删除？</span>
                <button
                  type="button"
                  class="btn danger"
                  disabled={deleteBusy === p.id || active > 0}
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
                  disabled={active > 0}
                  title={active > 0 ? `该项目有 ${active} 个活跃任务，不能删除（决策 101）` : undefined}
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

  {#if analysisFor}
    {#if analysisError}
      <div class="banner error">{analysisError}</div>
    {:else if analysis}
      <AnalysisChecklist {analysis} onclose={() => (analysisFor = null)} />
    {:else}
      <section class="analysis">
        <div class="running">
          <span class="st run">分析中</span>正在触发 project_analysis 伪阶段…
        </div>
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
  .banner {
    padding: 10px 12px;
    border: 2px solid var(--pane);
    color: var(--text-3);
    font-size: 12px;
    margin-top: 10px;
  }
  .banner.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  .analysis {
    padding: 12px 14px;
    margin-top: 10px;
    border: 2px solid var(--pane);
    background: var(--panel);
  }
  .running {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-2);
  }
</style>
