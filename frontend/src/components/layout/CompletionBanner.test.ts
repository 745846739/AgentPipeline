import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { completion } from '../../stores/completion.svelte';
import { setApiBase } from '../../api/config';
import CompletionBanner from './CompletionBanner.svelte';

/**
 * 完成横幅组件（票 08）：
 * - done 时出现且含 diff 摘要（`+N −M`）；
 * - 点「收下」后消失；
 * - 摘要缺失时渲染标题与「已合入」，**不渲染 0**。
 */
const DIFF = [
  'diff --git a/src/lib.js b/src/lib.js',
  '--- a/src/lib.js',
  '+++ b/src/lib.js',
  '@@ -1,2 +1,3 @@',
  '+const a = 1;',
  '-const b = 2;',
  ' // ctx',
  '',
].join('\n');

function stubDiff(body: string | null): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () =>
      body === null
        ? new Response('nf', { status: 404 })
        : new Response(body, { status: 200, headers: { 'Content-Type': 'text/plain' } }),
    ),
  );
}

function enterDone(title = '接入 SQLite 迁移') {
  completion.observeAll([{ id: 't1', status: 'running', title }]);
  completion.observeAll([{ id: 't1', status: 'done', title }]);
}

beforeEach(() => {
  setApiBase(null);
  completion.reset();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('CompletionBanner（票 08）', () => {
  it('任务 done 时出现，含 trophy 图元、可访问名与 diff 摘要', async () => {
    stubDiff(DIFF);
    enterDone();
    render(CompletionBanner);

    const banner = screen.getByRole('status', { name: '任务完成' });
    expect(banner).toBeTruthy();
    expect(banner.textContent).toContain('任务完成');
    expect(banner.textContent).toContain('接入 SQLite 迁移');
    expect(banner.textContent).toContain('已合入');

    // trophy sprite 是受控图元（非空 SVG + currentColor 填充）
    const svg = banner.querySelector('svg.sprite');
    expect(svg).toBeTruthy();
    expect(svg?.querySelectorAll('rect').length).toBeGreaterThan(0);

    // diff 摘要如 `+1 −1`
    await waitFor(() => expect(banner.textContent).toContain('+1'));
    expect(banner.textContent).toContain('−1');
  });

  it('点「收下」后横幅消失', async () => {
    stubDiff(DIFF);
    enterDone();
    render(CompletionBanner);

    await fireEvent.click(screen.getByRole('button', { name: '收下' }));
    expect(completion.notice).toBeNull();
    expect(screen.queryByRole('status', { name: '任务完成' })).toBeNull();
  });

  it('摘要不可得时不渲染 0：只有标题与「已合入」', async () => {
    stubDiff(null);
    enterDone('没有 diff 的任务');
    render(CompletionBanner);

    const banner = screen.getByRole('status', { name: '任务完成' });
    await waitFor(() => expect(vi.mocked(fetch)).toHaveBeenCalled());
    await Promise.resolve();

    expect(banner.textContent).toContain('没有 diff 的任务');
    expect(banner.textContent).toContain('已合入');
    // 「不画 0 冒充真实值」：+/- 数字一个都不出现
    expect(banner.textContent).not.toMatch(/[+−]\d/);
  });

  it('failed / cancelled 不渲染横幅（既有终态提示保留）', () => {
    completion.observeAll([{ id: 't1', status: 'running', title: '失败任务' }]);
    completion.note('t1', 'failed', '失败任务');
    completion.note('t1', 'cancelled', '失败任务');
    render(CompletionBanner);

    expect(screen.queryByRole('status', { name: '任务完成' })).toBeNull();
  });
});
