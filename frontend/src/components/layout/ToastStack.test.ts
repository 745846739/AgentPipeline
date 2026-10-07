import { fireEvent, render, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { notifications } from '../../stores/notifications.svelte';
import ToastStack from './ToastStack.svelte';

/**
 * toast 的 Escape 关闭路径（票 05 / ux-audit-3）。
 *
 * 审计票面记的现状是「有关闭钮、Tab/Enter 可达，但全文件 `grep Escape` = 0 命中」，
 * 且运行时探针 `[r3] ⑤.1` 因 harness 没弹 toast 而标「未验证」——这一组把那条路径
 * 钉在组件级：**Escape 按焦点归属分派，只关焦点所在的那一条**。
 *
 * 归属口径（设计 §0 裁决 ②）：焦点不在 `.toasts` 内 → 这条 Escape 不归 toast 管，
 * 一个字节都不动——对话框（`Modal.svelte` open 才关）、菜单（`menuTrap.ts` 开着才关）、
 * 决策 216④ 确认态各有自己的「有主的 Escape」，给 toast 一条全局 Escape 就是
 * 「无主的 Escape」，必然与有主的双发。
 *
 * 用 `failed` 类推 toast：它在 `ALWAYS_ANNOUNCED` 里（免免打扰、免 cooldown）——
 * 换 `done` 类会在 22–8 点被 quiet hours 吞掉、5 分钟内被 cooldown 拦掉，
 * `notifications.test.ts` 已记过这个坑。
 */

/** 推一条 toast，返回它的 id（id 由 store 内部递增，测试不猜数）。 */
function pushFailedToast(title: string, message?: string): number {
  expect(notifications.notify('failed', { title, message })).toBe(true);
  const toast = notifications.toasts.at(-1);
  expect(toast).toBeDefined();
  return toast!.id;
}

/** 屏上第 id 条 toast 的根元素。 */
function toastEl(id: number): Element | null {
  return document.querySelector(`.toast[data-toast-id="${id}"]`);
}

/** 第 id 条 toast 的关闭钮。 */
function closeBtn(id: number): HTMLElement {
  const btn = toastEl(id)?.querySelector('button.close') as HTMLElement | null;
  expect(btn, `toast ${id} 的关闭钮应当在`).not.toBeNull();
  return btn!;
}

afterEach(() => {
  notifications.clear();
  document.body.innerHTML = '';
});

describe('toast 的 Escape 关闭路径（票 05 / ux-audit-3）', () => {
  it('焦点在关闭钮上按 Escape：该条从 DOM 与 notifications.toasts 同步消失', async () => {
    const id = pushFailedToast('任务甲 · failed', '合入被拒');
    render(ToastStack);
    expect(toastEl(id)).not.toBeNull();

    const close = closeBtn(id);
    close.focus();
    expect(document.activeElement).toBe(close);

    await fireEvent.keyDown(window, { key: 'Escape' });

    await waitFor(() => expect(toastEl(id)).toBeNull());
    expect(notifications.toasts.some((t) => t.id === id)).toBe(false);
  });

  it('焦点在 toast 外（document.body）按 Escape：toast 一个字节都不动', async () => {
    const id = pushFailedToast('任务乙 · failed', '流断了');
    render(ToastStack);
    // 焦点不在 .toasts 内——这条 Escape 归对话框 / 菜单 / 确认态，不归 toast
    (document.activeElement as HTMLElement | null)?.blur();
    expect(document.activeElement).toBe(document.body);

    await fireEvent.keyDown(window, { key: 'Escape' });

    expect(toastEl(id), '焦点不在 toast 里时 Escape 不该关它').not.toBeNull();
    expect(notifications.toasts.some((t) => t.id === id)).toBe(true);
  });

  it('两条 toast 时只关焦点所在的那一条', async () => {
    const first = pushFailedToast('任务甲 · failed');
    const second = pushFailedToast('任务乙 · failed');
    render(ToastStack);
    expect(toastEl(first)).not.toBeNull();
    expect(toastEl(second)).not.toBeNull();

    closeBtn(second).focus();
    await fireEvent.keyDown(window, { key: 'Escape' });

    await waitFor(() => expect(toastEl(second)).toBeNull());
    // 另一条原样在（`data-toast-id` 定位，不吃「第 N 条」的歧义）
    expect(toastEl(first), '不该连坐关掉另一条').not.toBeNull();
    expect(notifications.toasts.map((t) => t.id)).toEqual([first]);
  });

  it('没有 toast 时按 Escape 不抛错', async () => {
    render(ToastStack);
    expect(notifications.toasts).toHaveLength(0);

    // handler 抛错会在这里直接冒出来（守卫第一句就是 `toasts.length === 0` → return）
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(notifications.toasts).toHaveLength(0);
  });

  it('关闭钮的点击路径回归照旧：点一下就关', async () => {
    const id = pushFailedToast('任务丙 · failed');
    render(ToastStack);

    await fireEvent.click(closeBtn(id));

    await waitFor(() => expect(toastEl(id)).toBeNull());
    expect(notifications.toasts.some((t) => t.id === id)).toBe(false);
  });

  it('Escape 不误伤 Enter 等其它键（守卫先看 key）', async () => {
    const id = pushFailedToast('任务丁 · failed');
    render(ToastStack);

    closeBtn(id).focus();
    await fireEvent.keyDown(window, { key: 'Enter' });

    expect(toastEl(id), 'Enter 不该走 Escape 的关闭路径').not.toBeNull();
    expect(notifications.toasts.some((t) => t.id === id)).toBe(true);
  });
});
