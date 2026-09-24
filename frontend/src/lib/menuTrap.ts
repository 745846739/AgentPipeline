import { tick } from 'svelte';

/**
 * 下拉弹层的键盘陷阱与点外关闭（决策 251⑤）。
 *
 * `routes/Talk.svelte` 的 ⋯ 班次菜单与 `components/layout/TopBar.svelte` 的「待处理」下拉
 * 原来是**同义的两份**（各 80 多行、只换了标识符），而 Talk 那份一条单测都没有——
 * 只有 e2e 盖住一半（盖不到 ArrowUp 回触发钮 / Home / End / 绕回）。两处共用这一份。
 *
 * **为什么不是 Svelte `use:` action**：全仓一个 `use:` 都没有（`grep 'use:' frontend/src/`
 * 零命中，也没有任何文档表态），不在一次去重里首次引入一种团队从未用过的模式。
 *
 * **为什么只产 handler、不自己挂监听**：两处本来就用 `<svelte:window onclick onkeydown>`
 * 在模板里收（那是「键盘一律在 window 上收」这条 a11y 立场的可见落点）。改成在
 * `$effect` 里挂/卸监听会：① 每次开关状态变化都重挂一遍；② 把那条立场从模板挪进脚本。
 *
 * **判据与接线分开**：`decideMenuKey` / `wrapIndex` / `closeOnOutsideClick` 是纯函数、
 * 本文件里的单测钉的是**陷阱本身**；接线由 `TopBar.test.ts` 黑盒验（它不经这些函数，
 * 直接在 `window` 上派发事件、断言 `document.activeElement`），那份测试**必须原样通过**。
 */

/**
 * 焦点绕回。与两处原实现逐字同形（`((i % n) + n) % n`），差别只在**长度为 0 时不除零**——
 * 调用方在此之前已短路，这里是兜底：除零会让焦点下标变成 `NaN`，`list[NaN]` 是 `undefined`
 * 而 `.focus()` 会在它上面抛。
 */
export function wrapIndex(index: number, length: number): number {
  if (length <= 0) return 0;
  return ((index % length) + length) % length;
}

/**
 * 一次按键的处置。
 *
 * `preventDefault` 与 `action` 绑在一起给出——顺序是承重的：先 `preventDefault` 再动焦点
 * （`ArrowDown` 在触发钮上若不先拦默认行为，浏览器会自己滚页面 / 移焦点，与我们送的焦点打架）。
 */
export type MenuKeyDecision =
  | { action: 'ignore'; preventDefault: false }
  | { action: 'open'; preventDefault: true }
  | { action: 'close'; returnFocus: boolean; preventDefault: false }
  | { action: 'focusItem'; index: number; preventDefault: true }
  | { action: 'focusTrigger'; preventDefault: true };

export interface MenuKeyContext {
  /** 面板是否开着。 */
  open: boolean;
  /** 焦点是否落在触发钮上。 */
  onTrigger: boolean;
  /** 焦点是否落在面板里（含面板本身与任一项）。 */
  inPanel: boolean;
  /**
   * `activeElement` 在项列表里的下标；**不在其中时是 -1**（焦点在触发钮、面板本身，
   * 或页面别处）。-1 要按「第一项之前」处置，否则 ArrowUp 会绕到末项、ArrowDown 会漏掉第一项。
   */
  current: number;
  /** 可聚焦项的数量（0 = 什么都不动）。 */
  count: number;
}

/**
 * 一个键该做什么。
 *
 * 三条陷阱写死在这里，两处都由它决定：
 * ① **关着时按 Escape 不关**（否则「开了才关得掉」的语义就反了）；
 * ② **焦点没进过面板时 Escape 也关得掉**、且那时**不**把焦点还回去（焦点不在这条链上，
 *    还回去反而把人的光标抢走）；
 * ③ **ArrowUp 从第一项回触发钮**（不是绕到末项）——这是键盘陷阱里最常被抄漏的一条。
 *
 * **判据的顺序即语义**：`ArrowDown && onTrigger && !open` 必须排在 `!open` 之前，
 * 否则「触发钮上按方向键打开」永远够不着。
 */
