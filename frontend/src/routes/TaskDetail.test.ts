import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AllowedAction, BranchCursor, Task } from '../api/types';
import { parseUnifiedDiff, type ParsedDiff } from '../lib/diff';
import { emptyTaskDetailState } from '../realtime/reduce';
import TaskDetail from './TaskDetail.svelte';

/**
 * 任务详情页接线层（票 08 / 06 / 07 / 13）。
 *
 * 这一层测的是**组件之间的接线**，不是子组件自身的渲染：
 * - 票 08：档案盒不再和 Diff 页签重复渲染同一份 diff，但动作行（合入 / 返回修改）始终在。
 *   子组件各自的内部行为由其自己的测试与 e2e 兜住；这里钉的是「什么时候把 `diffInPane`
 *   递下去」——这正是「用户不切页签就拍不了板」与「同屏两份 diff」两种坏法的分界。
 * - 票 06 / 07：任务级入口的地址形状是跨流接口契约，参数名逐字（写错对面就不就位）。
 * - 票 13：打不到任务的空态必须给一条**可点**的回去的路。
 *
 * 断言只落在可访问性契约上（heading / link / button 的可读名与其容器），不落 class 名。
 * 文案不做精确匹配（票 25 是文案改动，按先例：证据不是门）。
 */

const mocks = vi.hoisted(() => ({
  detail: {
    state: undefined as unknown,
    loading: false,
    error: null as string | null,
    errorStatus: null as number | null,
    actionError: null,
    busyKey: null,
    diff: null as ParsedDiff | null,
    diffRaw: null as string | null,
    diffError: null,
    diffStale: false,
    files: {},
    conversationsFull: {},
    conversationsLoading: false,
    load: vi.fn(async () => undefined),
    loadDiff: vi.fn(),
    loadFile: vi.fn(),
    loadConversation: vi.fn(),
    loadCommandOutput: vi.fn(),
    outputFor: vi.fn(() => null),
    getFile: vi.fn(() => undefined),
    runAllowedAction: vi.fn(async () => undefined),
    submitReview: vi.fn(async () => undefined),
    submitSplit: vi.fn(async () => undefined),
    submitModelOverride: vi.fn(async () => undefined),
  },
}));

vi.mock('../stores/taskDetail.svelte', () => ({ taskDetail: mocks.detail }));
vi.mock('../api/client', () => ({
  listProviders: vi.fn(async () => []),
  retryTask: vi.fn(async () => undefined),
  archiveTask: vi.fn(async () => undefined),
  // 托管开关（票 14）：接线状态那一读在**这个文件里不摆那颗钮**（默认未接线），
  // `set` 也只是让组件能 import——它的行为由 `lib/stewardship.test.ts` 钉。
  getForemanSessions: vi.fn(async () => ({ sessions: [] })),
  setStewardship: vi.fn(async () => ({ ok: true })),
}));

const DIFF_RAW = [
  'diff --git a/src/lib.js b/src/lib.js',
  '--- a/src/lib.js',
  '+++ b/src/lib.js',
  '@@ -1,1 +1,1 @@',
  '-function add(a, b) { return 0; }',
  '+function add(a, b) { return a + b; }',
  '',
].join('\n');

/** 停在合并提案上的任务（`merge_approval`：右栏档案盒 + Diff 页签并存的那一态）。 */
function pendingTask(): Task {
  return {
    id: 'task-1',
    project_id: 'proj-9',
    title: '合入前等人的任务',
    description: '',
    status: 'pending',
    current_stage: 'merge',
    current_node: 'execute',
    validate_attempts: 0,
    pending_reason: {
      type: 'merge_approval',
      stage: 'merge',
      node: 'execute',
      message: '合并提案已生成，等你拍板。',
    },
    worktree_path: null,
    branch_name: 'kanban/task-1',
    stewardship: null,
    total_tokens: 1234,
    total_calls: 7,
    review_mode: 'agent',
    model_override: null,
    archived_at: null,
    stalled: false,
    executor_owner: null,
    created_at: '2026-09-16T00:00:00Z',
    updated_at: '2026-09-16T00:10:00Z',
  };
}

const CURSOR: BranchCursor = {
  cursor_id: 'c-merge',
  branch: 'main',
  stage: 'merge',
  node: 'execute',
  status: 'pending',
  validate_attempts: 0,
  skipped_to_join: false,
  pending_reason: pendingTask().pending_reason,
};

const MERGE_ACTIONS: AllowedAction[] = [
  { action: 'approve', kind: 'side_effect', label: '合入', cursor_id: 'c-merge' },
  { action: 'return', kind: 'side_effect', label: '返回修改', cursor_id: 'c-merge' },
];

