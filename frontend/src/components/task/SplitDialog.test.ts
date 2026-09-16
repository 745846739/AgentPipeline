import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import SplitDialog from './SplitDialog.svelte';

/**
 * 拆分任务对话框（票 02 抽 `Modal` 之后的**行为不许破**）。
 *
 * 这个框的键盘与语义由 `e2e/modal-keyboard.spec.ts` 在真应用上断言；这里钉的是另外两件
 * 在 jsdom 里就能确定的事：① 它仍然是一个有名字的模态、原有内容还在；
 * ② **提交路径不变**——点「确认拆分」仍按「每行一个子任务、`标题 | 描述`」解析出子任务。
 */

describe('拆分任务对话框', () => {
  it('是有名字的模态，原有说明与输入框都还在', () => {
    render(SplitDialog, {
      props: { open: true, onclose: () => {}, onsubmit: () => {} },
    });
    const dialog = screen.getByRole('dialog', { name: '拆分任务' });
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(screen.getByRole('textbox')).toBeTruthy();
    // 正文说清动作与后果（不再出现内部编号）
    expect(dialog.textContent).toContain('每行一个子任务');
    expect(dialog.textContent).toContain('被取消');
  });

  it('提交路径不变：按行与 `|` 解析出子任务', async () => {
    const onsubmit = vi.fn();
    render(SplitDialog, { props: { open: true, onclose: () => {}, onsubmit } });

    await fireEvent.input(screen.getByRole('textbox'), {
      target: { value: '实现 A 部分 | 说明 A\n实现 B 部分' },
    });
    await fireEvent.click(screen.getByRole('button', { name: '确认拆分' }));

    expect(onsubmit).toHaveBeenCalledWith([
      { title: '实现 A 部分', description: '说明 A' },
      { title: '实现 B 部分', description: '' },
    ]);
  });

  it('没有子任务时不提交，取消仍然关得掉', async () => {
    const onsubmit = vi.fn();
    const onclose = vi.fn();
    render(SplitDialog, { props: { open: true, onclose, onsubmit } });

    await fireEvent.click(screen.getByRole('button', { name: '确认拆分' }));
    expect(onsubmit).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByRole('button', { name: '取消' }));
    expect(onclose).toHaveBeenCalledTimes(1);
  });
});
