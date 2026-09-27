import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { theme } from './theme.svelte';

/**
 * 深浅配色 store（决策 300）：切换钮有两处挂载，**一份状态**是它成立的前提。
 * 钉三件事——切换真的落三处（状态 / `data-theme` / 本地记忆）、恢复按本地记忆走、
 * 本地记忆不可写时本次会话照样能切（隐私模式那条既有语义，逐字没变）。
 */
describe('theme store（决策 300）', () => {
  beforeEach(() => {
    localStorage.clear();
    delete document.documentElement.dataset.theme;
    theme.reset();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
    delete document.documentElement.dataset.theme;
    theme.reset();
  });

  it('toggle：状态翻面，并把结果写进 data-theme 与本地记忆', () => {
    const before = theme.current;
    theme.toggle();

    expect(theme.current).not.toBe(before);
    expect(document.documentElement.dataset.theme).toBe(theme.current);
    expect(localStorage.getItem('agentpipeline.theme')).toBe(theme.current);
  });

  it('reset：回到本地记忆里的值（另一处挂载冷启动读的就是这个）', () => {
    localStorage.setItem('agentpipeline.theme', theme.current === 'dark' ? 'light' : 'dark');
    theme.current = theme.current === 'dark' ? 'dark' : 'light'; // 污染运行态
    theme.reset();

    expect(theme.current).toBe(localStorage.getItem('agentpipeline.theme'));
  });

  it('本地记忆里是垃圾值时退回深色（与 index.html 同一口径）', () => {
    localStorage.setItem('agentpipeline.theme', 'neon');
    theme.reset();

    expect(theme.current).toBe('dark');
  });

  it('本地记忆不可写：本次会话仍能切换（只是不落盘）', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('隐私模式');
    });
    const before = theme.current;

    expect(() => theme.toggle()).not.toThrow();
    expect(theme.current).not.toBe(before);
    expect(document.documentElement.dataset.theme).toBe(theme.current);
  });
});
