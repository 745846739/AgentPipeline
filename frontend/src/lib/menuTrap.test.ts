import { describe, expect, it } from 'vitest';

import { closeOnOutsideClick, decideMenuKey, wrapIndex } from './menuTrap';

/**
 * ⋯ 班次菜单 / 待处理下拉共用的键盘陷阱（决策 251⑤）。
 *
 * 这两处原来是**同义的两份**（`routes/Talk.svelte` 与 `components/layout/TopBar.svelte`），
 * 各 80 多行、换了标识符而已，而 Talk 那份**一条单测都没有**——只有 e2e 盖住一半
 * （盖不到 ArrowUp 回触发钮 / Home / End / 绕回）。判据收在这里，两处共用。
 *
 * 这里钉的是**陷阱本身**，不是接线：接线由 `TopBar.test.ts` 黑盒验（它不经这些函数，
 * 直接在 `window` 上派发事件、断言 `document.activeElement`），且**必须原样通过**。
 */
describe('索引绕回（wrapIndex）', () => {
  it('越界绕回，且与焦点移动用的那条公式逐字同形', () => {
    expect(wrapIndex(0, 3)).toBe(0);
    expect(wrapIndex(2, 3)).toBe(2);
    expect(wrapIndex(3, 3)).toBe(0); // 末项再往下 → 回第一项
    expect(wrapIndex(4, 3)).toBe(1);
    expect(wrapIndex(-1, 3)).toBe(2); // 负数不落进 JS 余数的负值坑
  });

  it('长度为 0 不除零：退回 0（调用方在此之前已短路，这里是兜底）', () => {
    expect(wrapIndex(0, 0)).toBe(0);
    expect(wrapIndex(3, 0)).toBe(0);
    expect(wrapIndex(-3, 0)).toBe(0);
  });
});

describe('一个键该做什么（decideMenuKey）', () => {
  const base = { open: true, onTrigger: false, inPanel: true, current: 0, count: 3 };

  /** 焦点在第一项上时往下走 → 第二项。 */
  it('面板内 ArrowDown 走到下一项', () => {
    expect(decideMenuKey('ArrowDown', { ...base, current: 0 })).toEqual({
      action: 'focusItem',
      index: 1,
      preventDefault: true,
    });
  });

  it('末项再 ArrowDown 绕回第一项', () => {
    expect(decideMenuKey('ArrowDown', { ...base, current: 2 })).toEqual({
      action: 'focusItem',
      index: 0,
      preventDefault: true,
    });
  });

  it('**ArrowUp 从第一项回触发钮**——不绕到末项（这条是 Talk 那份 e2e 盖不到的）', () => {
    expect(decideMenuKey('ArrowUp', { ...base, current: 0 })).toEqual({
      action: 'focusTrigger',
      preventDefault: true,
    });
  });

  it('ArrowUp 在中间项往上走一项', () => {
    expect(decideMenuKey('ArrowUp', { ...base, current: 2 })).toEqual({
      action: 'focusItem',
      index: 1,
      preventDefault: true,
    });
  });

  it('Home / End 落首末项', () => {
    expect(decideMenuKey('Home', base)).toEqual({ action: 'focusItem', index: 0, preventDefault: true });
    expect(decideMenuKey('End', base)).toEqual({ action: 'focusItem', index: 2, preventDefault: true });
  });

  it('current 是 -1（焦点在触发钮或面板本身、不在任一项上）按「第一项之前」处理', () => {
    // ArrowDown → 第一项；ArrowUp → 回触发钮（与 current: 0 同一处置）
    expect(decideMenuKey('ArrowDown', { ...base, current: -1 })).toEqual({
      action: 'focusItem',
      index: 0,
      preventDefault: true,
    });
    expect(decideMenuKey('ArrowUp', { ...base, current: -1 })).toEqual({
      action: 'focusTrigger',
      preventDefault: true,
    });
  });

  it('**焦点没进过面板时 Escape 也关得掉**，且那时不还焦点', () => {
    const d = decideMenuKey('Escape', { ...base, onTrigger: false, inPanel: false });
    expect(d).toEqual({ action: 'close', returnFocus: false, preventDefault: false });
  });

  it('焦点在触发钮或面板里时 Escape 关并把焦点还回去', () => {
    expect(decideMenuKey('Escape', { ...base, onTrigger: true, inPanel: false })).toEqual({
      action: 'close',
      returnFocus: true,
      preventDefault: false,
    });
    expect(decideMenuKey('Escape', { ...base, onTrigger: false, inPanel: true })).toEqual({
      action: 'close',
      returnFocus: true,
      preventDefault: false,
    });
  });

  it('**关着时按 Escape 不关**（否则开了才关得掉的语义就反了）', () => {
    expect(decideMenuKey('Escape', { ...base, open: false }).action).toBe('ignore');
  });

  it('关着时在触发钮上按 ArrowDown：打开并送焦点进第一项', () => {
    expect(decideMenuKey('ArrowDown', { ...base, open: false, onTrigger: true })).toEqual({
      action: 'open',
      preventDefault: true,
    });
  });

  it('开着时在触发钮上按 ArrowDown：进面板第一项（不是再开一次）', () => {
    expect(decideMenuKey('ArrowDown', { ...base, open: true, onTrigger: true, current: -1 })).toEqual({
      action: 'focusItem',
      index: 0,
      preventDefault: true,
    });
  });

  it('焦点既不在触发钮也不在面板里 → 什么都不做（别在页面别处按方向键就乱跳）', () => {
    for (const key of ['ArrowDown', 'ArrowUp', 'Home', 'End']) {
      expect(
        decideMenuKey(key, { ...base, onTrigger: false, inPanel: false }).action,
        key,
      ).toBe('ignore');
    }
  });

  it('没有可聚焦项时方向键不动（Home / End 也不动）', () => {
    for (const key of ['ArrowDown', 'ArrowUp', 'Home', 'End']) {
      expect(decideMenuKey(key, { ...base, count: 0 }).action, key).toBe('ignore');
    }
  });

  it('不认识的键不动、不 preventDefault（Tab / Enter 等照浏览器默认走）', () => {
    for (const key of ['Tab', 'Enter', 'a']) {
      expect(decideMenuKey(key, base), key).toEqual({
        action: 'ignore',
        preventDefault: false,
      });
    }
  });
});

describe('点外面关（closeOnOutsideClick）', () => {
  const wrap = document.createElement('div');
  const inside = document.createElement('button');
  const outside = document.createElement('button');
  wrap.appendChild(inside);
  document.body.appendChild(wrap);

  it('点面板里面不关', () => {
    expect(closeOnOutsideClick({ open: true, wrap, target: inside })).toBe(false);
  });

  it('点面板外面关', () => {
    expect(closeOnOutsideClick({ open: true, wrap, target: outside })).toBe(true);
  });

  it('本来就关着 → 不动作', () => {
    expect(closeOnOutsideClick({ open: false, wrap, target: outside })).toBe(false);
  });

  it('target 是 null（事件目标已卸载）→ 当成点在外面，关掉', () => {
    expect(closeOnOutsideClick({ open: true, wrap, target: null })).toBe(true);
  });
});
