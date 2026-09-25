import { describe, expect, it } from 'vitest';

import { dockRows, growTextarea, TALK_DOCK_CAP_ROWS } from './talkDock';

describe('坞自长的行数判据（决策 282 ③）', () => {
  it('空：1 行起步（不随占位语折行涨）', () => {
    expect(dockRows('')).toBe(1);
  });

  it('单行：仍是 1 行', () => {
    expect(dockRows('对值班长说一句话')).toBe(1);
  });

  it('多行：随硬换行自长', () => {
    expect(dockRows('第一行\n第二行')).toBe(2);
    expect(dockRows('一\n二\n三\n四')).toBe(4);
  });

  it('封顶数可核：判据封在 TALK_DOCK_CAP_ROWS', () => {
    const ten = Array.from({ length: 10 }, (_, i) => `第${i + 1}行`).join('\n');
    expect(dockRows(ten)).toBe(TALK_DOCK_CAP_ROWS);
    expect(dockRows('一\n二\n三\n四\n五\n六\n七')).toBe(6);
  });

  it('换行归零：删掉换行后档位跟着落回去', () => {
    const text = '一\n二\n三';
    expect(dockRows(text)).toBe(3);
    expect(dockRows(text.replaceAll('\n', ''))).toBe(1);
    expect(dockRows('')).toBe(1);
  });
});

describe('接线的量测校正（硬换行与空值路径；软换行由 e2e 覆盖）', () => {
  /** jsdom 里没有真排版（scrollHeight 恒 0），只钉得住「档位拨到哪一行」。 */
  function field(value: string): HTMLTextAreaElement {
    const el = document.createElement('textarea');
    el.value = value;
    document.body.appendChild(el);
    return el;
  }

  it('空值：拨到 1 行且不进入量测（占位语折行不算内容）', () => {
    const el = field('');
    growTextarea(el);
    expect(el.rows).toBe(1);
  });

  it('硬换行：档位与 dockRows 一致', () => {
    const el = field('一\n二\n三');
    growTextarea(el);
    expect(el.rows).toBe(3);
  });

  it('超封顶：停在封顶行', () => {
    const el = field(Array.from({ length: 9 }, () => '行').join('\n'));
    growTextarea(el);
    expect(el.rows).toBe(TALK_DOCK_CAP_ROWS);
  });

  it('收缩：从大档位拨回小档位（换行归零的接线面）', () => {
    const el = field('一\n二\n三');
    growTextarea(el);
    expect(el.rows).toBe(3);
    el.value = '只剩一句';
    growTextarea(el);
    expect(el.rows).toBe(1);
  });
});
