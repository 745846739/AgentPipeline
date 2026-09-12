import type { ProjectAnalysis, ProjectAnalysisResult } from '../api/types';

/**
 * 项目分析（`POST /projects/analyze` 202 + `GET /projects/{id}/analysis` 轮询，决策 78 / 130⑦）
 * 的纯逻辑：轮询退避、终态判定、结果 → 核对清单映射。
 */

/** 后端 catalog.rs：`running` → `done` | `failed`。 */
export function isTerminalAnalysisStatus(status: string): boolean {
  return status === 'done' || status === 'failed';
}

/** 后端 status 的界面文案。 */
export function analysisStatusLabel(status: string): string {
  switch (status) {
    case 'running':
      return '分析中…';
    case 'done':
      return '分析完成';
    case 'failed':
      return '分析失败';
    default:
      return status;
  }
}

/**
 * 轮询退避：250ms 起指数增长，1.5s 封顶（本地静态探测很快，避免抖动）。
 * 返回下一次查询前应等待的毫秒数。
 */
export function analysisPollDelayMs(attempt: number): number {
  const base = 250 * 2 ** Math.max(0, attempt);
  return Math.min(base, 1500);
}

/** 轮询总时长上限（超过则停止并提示，避免无限挂起）。 */
export const ANALYSIS_POLL_TIMEOUT_MS = 60_000;

/** 给定已等待时长与状态，判断是否应继续轮询。 */
export function shouldContinuePolling(
  status: string,
  elapsedMs: number,
  timeoutMs: number = ANALYSIS_POLL_TIMEOUT_MS,
): boolean {
  return !isTerminalAnalysisStatus(status) && elapsedMs < timeoutMs;
}

export interface ChecklistItem {
  key: string;
  label: string;
  /** 探测到的值；null 表示未探测到。 */
  value: string | null;
  /** 该项是否正常/可确认。 */
  ok: boolean;
}

/**
 * 分析结果 → 核对清单（design §7：结果以核对清单呈现供确认）。
 * 缺失项 `ok=false`，UI 以弱色 + 「未探测到」呈现，而不是假装通过。
 */
export function analysisChecklist(result: ProjectAnalysisResult): ChecklistItem[] {
  const suspicious = Array.isArray(result.suspicious) ? result.suspicious : [];
  return [
    { key: 'language', label: '语言', value: result.language, ok: result.language !== null },
    {
      key: 'default_branch',
      label: '默认分支',
      value: result.default_branch || null,
      ok: Boolean(result.default_branch),
    },
    {
      key: 'test_framework',
      label: '测试框架',
      value: result.test_framework,
      ok: result.test_framework !== null,
    },
    {
      key: 'lint_command',
      label: 'Lint 命令',
      value: result.lint_command,
      ok: result.lint_command !== null,
    },
    {
      key: 'agents_md_path',
      label: 'AGENTS.md',
      value: result.agents_md_path,
      ok: result.agents_md_path !== null,
    },
    {
      key: 'has_gitignore',
      label: '.gitignore',
      value: result.has_gitignore ? '已存在' : '缺失',
      ok: result.has_gitignore,
    },
    {
      key: 'suspicious',
      label: '可疑项',
      value: suspicious.length > 0 ? `${suspicious.length} 项待人工核对` : '无',
      ok: suspicious.length === 0,
    },
  ];
}

/** 分析是否可供用户确认（done 且有 result）。 */
export function analysisReady(analysis: ProjectAnalysis | null): boolean {
  return analysis?.status === 'done' && analysis.result !== null;
}