/** 把 store 摆到「合入前等人」那一态。 */
function armPendingMerge(): void {
  mocks.detail.state = emptyTaskDetailState({
    task: pendingTask(),
    cursors: [CURSOR],
    allowedActions: MERGE_ACTIONS,
    pendingReason: pendingTask().pending_reason,
  });
  mocks.detail.diff = parseUnifiedDiff(DIFF_RAW);
  mocks.detail.diffRaw = DIFF_RAW;
  mocks.detail.loading = false;
}

const dossier = () => screen.getByRole('complementary', { name: '待办' });
/** diff 正文里的文件块标题（DiffView 的每个文件一个 h4）——用户看见的「一份 diff」。 */
const diffHeadings = (scope: HTMLElement | Document = document) =>
  within(scope as HTMLElement).queryAllByRole('heading', { level: 3, name: /lib\.js/ });

beforeAll(() => {
  // jsdom 不实现 matchMedia；详情页用它判断移动款（<480px），这里一律桌面档。
  if (typeof window.matchMedia !== 'function') {
    window.matchMedia = ((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
  }
});

afterEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '';
});

describe('任务详情 · 档案盒与 Diff 页签不同时摆两份 diff（票 08）', () => {
  it('停在 Diff 页签：屏上只有一份 diff，右栏动作行仍在（合入 / 返回修改）', async () => {
    armPendingMerge();
    render(TaskDetail, { props: { id: 'task-1' } });

    await fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));

    // 屏上只有一份 diff 正文：那一份在主区
    expect(diffHeadings()).toHaveLength(1);
    expect(diffHeadings(dossier())).toHaveLength(0);

    // 动作行是红线：右栏始终能拍板，不必先切页签回去
    expect(within(dossier()).getByRole('button', { name: '合入' })).toBeTruthy();
    expect(within(dossier()).getByRole('button', { name: '返回修改' })).toBeTruthy();
  });

  it('不在 Diff 页签：档案盒内嵌的 diff 按原样回来（既定设计，不动）', async () => {
    armPendingMerge();
    render(TaskDetail, { props: { id: 'task-1' } });

    // 默认落在时间线页签：主区是时间线，diff 只在右栏
    expect(diffHeadings(dossier())).toHaveLength(1);
    expect(within(dossier()).getByRole('button', { name: '合入' })).toBeTruthy();

    await fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
    expect(diffHeadings(dossier())).toHaveLength(0);

    await fireEvent.click(screen.getByRole('tab', { name: '时间线' }));
    expect(diffHeadings(dossier())).toHaveLength(1);
  });

  it('人工评审同理：diff 只在 Diff 页签里一份，报告与评审动作仍在右栏', async () => {
    armPendingMerge();
    const review = pendingTask();
    review.pending_reason = {
      type: 'human_review',
      stage: 'review',
      node: 'execute',
      message: '这一轮等人工评审。',
    };
    review.current_stage = 'review';
    mocks.detail.state = emptyTaskDetailState({
      task: review,
      cursors: [CURSOR],
      allowedActions: MERGE_ACTIONS,
      pendingReason: review.pending_reason,
    });

    render(TaskDetail, { props: { id: 'task-1' } });
    // 时间线页签：评审面板里的 diff 在右栏（主区是时间线）
    expect(diffHeadings(dossier())).toHaveLength(1);

    await fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
    expect(diffHeadings(dossier())).toHaveLength(0);
    expect(within(dossier()).getByRole('button', { name: '通过' })).toBeTruthy();
  });
});

describe('任务详情 · 任务级入口（票 06 / 07）', () => {
  it('指标入口指向该任务，参数名逐字 `task`', () => {
    armPendingMerge();
    render(TaskDetail, { props: { id: 'task-1' } });

    const link = screen.getByRole('link', { name: /这个任务的指标/ });
    expect(link.getAttribute('href')).toBe('#/metrics?task=task-1');
  });

  it('项目分析入口指向所属项目且带 `analyze=1`，参数名逐字', () => {
    armPendingMerge();
    render(TaskDetail, { props: { id: 'task-1' } });

    const link = screen.getByRole('link', { name: /分析所属项目/ });
    expect(link.getAttribute('href')).toBe('#/settings/projects?project=proj-9&analyze=1');
  });
});

