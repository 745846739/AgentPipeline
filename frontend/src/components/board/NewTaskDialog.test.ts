import { fireEvent, render } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import NewTaskDialog from './NewTaskDialog.svelte';
import { board } from '../../stores/board.svelte';

/**
 * 「依赖任务 ID」的候选（票 05）。
 *
 * 口径：**原生候选项列表**（同一个输入框上挂 `datalist`，不做完整选择器），候选来自
 * **当前项目已有的任务**且能靠标题区分；手打 / 粘贴的路径原样保留。
 * 断言只落在用户看得见的东西上——输入框挂着的候选列表、候选里的 id 与标题、
 * 打进去的字还在（不落组件内部状态）。
 */

// `vi.mock` 会被提升到文件顶部，故替身数据用 `vi.hoisted` 一起提前建好（vitest 的既定写法）。
const TASKS = vi.hoisted(() => [
  { id: '01AAAAAAAAAAAAAAAAAAAAAAAA', project_id: 'p1', title: '把登录失败原因写进审计日志' },
  { id: '01BBBBBBBBBBBBBBBBBBBBBBBB', project_id: 'p1', title: '给导出命令加一个 --since 参数' },
  { id: '01CCCCCCCCCCCCCCCCCCCCCCCC', project_id: 'p2', title: '别的项目里的任务' },
]);

vi.mock('../../stores/board.svelte', () => ({
  board: {
    projectId: 'p1',
    projects: [
      { id: 'p1', name: '项目一' },
      { id: 'p2', name: '项目二' },
    ],
    tasks: TASKS,
    createTask: vi.fn(),
  },
}));

function dependsInput(container: HTMLElement): HTMLInputElement {
  const input = container.querySelector<HTMLInputElement>('input[list]');
  if (!input) throw new Error('依赖任务 ID 的输入框没有挂候选项列表（list 属性）');
  return input;
}

function optionsOf(container: HTMLElement, input: HTMLInputElement): HTMLOptionElement[] {
  const list = container.querySelector(`datalist#${input.getAttribute('list')}`);
  if (!list) throw new Error(`list 指向的 datalist 不存在：#${input.getAttribute('list')}`);
  return [...list.querySelectorAll('option')];
}

describe('新建任务 · 依赖任务 ID 的候选项列表（票 05）', () => {
  it('候选是当前项目已有的任务，靠标题区分，选中即填好 id', async () => {
    const { container } = render(NewTaskDialog, { props: { open: true, onclose: () => {} } });
    const input = dependsInput(container);
    const options = optionsOf(container, input);

    expect(options.map((o) => o.value)).toEqual(['01AAAAAAAAAAAAAAAAAAAAAAAA', '01BBBBBBBBBBBBBBBBBBBBBBBB']);
    expect(options.map((o) => o.textContent?.trim())).toEqual([
      '把登录失败原因写进审计日志',
      '给导出命令加一个 --since 参数',
    ]);

    // 选中候选项 = 填好该任务的 ID（原生 datalist 的行为就是把 value 填进输入框）
    await fireEvent.input(input, { target: { value: options[0].value } });
    expect(input.value).toBe('01AAAAAAAAAAAAAAAAAAAAAAAA');
  });

  it('别的项目的任务不进候选（候选是「当前项目」的）', async () => {
    const { container } = render(NewTaskDialog, { props: { open: true, onclose: () => {} } });
    const values = optionsOf(container, dependsInput(container)).map((o) => o.value);
    expect(values).not.toContain('01CCCCCCCCCCCCCCCCCCCCCCCC');
  });

  it('手打 / 粘贴多个逗号分隔的 id 仍然成立', async () => {
    const { container } = render(NewTaskDialog, { props: { open: true, onclose: () => {} } });
    const input = dependsInput(container);
    await fireEvent.input(input, {
      target: { value: ' 01AAAAAAAAAAAAAAAAAAAAAAAA ,01BBBBBBBBBBBBBBBBBBBBBBBB,' },
    });
    expect(input.value).toBe(' 01AAAAAAAAAAAAAAAAAAAAAAAA ,01BBBBBBBBBBBBBBBBBBBBBBBB,');
  });

  it('Escape 关得掉（票 02：焦点没进过框也算）', async () => {
    const onclose = vi.fn();
    render(NewTaskDialog, { props: { open: true, onclose } });
    (document.activeElement as HTMLElement | null)?.blur();
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(onclose).toHaveBeenCalledTimes(1);
  });
});

/**
 * 描述必填（票 05①）：与后端 `POST /tasks` 同一关——空描述的任务会让 architect 靠翻仓库
 * 猜范围。前端先拦一道，用户不必撞到 400 才知道。
 */
describe('新建任务 · 描述必填（票 05①）', () => {
  beforeEach(() => vi.clearAllMocks());

  it('描述为空时不提交，错落在描述那一格；填上就放行', async () => {
    const { container } = render(NewTaskDialog, { props: { open: true, onclose: () => {} } });
    const title = container.querySelector<HTMLInputElement>('input:not([list])');
    const description = container.querySelector<HTMLTextAreaElement>('textarea');
    if (!title || !description) throw new Error('新建任务对话框少了标题或描述控件');

    await fireEvent.input(title, { target: { value: '有标题' } });
    await fireEvent.submit(container.querySelector('form')!);
    expect(board.createTask).not.toHaveBeenCalled();
    expect(container.querySelector('#new-task-error')?.textContent).toContain('描述');
    expect(description.getAttribute('aria-invalid')).toBe('true');

    await fireEvent.input(description, { target: { value: '要实现的东西' } });
    await fireEvent.submit(container.querySelector('form')!);
    expect(board.createTask).toHaveBeenCalledTimes(1);
  });
});
