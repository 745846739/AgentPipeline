import { describe, expect, it } from 'vitest';

import type { AllowedAction } from '../api/types';
import {
  defaultOpenStop,
  isFoldable,
  isStopOpen,
  resolveOpenStop,
  stopActionCount,
  toggleOpenStop,
} from './talkStops';

describe('状态区急停折叠（决策 183）', () => {
  it('一张急停不折叠：单急停的版面与折叠前逐像素一致', () => {
    expect(isFoldable([])).toBe(false);
    expect(isFoldable(['only'])).toBe(false);
    expect(isFoldable(['a', 'b'])).toBe(true);
  });

  it('两张以上一张都不展开：展开一张就占满 13″ 上的状态区，第二张照样掉出第一屏', () => {
    expect(defaultOpenStop(['a', 'b', 'c'])).toBe(null);
    expect(resolveOpenStop(['a', 'b', 'c'], undefined)).toBe(null);
  });

  it('只有一张时默认展开的就是它自己', () => {
    expect(defaultOpenStop(['only'])).toBe('only');
    expect(resolveOpenStop(['only'], undefined)).toBe('only');
  });

  it('没有急停时没有展开项', () => {
    expect(resolveOpenStop([], undefined)).toBe(null);
    expect(resolveOpenStop([], 'gone')).toBe(null);
  });

  it('人显式收起后保持收起，不自动弹回', () => {
    expect(resolveOpenStop(['a', 'b'], null)).toBe(null);
    // 纯函数层面单张也一样（收起就是收起）；但组件对单张**不提供**收起钮，
    // 且展开判据 `isStopOpen` 对单张恒为真——见下一条
    expect(resolveOpenStop(['only'], null)).toBe(null);
  });

  it('展开判据：只有一张时恒展开，多张时只有那唯一一张', () => {
    expect(isStopOpen(false, null, 'only')).toBe(true);
    expect(isStopOpen(true, null, 'a')).toBe(false);
    expect(isStopOpen(true, 'a', 'a')).toBe(true);
    expect(isStopOpen(true, 'a', 'b')).toBe(false);
  });

  it('选中的那张仍在集合里时保持不动', () => {
    expect(resolveOpenStop(['a', 'b'], 'b')).toBe('b');
  });

  it('选中的那张被处理掉后回落到默认，展开项不悬空', () => {
    // a 被 resume 掉、不再 pending：多张时回落到「都不展开」，而不是留着一个空壳 id
    expect(resolveOpenStop(['b', 'c'], 'a')).toBe(null);
    // 仅剩一张时，回落成展开它
    expect(resolveOpenStop(['b'], 'a')).toBe('b');
  });

  it('翻转：点别的那张就换成它（同时只展开一张），点自己收起', () => {
    expect(toggleOpenStop('a', 'b')).toBe('b');
    expect(toggleOpenStop(null, 'b')).toBe('b');
    expect(toggleOpenStop('a', 'a')).toBe(null);
  });

  it('详情没到就不给动作数（不把「还没读到」说成「没有」）', () => {
    expect(stopActionCount(undefined)).toBe(null);
    expect(stopActionCount({ actions: [] })).toBe(0);
    // info_insufficient 那一份后端动作集（补充信息的 resume + 取消任务的旁路）
    const actions: AllowedAction[] = [
      { action: 'continue', kind: 'resume', label: '补充信息并继续', requires_input: true },
      { action: 'cancel', kind: 'side_effect', label: '取消任务' },
    ];
    expect(stopActionCount({ actions })).toBe(2);
  });
});

describe('窄屏：一张也折（决策 192，规格 §5）', () => {
  it('单张在窄屏折叠', () => {
    expect(isFoldable(['only'], true)).toBe(true);
    expect(isFoldable([], true)).toBe(false); // 没有急停就无所谓折叠
    expect(isFoldable(['a', 'b'], true)).toBe(true);
  });

  it('窄屏默认一张都不展开——包括只有一张时', () => {
    expect(defaultOpenStop(['only'], true)).toBe(null);
    expect(resolveOpenStop(['only'], undefined, true)).toBe(null);
    expect(resolveOpenStop(['a', 'b'], undefined, true)).toBe(null);
  });

  it('窄屏下摊开判据只认「人点了展开」那一个 id', () => {
    expect(isStopOpen(isFoldable(['only'], true), null, 'only')).toBe(false);
    expect(isStopOpen(isFoldable(['only'], true), 'only', 'only')).toBe(true);
  });

  it('窄屏下显式收起仍不弹回，选中的那张仍在集合里仍保持', () => {
    expect(resolveOpenStop(['only'], null, true)).toBe(null);
    expect(resolveOpenStop(['a', 'b'], 'b', true)).toBe('b');
    // 被处理掉后回落：窄屏回落到「都不展开」，不是补一个展开项上来
    expect(resolveOpenStop(['b'], 'a', true)).toBe(null);
  });

  it('同一份集合：宽屏展开它自己，窄屏收起——差别只来自 forceFold', () => {
    expect(resolveOpenStop(['only'], undefined, false)).toBe('only');
    expect(resolveOpenStop(['only'], undefined, true)).toBe(null);
  });
});
