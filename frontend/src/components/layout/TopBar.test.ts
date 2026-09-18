import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import type { TaskListItem } from '../../api/types';
import { router } from '../../router.svelte';
import { board } from '../../stores/board.svelte';
import TopBar from './TopBar.svelte';

/**
 * 待处理下拉的键盘与关闭（票 04 / R2-04）。
 *
 * 它原来播报自己是 `role="menu"`，却没有菜单的任何行为：Escape 关不掉、点面板外面也不关、
 * 方向键不动。这一组钉的是**用户按得动的四件事**与触发钮那两个属性：
 * Escape 关得掉、点外关得掉、ArrowDown 把焦点送进第一项、关闭后焦点回到触发钮。
 *
 * 断言落在可访问性契约上（role / aria-expanded / aria-controls / 焦点落点），不落 class 名。
 */

function pendingTask(id: string, title: string): TaskListItem {
  return {
    id,
    project_id: 'p1',
    title,
    description: '',
    status: 'pending',
    current_stage: 'merge',
    current_node: 'execute',
    validate_attempts: 0,
    pending_reason: { type: 'merge_approval', stage: 'merge', node: 'execute', message: '等你拍板' },
    worktree_path: null,
    branch_name: null,
    stewardship: null,
    total_tokens: 0,
    total_calls: 0,
    review_mode: 'agent',
    model_override: null,
    archived_at: null,
    stalled: false,
    executor_owner: null,
    created_at: '2026-09-16T00:00:00Z',
    updated_at: '2026-09-16T00:10:00Z',
    branches: [],
    blocks: [],
  };
}

// 芯片的可读名是「待处理 2」；过滤槽那一格是「待处理（2）」（全角括号），故按形状分开
beforeAll(() => {
  // jsdom 不实现 ResizeObserver，而顶栏用 `bind:offsetHeight` 量自己（票 09 的
  // `--topbar-h`）。照 `TaskDetail.test.ts` 里 matchMedia 那条先例就地补一个空壳。
  if (typeof globalThis.ResizeObserver !== 'function') {
    globalThis.ResizeObserver = class {
      observe(): void {}
      unobserve(): void {}
      disconnect(): void {}
    } as unknown as typeof ResizeObserver;
  }
});

const trigger = () => screen.getByRole('button', { name: /^待处理 \d+$/ });
const panel = () => document.getElementById('pending-dropdown') as HTMLElement;
const items = () => [...panel().querySelectorAll('a.dd-item')] as HTMLAnchorElement[];
const isOpen = () => trigger().getAttribute('aria-expanded') === 'true';

beforeEach(() => {
  board.tasks = [pendingTask('t1', '甲任务'), pendingTask('t2', '乙任务')];
  board.pendingOpen = false;
  router.hash = '#/';
});

afterEach(() => {
  board.pendingOpen = false;
  document.body.innerHTML = '';
});

