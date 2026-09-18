import type { Project, ProjectCreatePayload, ProjectPatchPayload } from '../api/types';

/** 项目表单草稿与校验（决策 29 / 61 / 101）。`local_path` 创建后不可改。 */
export interface ProjectDraft {
  name: string;
  local_path: string;
  default_branch: string;
  test_framework: string;
  lint_command: string;
}

export function emptyProjectDraft(): ProjectDraft {
  return {
    name: '',
    local_path: '',
    default_branch: '',
    test_framework: '',
    lint_command: '',
  };
}

export function draftFromProject(p: Project): ProjectDraft {
  return {
    name: p.name,
    local_path: p.local_path,
    default_branch: p.default_branch,
    test_framework: p.test_framework ?? '',
    lint_command: p.lint_command ?? '',
  };
}

/**
 * 字段级校验结果（票 02 / R2-06）。
 *
 * 校验失败要能指到**哪一格**：只有文案的版本没法给那一格 `aria-invalid` +
 * `aria-describedby`，读屏用户听到的是一句没有落点的告警。
 */
export interface ProjectFieldError {
  field: 'name' | 'local_path';
  message: string;
}

/** 返回 null 表示通过；否则是出错的那一格与界面提示文案。 */
export function validateProjectDraft(draft: ProjectDraft, isNew: boolean): ProjectFieldError | null {
  if (!draft.name.trim()) return { field: 'name', message: '请填写项目名。' };
  if (isNew && !draft.local_path.trim()) {
    return { field: 'local_path', message: '请填写本地路径。' };
  }
  return null;
}

/** 创建载荷：local_path 必填；可选字段留空则省略（后端缺省取当前分支）。 */
export function buildProjectCreate(draft: ProjectDraft): ProjectCreatePayload {
  const payload: ProjectCreatePayload = {
    name: draft.name.trim(),
    local_path: draft.local_path.trim(),
  };
  const branch = draft.default_branch.trim();
  if (branch) payload.default_branch = branch;
  return payload;
}

/** 编辑载荷：只发可改字段，空串表示清空（后端 update_project 按 Option 处理）。 */
export function buildProjectPatch(draft: ProjectDraft): ProjectPatchPayload {
  return {
    name: draft.name.trim(),
    default_branch: draft.default_branch.trim(),
    test_framework: draft.test_framework.trim(),
    lint_command: draft.lint_command.trim(),
  };
}
