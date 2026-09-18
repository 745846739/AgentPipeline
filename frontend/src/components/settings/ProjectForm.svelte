<script lang="ts">
  import { untrack } from 'svelte';
  import type { Project } from '../../api/types';
  import {
    draftFromProject,
    emptyProjectDraft,
    validateProjectDraft,
    type ProjectDraft,
    type ProjectFieldError,
  } from '../../lib/projects';

  /**
   * 项目创建 / 编辑表单。
   * - 创建：name + local_path（必填）+ default_branch（可选，后端缺省取当前分支）——
   *   local_path 由后端立即校验 git 仓库（决策 61），拒绝原因经 error 回显；
   * - 编辑：PATCH 仅改名 / default_branch / test_framework / lint_command（决策 101），
   *   local_path 是唯一事实来源，不可改（决策 29）。
   */
  interface Props {
    project: Project | null;
    submitting: boolean;
    error: string | null;
    onsubmit: (draft: ProjectDraft) => void;
    oncancel: () => void;
  }
  let { project, submitting, error, onsubmit, oncancel }: Props = $props();

  const isNew = untrack(() => project === null);
  // 只取初始值：父组件以 {#key} 重挂载表单，project 变更即重新初始化。
  let draft = $state<ProjectDraft>(
    untrack(() => (project ? draftFromProject(project) : emptyProjectDraft())),
  );
  let localError = $state<ProjectFieldError | null>(null);
  const shownError = $derived(error ?? localError?.message ?? null);
  /**
   * 只有**本地**校验知道错在哪一格（后端拒绝原因是一句话，没有落点）。
   * 出错的那一格拿 `aria-invalid` + 指向错误节点的 `aria-describedby`（票 02 / R2-06）。
   */
  const badField = $derived(localError?.field ?? null);

  function submit(e: SubmitEvent) {
    e.preventDefault();
    const invalid = validateProjectDraft(draft, isNew);
    if (invalid) {
      localError = invalid;
      return;
    }
    localError = null;
    onsubmit(draft);
  }
</script>

<form class="proj-form panel" onsubmit={submit}>
  <div class="form-head cond">{isNew ? '新建项目' : `编辑项目 · ${project?.name}`}</div>

  <div class="grid">
    <label class="field">
      <span>名称</span>
      <input
        class="input"
        bind:value={draft.name}
        placeholder="项目显示名"
        aria-invalid={badField === 'name' ? 'true' : undefined}
        aria-describedby={badField === 'name' ? 'proj-form-error' : undefined}
      />
    </label>

    <label class="field">
      <span>默认分支（创建时可留空 → 取当前分支）</span>
      <input class="input mono" bind:value={draft.default_branch} placeholder="main" />
    </label>

    <label class="field wide">
      <span>本地路径 local_path{isNew ? '（需为已初始化且有提交的 git 仓库）' : '（不可修改）'}</span>
      <input
        class="input mono"
        bind:value={draft.local_path}
        readonly={!isNew}
        placeholder="/Users/you/code/project"
        aria-invalid={badField === 'local_path' ? 'true' : undefined}
        aria-describedby={badField === 'local_path' ? 'proj-form-error' : undefined}
      />
    </label>

    {#if !isNew}
      <label class="field">
        <span>测试框架</span>
        <input class="input mono" bind:value={draft.test_framework} placeholder="cargo test" />
      </label>
      <label class="field">
        <span>Lint 命令</span>
        <input class="input mono" bind:value={draft.lint_command} placeholder="cargo clippy" />
      </label>
    {/if}
  </div>

  {#if isNew}
    <p class="hint">
      创建后立即校验：路径必须存在、是 git 仓库且 HEAD 已有提交。校验失败会原样回显后端拒绝原因。
    </p>
  {/if}

  {#if shownError}
    <div class="error" id="proj-form-error" role="alert">{shownError}</div>
  {/if}

  <div class="actions">
    <button type="button" class="btn quiet" disabled={submitting} onclick={oncancel}>取消</button>
    <button type="submit" class="btn solid" disabled={submitting}>
      {#if submitting}<span class="spin"></span>{/if}
      {isNew ? '创建' : '保存'}
    </button>
  </div>
</form>

<style>
  .proj-form {
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
  .input[readonly] {
    opacity: 0.6;
    cursor: not-allowed;
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
    white-space: pre-wrap;
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