describe('任务详情 · 页签语义与标题层级（票 06 / R2-19 / R2-20）', () => {
  it('页签是 tablist/tab + aria-selected + aria-controls，面板是 tabpanel', () => {
    armPendingMerge();
    render(TaskDetail, { props: { id: 'task-1' } });

    const tablist = screen.getByRole('tablist', { name: '任务详情页签' });
    const tabs = within(tablist).getAllByRole('tab');
    expect(tabs.map((t) => t.textContent?.trim())).toEqual([
      '时间线',
      '会话',
      '命令与输出0',
      '产出文件',
      'Diff',
    ]);
    // 选中态是给读屏的那一个属性，不只是 class
    expect(within(tablist).getByRole('tab', { name: '时间线' }).getAttribute('aria-selected')).toBe(
      'true',
    );
    expect(within(tablist).getByRole('tab', { name: 'Diff' }).getAttribute('aria-selected')).toBe(
      'false',
    );
    // aria-controls 指向真面板
    expect(screen.getByRole('tabpanel').id).toBe('detail-pane');
    expect(
      within(tablist).getByRole('tab', { name: '时间线' }).getAttribute('aria-controls'),
    ).toBe('detail-pane');
  });

  it('方向键在页签间走，焦点跟着选中项（Home/End 到两端）', async () => {
    armPendingMerge();
    render(TaskDetail, { props: { id: 'task-1' } });
    const tablist = screen.getByRole('tablist', { name: '任务详情页签' });

    await fireEvent.keyDown(tablist, { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: '会话' }).getAttribute('aria-selected')).toBe('true');

    await fireEvent.keyDown(tablist, { key: 'End' });
    expect(screen.getByRole('tab', { name: 'Diff' }).getAttribute('aria-selected')).toBe('true');

    await fireEvent.keyDown(tablist, { key: 'Home' });
    expect(screen.getByRole('tab', { name: '时间线' }).getAttribute('aria-selected')).toBe('true');
  });

  it('标题层级不断级：h1 之下有 h2，Diff 的文件块是 h3（不再是 h1 → h4）', async () => {
    armPendingMerge();
    render(TaskDetail, { props: { id: 'task-1' } });

    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(screen.getAllByRole('heading', { level: 2 }).length).toBeGreaterThan(0);

    await fireEvent.click(screen.getByRole('tab', { name: 'Diff' }));
    expect(screen.getAllByRole('heading', { level: 3, name: /lib\.js/ }).length).toBeGreaterThan(0);
    expect(screen.queryAllByRole('heading', { level: 4 })).toHaveLength(0);
  });
});

describe('任务详情 · 空态（票 13）', () => {
  it('任务不存在时说清状态与下一步，且不再自带一条回看板的链（决策 240）', async () => {
    mocks.detail.state = emptyTaskDetailState();
    mocks.detail.diff = null;
    mocks.detail.diffRaw = null;
    mocks.detail.loading = false;
    mocks.detail.error = null;
    mocks.detail.errorStatus = null;
    render(TaskDetail, { props: { id: 'nope' } });

    // 状态一行说清是什么（这一步是「没读到」，故说的是「没能打开」；下一步照旧给）
    expect(screen.getByText(/任务(不存在|没能打开)：nope/)).toBeTruthy();
    // 看板入口**只在顶栏那一枚页签上**（决策 240），本页不再代它递一次
    expect(screen.queryByRole('link', { name: /看板/ })).toBeNull();
  });
});

describe('任务详情 · 加载失败有出口（票 01 / R2-01）', () => {
  beforeEach(() => {
    mocks.detail.state = emptyTaskDetailState();
    mocks.detail.diff = null;
    mocks.detail.diffRaw = null;
    mocks.detail.loading = false;
  });

  it('失败态给一颗「重新加载」，点了真的重跑 load（首次加载失败不再是永久死页）', async () => {
    mocks.detail.error = 'Internal Server Error';
    mocks.detail.errorStatus = 500;
    render(TaskDetail, { props: { id: 'task-1' } });

    const again = screen.getByRole('button', { name: '重新加载' });
    await fireEvent.click(again);
    expect(mocks.detail.load).toHaveBeenCalledWith('task-1');
  });

  it('「404」说这个 id 没有，「其它失败」说没能打开——两种话不混用', () => {
    mocks.detail.error = '任务不存在：nope';
    mocks.detail.errorStatus = 404;
    render(TaskDetail, { props: { id: 'nope' } });
    // 空态与失败横幅各说一遍同一句（横幅那一份同时是 live region）
    expect(screen.getAllByText(/任务不存在：nope/).length).toBeGreaterThan(0);

    document.body.innerHTML = '';
    mocks.detail.error = 'Failed to fetch';
    mocks.detail.errorStatus = 0;
    render(TaskDetail, { props: { id: 'task-1' } });
    expect(screen.getByText(/任务没能打开：task-1/)).toBeTruthy();
    expect(screen.getByText(/Failed to fetch/)).toBeTruthy();
  });

  it('失败原因进 live region（票 02）：读屏听到的是那句话，不只是红颜色', () => {
    mocks.detail.error = 'Failed to fetch';
    mocks.detail.errorStatus = 0;
    render(TaskDetail, { props: { id: 'task-1' } });
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain('Failed to fetch');
  });

  it('加载中不摆失败态：先给「正在加载任务…」', () => {
    mocks.detail.loading = true;
    mocks.detail.error = null;
    render(TaskDetail, { props: { id: 'task-1' } });
    expect(screen.getByText(/正在加载任务/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: '重新加载' })).toBeNull();
  });
});
