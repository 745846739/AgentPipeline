import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import Modal from './Modal.svelte';

/**
 * 对话框形状的契约（UX 审计票 02）。
 *
 * 三个对话框（新建任务 / 拆分任务 / 更换长上下文模型）现在共用这一份形状，所以这里钉的是
 * 形状本身：**打开时焦点在第一个输入框**、**Escape 一律可关（焦点没进过框也算）**、
 * **被当作一个有名字的模态播报**、**取消 / 点遮罩的鼠标路径不变**。
 *
 * 边界（诚实说明）：Tab 的**环绕**与「焦点不跑到背后页面」这两条依赖真实布局
 * （`focusables()` 按元素是否真的排了版来筛），jsdom 不做布局，故那两条只在
 * `e2e/modal-keyboard.spec.ts` 里断言，这里不假装覆盖。
 */

/** 表单内容：真实调用点各自内联自己的字段，这里只拼一小段来验证 Modal 的契约。 */
const children = createRawSnippet(() => ({
  render: () => '<div><label for="probe">标题</label><input id="probe" /></div>',
}));

function baseProps(overrides: Record<string, unknown> = {}) {
  return {
    open: true,
    title: '新建任务',
    submitLabel: '创建并启动',
    children,
    ...overrides,
  };
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('Modal：对话框的唯一形状（票 02）', () => {
  it('关着的时候屏上什么都没有', () => {
    render(Modal, { props: { ...baseProps({ open: false }), onclose: () => {}, onsubmit: () => {} } });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('读屏播报的是一个有名字的模态对话框（不是一段无名的排版）', () => {
    render(Modal, { props: { ...baseProps(), onclose: () => {}, onsubmit: () => {} } });
    const dialog = screen.getByRole('dialog', { name: '新建任务' });
    expect(dialog.getAttribute('aria-modal')).toBe('true');
  });

  it('打开时焦点就在第一个输入框里（不需要先点一下）', async () => {
    render(Modal, { props: { ...baseProps(), onclose: () => {}, onsubmit: () => {} } });
    const input = screen.getByLabelText('标题');
    await waitFor(() => expect(document.activeElement).toBe(input));
  });

  it('Escape 关得掉——即使焦点从没进过对话框', async () => {
    const onclose = vi.fn();
    render(Modal, { props: { ...baseProps(), onclose, onsubmit: () => {} } });
    // 焦点没进过对话框（改动前正是这一条不成立：Escape 挂在遮罩上，没人接）
    (document.activeElement as HTMLElement | null)?.blur();
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it('鼠标路径不变：点遮罩关、点框内不关、取消关', async () => {
    const onclose = vi.fn();
    const { container } = render(Modal, { props: { ...baseProps(), onclose, onsubmit: () => {} } });

    await fireEvent.click(screen.getByLabelText('标题'));
    expect(onclose).not.toHaveBeenCalled();

    const overlay = container.querySelector('div.overlay') as HTMLElement;
    await fireEvent.click(overlay);
    expect(onclose).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByRole('button', { name: '取消' }));
    expect(onclose).toHaveBeenCalledTimes(2);
  });

  it('提交按钮是表单的 submit，文案由调用点给；禁用条件（提交中 / 额外条件）生效', () => {
    render(Modal, {
      props: { ...baseProps({ submitDisabled: true }), onclose: () => {}, onsubmit: () => {} },
    });
    const submit = screen.getByRole('button', { name: '创建并启动' }) as HTMLButtonElement;
    expect(submit.type).toBe('submit');
    expect(submit.disabled).toBe(true);
  });

  it('表单内容由调用点内联渲染在标题下', () => {
    render(Modal, { props: { ...baseProps(), onclose: () => {}, onsubmit: () => {} } });
    expect(screen.getByText('标题')).toBeTruthy();
    expect(screen.getByLabelText('标题')).toBeTruthy();
  });
});