describe('待处理下拉（票 04 / R2-04）', () => {
  it('触发钮有 aria-expanded 与 aria-controls，且指向真实存在的元素', () => {
    render(TopBar);
    expect(trigger().getAttribute('aria-controls')).toBe('pending-dropdown');
    expect(isOpen()).toBe(false);
    // IDREF 不悬空：面板常驻 DOM，靠 hidden 开合
    expect(panel()).toBeTruthy();
    expect(panel().hasAttribute('hidden')).toBe(true);
  });

  it('打开之后按 Escape 关得掉（焦点从没进过面板也算）', async () => {
    render(TopBar);
    await fireEvent.click(trigger());
    expect(isOpen()).toBe(true);
    expect(panel().hasAttribute('hidden')).toBe(false);

    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(isOpen()).toBe(false);
    expect(panel().hasAttribute('hidden')).toBe(true);
  });

  it('点面板外面关得掉；点面板里面不关', async () => {
    render(TopBar);
    await fireEvent.click(trigger());
    await fireEvent.click(items()[0]);
    expect(isOpen(), '点面板里面的项不该把面板关掉之外再触发一次开关').toBe(false);

    await fireEvent.click(trigger());
    expect(isOpen()).toBe(true);
    await fireEvent.click(document.body);
    expect(isOpen()).toBe(false);
  });

  it('触发钮上按 ArrowDown：打开并把焦点送进第一项', async () => {
    render(TopBar);
    trigger().focus();
    await fireEvent.keyDown(trigger(), { key: 'ArrowDown' });

    await waitFor(() => expect(document.activeElement).toBe(items()[0]));
    expect(isOpen()).toBe(true);
  });

  it('面板内 ArrowDown / ArrowUp 在项间走，Escape 把焦点还给触发钮', async () => {
    render(TopBar);
    await fireEvent.click(trigger());
    items()[0].focus();


    await fireEvent.keyDown(items()[0], { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items()[1]);

    await fireEvent.keyDown(items()[1], { key: 'Home' });
    expect(document.activeElement).toBe(items()[0]);

    await fireEvent.keyDown(items()[0], { key: 'End' });
    expect(document.activeElement).toBe(items()[1]);

    await fireEvent.keyDown(items()[1], { key: 'Escape' });
    expect(isOpen()).toBe(false);
    expect(document.activeElement).toBe(trigger());
  });

  it('每一项是**链接**（跳任务详情），不是菜单项', async () => {
    render(TopBar);
    await fireEvent.click(trigger());

    expect(items().map((a) => a.getAttribute('href'))).toEqual(['#/task/t1', '#/task/t2']);
    // 降级掉的正是这两个 role：链接不是菜单项
    expect(panel().getAttribute('role')).toBeNull();
    expect(items()[0].getAttribute('role')).toBeNull();
  });
});

/* ───────────────────────── 顶栏页面导航行（决策 198） ───────────────────────── */

/**
 * 顶栏**页面导航行**（决策 198 / design §4.2）：由六项收到三项——对讲台 / 指标 / 设置。
 *
 * 这是**有意的收缩**：顶栏是「第一屏必须懂」的那一处。原「项目 / 模型与密钥 / 技能市场 /
 * 手机访问」四项从这一行移入设置落地页（项名逐字不改、各自路由不变），故这里同时钉两件事：
 * 三项**恰好在**、四项**确实不在**——只钉前者，多留一项也照样绿。
 *
 * 「手机访问」那条「只在本机给入口」的规则随入口一起挪到了落地页，接线测试因此在
 * `SettingsLanding.test.ts`（本文件原先那份 mock `onHostMachine` 的用例已随之搬走）。
 *
 * 断言只落在可访问性契约上（导航区的名字、链接的可读名与 href），不落 class 名。
 */
const NAV: Array<{ label: string; href: string }> = [
  { label: '对讲台', href: '#/talk' },
  { label: '指标', href: '#/metrics' },
  { label: '设置', href: '#/settings' },
];

/** 从这一行移入落地页的四项（它们不该再出现在顶栏导航行里）。 */
const MOVED_OUT = ['项目', '模型与密钥', '技能市场', '手机访问'];

describe('顶栏页面导航行（决策 198）', () => {
  it('当前项带 aria-current="page"（票 06：此前只靠 CSS 亮一下）', () => {
    router.hash = '#/talk';
    render(TopBar);
    const nav = screen.getByRole('navigation', { name: '页面导航' });
    expect(within(nav).getByRole('link', { name: '对讲台' }).getAttribute('aria-current')).toBe(
      'page',
    );
    expect(within(nav).getByRole('link', { name: '指标' }).getAttribute('aria-current')).toBeNull();
  });

  it('恰三项：对讲台 / 指标 / 设置（顺序与落点逐字如此）', () => {
    render(TopBar);
    const nav = screen.getByRole('navigation', { name: '页面导航' });
    const chips = within(nav).getAllByRole('link');

    expect(chips.map((a) => a.textContent?.trim())).toEqual(NAV.map((n) => n.label));
    expect(chips.map((a) => a.getAttribute('href'))).toEqual(NAV.map((n) => n.href));
  });

  it('「新建任务」与 wordmark 仍在这一行之外（收缩只针对导航行）', () => {
    render(TopBar);
    const nav = screen.getByRole('navigation', { name: '页面导航' });

    // 「新建任务」是顶栏的道具栏动作，不在导航行里——它是第一屏控件，一字不动
    expect(within(nav).queryByRole('button', { name: '新建任务' })).toBeNull();
    expect(screen.getByRole('button', { name: '新建任务' })).not.toBeNull();
    // wordmark 是看板入口，也不是页面导航项
    expect(within(nav).queryByRole('link', { name: 'AGENTPIPELINE' })).toBeNull();
    expect(screen.getByRole('link', { name: 'AGENTPIPELINE' })).not.toBeNull();
  });

  it('移入落地页的四项不再出现在导航行里', () => {
    render(TopBar);
    const nav = screen.getByRole('navigation', { name: '页面导航' });

    for (const label of MOVED_OUT) {
      expect(
        within(nav).queryByRole('link', { name: label }),
        `${label} 仍在顶栏导航行里`,
      ).toBeNull();
    }
  });
});
