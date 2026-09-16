import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { Provider } from '../../api/types';
import ModelOverrideDialog from './ModelOverrideDialog.svelte';

/**
 * 更换长上下文模型对话框（票 02 抽 `Modal` 之后的**行为不许破**）。
 *
 * 键盘与语义的真应用断言在 `e2e/modal-keyboard.spec.ts`；这里钉 jsdom 里能确定的三件：
 * ① 有名字的模态；② 打开时焦点进框内第一个控件（这里只有一个下拉）；
 * ③ **提交路径不变**——点「应用」仍把选中的 provider id 交给调用点，没有 provider 时按钮禁用。
 */

const PROVIDERS: Provider[] = [
  {
    id: 'pv-1',
    vendor: 'openai',
    model: 'mock',
    context_window: 8000,
    base_url: 'http://127.0.0.1:1',
    api_key: '***',
    enabled: true,
  },
  {
    id: 'pv-2',
    vendor: 'anthropic',
    model: 'long-ctx',
    context_window: 200000,
    base_url: 'http://127.0.0.1:2',
    api_key: '***',
    enabled: true,
  },
];

describe('更换长上下文模型对话框', () => {
  it('是有名字的模态，打开时焦点在框内第一个控件（下拉）', async () => {
    render(ModelOverrideDialog, {
      props: { open: true, providers: PROVIDERS, onclose: () => {}, onsubmit: () => {} },
    });
    const dialog = screen.getByRole('dialog', { name: '更换长上下文模型' });
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('combobox')));
  });

  it('提交路径不变：把选中的 provider id 交给调用点', async () => {
    const onsubmit = vi.fn();
    render(ModelOverrideDialog, {
      props: { open: true, providers: PROVIDERS, onclose: () => {}, onsubmit },
    });

    // 默认选中第一个可用 provider
    await fireEvent.click(screen.getByRole('button', { name: '应用' }));
    expect(onsubmit).toHaveBeenCalledWith('pv-1');

    // 换一个再提交 → 交出去的就是换的那个
    await fireEvent.change(screen.getByRole('combobox'), { target: { value: 'pv-2' } });
    await fireEvent.click(screen.getByRole('button', { name: '应用' }));
    expect(onsubmit).toHaveBeenLastCalledWith('pv-2');
  });

  it('没有 provider 时按钮禁用并给出下一步（而不是静默）', async () => {
    const onsubmit = vi.fn();
    render(ModelOverrideDialog, {
      props: { open: true, providers: [], onclose: () => {}, onsubmit },
    });
    const submit = screen.getByRole('button', { name: '应用' }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    expect(screen.getByRole('dialog').textContent).toContain('模型与密钥');

    await fireEvent.click(submit);
    expect(onsubmit).not.toHaveBeenCalled();
  });

  it('调用点给的错误照旧上屏，取消仍然关得掉', async () => {
    const onclose = vi.fn();
    render(ModelOverrideDialog, {
      props: {
        open: true,
        providers: PROVIDERS,
        error: 'provider 已被禁用',
        onclose,
        onsubmit: () => {},
      },
    });
    expect(screen.getByRole('dialog').textContent).toContain('provider 已被禁用');

    await fireEvent.click(screen.getByRole('button', { name: '取消' }));
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it('Escape 关得掉（焦点没进过框也算）', async () => {
    const onclose = vi.fn();
    render(ModelOverrideDialog, {
      props: { open: true, providers: PROVIDERS, onclose, onsubmit: () => {} },
    });
    (document.activeElement as HTMLElement | null)?.blur();
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(onclose).toHaveBeenCalledTimes(1);
  });
});
