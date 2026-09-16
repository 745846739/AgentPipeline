import { render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import TopBar from './TopBar.svelte';

/**
 * 顶栏导航项随来源变化的那一条（决策 190）：**不是本机打开的页面不给「手机访问」入口**。
 *
 * 判据本身（主机名）钉在 `lib/localPage.test.ts`；这里钉的是**接线**——组件确实按它取舍了
 * 入口，且其余项一个不少（藏一个入口顺带把别的一起藏了，是这类改动最容易犯的错）。
 */

const mocks = vi.hoisted(() => ({ onHostMachine: vi.fn() }));

vi.mock('../../lib/localPage', () => ({ onHostMachine: mocks.onHostMachine }));

/** 除「手机访问」外的那五个入口（决策 174 / 187）。 */
const OTHERS = ['对讲台', '指标', '项目', '模型与密钥', '技能市场'];

afterEach(() => {
  vi.resetAllMocks();
  document.body.innerHTML = '';
});

describe('顶栏导航（决策 190）', () => {
  it('在本机打开：六个入口齐全，含「手机访问」', () => {
    mocks.onHostMachine.mockReturnValue(true);
    render(TopBar);

    for (const label of [...OTHERS, '手机访问']) {
      expect(screen.queryByRole('link', { name: label }), label).not.toBeNull();
    }
  });

  it('在手机（或任何非本机来源）打开：不给「手机访问」，其余入口不动', () => {
    mocks.onHostMachine.mockReturnValue(false);
    render(TopBar);

    expect(screen.queryByRole('link', { name: '手机访问' })).toBeNull();
    for (const label of OTHERS) {
      expect(screen.queryByRole('link', { name: label }), label).not.toBeNull();
    }
  });
});
