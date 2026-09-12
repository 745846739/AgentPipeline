import { describe, expect, it } from 'vitest';
import {
  ANALYSIS_POLL_TIMEOUT_MS,
  analysisChecklist,
  analysisPollDelayMs,
  analysisReady,
  analysisStatusLabel,
  isTerminalAnalysisStatus,
  shouldContinuePolling,
} from './analysis';
import type { ProjectAnalysisResult } from '../api/types';

const result: ProjectAnalysisResult = {
  language: 'Rust',
  test_framework: 'cargo test',
  lint_command: 'cargo clippy',
  agents_md_path: 'AGENTS.md',
  has_gitignore: true,
  default_branch: 'main',
  suspicious: [],
};

describe('项目分析轮询状态机（决策 130⑦）', () => {
  it('running 非终态；done / failed 为终态', () => {
    expect(isTerminalAnalysisStatus('running')).toBe(false);
    expect(isTerminalAnalysisStatus('done')).toBe(true);
    expect(isTerminalAnalysisStatus('failed')).toBe(true);
  });

  it('退避 250ms 起、1.5s 封顶', () => {
    expect(analysisPollDelayMs(0)).toBe(250);
    expect(analysisPollDelayMs(1)).toBe(500);
    expect(analysisPollDelayMs(2)).toBe(1000);
    expect(analysisPollDelayMs(3)).toBe(1500);
    expect(analysisPollDelayMs(10)).toBe(1500);
  });

  it('终态或超时即停止轮询', () => {
    expect(shouldContinuePolling('running', 0)).toBe(true);
    expect(shouldContinuePolling('running', ANALYSIS_POLL_TIMEOUT_MS - 1)).toBe(true);
    expect(shouldContinuePolling('running', ANALYSIS_POLL_TIMEOUT_MS)).toBe(false);
    expect(shouldContinuePolling('done', 10)).toBe(false);
    expect(shouldContinuePolling('failed', 10)).toBe(false);
  });

  it('状态文案', () => {
    expect(analysisStatusLabel('running')).toBe('分析中…');
    expect(analysisStatusLabel('done')).toBe('分析完成');
    expect(analysisStatusLabel('failed')).toBe('分析失败');
    expect(analysisStatusLabel('weird')).toBe('weird');
  });
});

describe('分析结果 → 核对清单（design §7）', () => {
  it('探测到的项 ok=true，值原样呈现', () => {
    const items = analysisChecklist(result);
    const byKey = Object.fromEntries(items.map((i) => [i.key, i]));
    expect(byKey.language).toMatchObject({ value: 'Rust', ok: true });
    expect(byKey.test_framework).toMatchObject({ value: 'cargo test', ok: true });
    expect(byKey.has_gitignore).toMatchObject({ value: '已存在', ok: true });
    expect(byKey.suspicious).toMatchObject({ value: '无', ok: true });
  });

  it('未探测到的项 ok=false，不假装通过', () => {
    const items = analysisChecklist({
      ...result,
      language: null,
      test_framework: null,
      lint_command: null,
      agents_md_path: null,
      has_gitignore: false,
      default_branch: '',
      suspicious: [{ kind: 'multi-lockfile' }],
    });
    const byKey = Object.fromEntries(items.map((i) => [i.key, i]));
    expect(byKey.language.ok).toBe(false);
    expect(byKey.language.value).toBeNull();
    expect(byKey.default_branch.ok).toBe(false);
    expect(byKey.suspicious).toMatchObject({ value: '1 项待人工核对', ok: false });
  });

  it('analysisReady 仅在 done 且有结果时为真', () => {
    expect(analysisReady(null)).toBe(false);
    expect(analysisReady({ analysis_id: 'a', status: 'running', result: null, error: null })).toBe(
      false,
    );
    expect(analysisReady({ analysis_id: 'a', status: 'done', result, error: null })).toBe(true);
    expect(analysisReady({ analysis_id: 'a', status: 'failed', result: null, error: 'x' })).toBe(
      false,
    );
  });
});