export function decideMenuKey(key: string, ctx: MenuKeyContext): MenuKeyDecision {
  const { open, onTrigger, inPanel, current, count } = ctx;
  if (key === 'ArrowDown' && onTrigger && !open) {
    return { action: 'open', preventDefault: true };
  }
  if (!open) return { action: 'ignore', preventDefault: false };
  if (key === 'Escape') {
    return { action: 'close', returnFocus: onTrigger || inPanel, preventDefault: false };
  }
  if (!onTrigger && !inPanel) return { action: 'ignore', preventDefault: false };
  if (count === 0) return { action: 'ignore', preventDefault: false };
  if (key === 'ArrowDown') {
    return { action: 'focusItem', index: wrapIndex(current + 1, count), preventDefault: true };
  }
  if (key === 'ArrowUp') {
    if (current <= 0) return { action: 'focusTrigger', preventDefault: true };
    return { action: 'focusItem', index: wrapIndex(current - 1, count), preventDefault: true };
  }
  if (key === 'Home') {
    return { action: 'focusItem', index: wrapIndex(0, count), preventDefault: true };
  }
  if (key === 'End') {
    return { action: 'focusItem', index: wrapIndex(count - 1, count), preventDefault: true };
  }
  return { action: 'ignore', preventDefault: false };
}

/**
 * 点外面关不关。`target` 是 `null`（事件目标已卸载）**当点在外面**：那种时刻面板多半正在
 * 被拆掉，留着开态只会让下一次点击以为它还开着。
 */
export function closeOnOutsideClick(ctx: {
  open: boolean;
  wrap: Element | null;
  target: Node | null;
}): boolean {
  if (!ctx.open) return false;
  if (ctx.target && ctx.wrap && ctx.wrap.contains(ctx.target)) return false;
  return true;
}

export interface MenuTrapOptions {
  /** 面板当前是否开着。 */
  isOpen: () => boolean;
  /**
   * 「键盘要打开时」做什么。**与点击打开不必同一条**：TopBar 这里走
   * `togglePendingDropdown()`（顺带拉一次待办列表），而它的点击打开是模板里另一处直调。
   */
  onOpen: () => void;
  /** 关闭（**不**还焦点——还焦点由 {@link MenuTrap.close} 按需做）。 */
  onClose: () => void;
  trigger: () => HTMLElement | null;
  panel: () => HTMLElement | null;
  /** 包住触发钮与面板的容器：点它**里面**不算「点外面」。 */
  wrap: () => Element | null;
  /**
   * 项选择器。**两处不同且不该强行统一**：Talk 是
   * `button[data-menu-item]:not([disabled])`（有禁用项，禁用的要跳过），TopBar 是 `a.dd-item`
   * （全是链接、没有禁用态）。把选择器收成一条会丢掉其中一边的判据。
   */
  itemSelector: string;
}

/**
 * 造一份弹层的键盘/点外处理器，连同开合两个动作，交给模板的 `<svelte:window>` 收。
 *
 * 返回的是**函数**而不是注册器——见文件头「为什么只产 handler」。
 */
export function createMenuTrap(opts: MenuTrapOptions) {
  const items = (): HTMLElement[] => {
    const panel = opts.panel();
    return panel ? [...panel.querySelectorAll<HTMLElement>(opts.itemSelector)] : [];
  };

  const focusItem = (index: number): void => {
    const list = items();
    if (list.length === 0) return;
    list[wrapIndex(index, list.length)].focus();
  };

  const focusTrigger = (): void => {
    opts.trigger()?.focus();
  };

  /** 打开 + 送焦点进第一项。**面板刚变可见，要等一次 DOM 刷新**（故 `tick`）。 */
  function open(): void {
    opts.onOpen();
    void tick().then(() => focusItem(0));
  }

  /** 关闭；`returnFocus` 决定要不要把焦点还给触发钮。 */
  function close(returnFocus: boolean): void {
    opts.onClose();
    if (returnFocus) focusTrigger();
  }

  function onKeydown(e: KeyboardEvent): void {
    const active = document.activeElement as HTMLElement | null;
    const trigger = opts.trigger();
    const panel = opts.panel();
    const list = items();
    const decision = decideMenuKey(e.key, {
      open: opts.isOpen(),
      onTrigger: !!trigger && active === trigger,
      inPanel: !!active && !!panel && panel.contains(active),
      current: list.indexOf(active as HTMLElement),
      count: list.length,
    });
    if (decision.preventDefault) e.preventDefault();
    switch (decision.action) {
      case 'open':
        open();
        break;
      case 'close':
        close(decision.returnFocus);
        break;
      case 'focusItem':
        focusItem(decision.index);
        break;
      case 'focusTrigger':
        focusTrigger();
        break;
      case 'ignore':
        break;
    }
  }

  function onClick(e: MouseEvent): void {
    if (
      closeOnOutsideClick({ open: opts.isOpen(), wrap: opts.wrap(), target: e.target as Node | null })
    ) {
      opts.onClose();
    }
  }

  return { onKeydown, onClick, open, close };
}
