import { describe, expect, it } from 'vitest';
import type { ForemanSessionMeta } from '../api/types';
import {
  isFresh,
  loadSeen,
  loadSessionId,
  markSeen,
  pruneSeen,
  saveSeen,
  saveSessionId,
  seedBaselineIfFirstRun,
  seedSeen,
  sessionMark,
  TALK_SEEN_KEY,
  TALK_SESSION_KEY,
  type SeenAt,
} from './talkSessions';

/**
 * 班次两枚标记的判据与「看过哪条」的留存（决策 220③ / 217）。
 *
 * 判定本身是纯函数；存储那一段（`localStorage` 的读写、坏值当空、非法值删键）也在这里钉
 * ——jsdom 自带 `localStorage`，而这几条**只能**在这一层钉：e2e 跑的是真浏览器，但「库里
 * 被别的版本写坏了」「键名换了」这类事在那个高度上造不出来。
 */

function meta(id: string, lastActiveAt: string, title = id): ForemanSessionMeta {
  return {
    id,
    title,
    created_at: '2026-09-18T01:00:00Z',
    last_active_at: lastActiveAt,
    archived_at: null,
  };
}

describe('「有新动静」 = last_active_at 晚于本机记的那一个', () => {
  it('晚于记录才算：相等、更早都不算', () => {
    const seen: SeenAt = { a: '2026-09-18T10:00:00Z' };
    expect(isFresh(meta('a', '2026-09-18T10:00:01Z'), seen)).toBe(true);
    // 相等 = 你看过的就是这一版（打开它那一刻记下的正是这个值）
    expect(isFresh(meta('a', '2026-09-18T10:00:00Z'), seen)).toBe(false);
    expect(isFresh(meta('a', '2026-09-18T09:59:59Z'), seen)).toBe(false);
  });

  it('本机没见过的班次算「有新动静」——它在别处动过', () => {
    expect(isFresh(meta('b', '2026-09-18T10:00:00Z'), {})).toBe(true);
  });

  it('坏时间戳不抛、也不假装知道', () => {
    const seen: SeenAt = { a: 'not-a-date' };
    expect(isFresh(meta('a', 'also-not-a-date'), seen)).toBe(false);
  });

  it('记的是**那一班当时的 last_active_at**，不是本机的当下（同一座钟才可比）', () => {
    const seen = markSeen({}, 'a', '2026-09-18T10:00:00Z');
    expect(seen).toEqual({ a: '2026-09-18T10:00:00Z' });
    // 值没变就不产生新对象（组件里靠它决定要不要写盘）
    expect(markSeen(seen, 'a', '2026-09-18T10:00:00Z')).toBe(seen);
    expect(markSeen(seen, 'a', '2026-09-18T11:00:00Z')).not.toBe(seen);
  });
});

describe('看过表的基线与清理', () => {
  it('基线 = 把当时的状态当「都看过了」（第一屏不该每条都带标记）', () => {
    const seeded = seedSeen([
      meta('a', '2026-09-18T10:00:00Z'),
      meta('b', '2026-09-18T11:00:00Z'),
    ]);
    expect(seeded).toEqual({ a: '2026-09-18T10:00:00Z', b: '2026-09-18T11:00:00Z' });
    expect(isFresh(meta('a', '2026-09-18T10:00:00Z'), seeded)).toBe(false);
    // 之后它又动过 → 才是真有新动静
    expect(isFresh(meta('a', '2026-09-18T10:05:00Z'), seeded)).toBe(true);
  });

  it('服务端已经不存在的班次从表里清掉（决策 217⑤），没有可清的就不换对象', () => {
    const seen: SeenAt = { a: 'T1', gone: 'T2' };
    expect(pruneSeen(seen, ['a', 'b'])).toEqual({ a: 'T1' });
    expect(pruneSeen(seen, ['a', 'gone'])).toBe(seen);
  });
});

describe('两枚标记的合成', () => {
  const ctx = {
    currentId: 'a',
    sendingSid: null as string | null,
    foreignReplying: false,
    seen: {} as SeenAt,
  };

  it('当前打开的那一条**永远不带**「有新动静」（打开即清零，不闪一下）', () => {
    expect(sessionMark(meta('a', '2026-09-18T23:00:00Z'), { ...ctx, seen: {} })).toBeNull();
  });

  it('本机发出未落地的那一班带「正在回话」——**包括它就是当前这一班**', () => {
    expect(sessionMark(meta('a', 'T1'), { ...ctx, sendingSid: 'a', seen: { a: 'T1' } })).toBe(
      'replying',
    );
    // 换班之后标记落在**它**那一行，而不是「你现在看的这一班」
    expect(sessionMark(meta('b', 'T1'), { ...ctx, sendingSid: 'b', seen: { b: 'T1' } })).toBe(
      'replying',
    );
  });

  it('SSE 里别的班次的增量点亮「正在回话」（跨设备那一组的判据）', () => {
    expect(
      sessionMark(meta('c', 'T1'), { ...ctx, foreignReplying: true, seen: { c: 'T1' } }),
    ).toBe('replying');
  });

  it('两条同时成立时「正在回话」优先（它是「此刻」，另一条是「累计」）', () => {
    // 优先关系在**纯函数**里就已经定下来（模板里只有一条 `{#if}`）：这里拿同一个班次
    // 比两次——先让它两条都为真，再看把「回话中」摘掉之后它才转向「有新动静」
    const fresh = { b: '2026-09-18T01:00:00Z' };
    const live = meta('b', '2026-09-18T23:00:00Z');
    expect(sessionMark(live, { ...ctx, foreignReplying: true, seen: fresh })).toBe('replying');
    expect(sessionMark(live, { ...ctx, foreignReplying: false, seen: fresh })).toBe('fresh');
  });

  it('没有标记时是 `null`（三态，不是两个布尔）', () => {
    expect(sessionMark(meta('b', 'T1'), { ...ctx, seen: { b: 'T1' } })).toBeNull();
  });
});

describe('立基线的时机：只在**本机一条记录都没有**时（决策 220③）', () => {
  const remote = [meta('远', '2026-09-18T23:00:00Z')];

  it('空表（装着没用过 / 清了本地状态）→ 立基线，第一屏不整片亮标记', () => {
    const seeded = seedBaselineIfFirstRun({}, remote);
    expect(seeded).toEqual({ 远: '2026-09-18T23:00:00Z' });
    expect(sessionMark(meta('远', '2026-09-18T23:00:00Z'), { ...ctx2, seen: seeded })).toBeNull();
  });

  it('**有过记录**就不再立基线：关机期间别处开的班次照旧算「有新动静」', () => {
    const seen: SeenAt = { 旧: '2026-09-18T09:00:00Z' };
    expect(seedBaselineIfFirstRun(seen, remote)).toBe(seen);
    expect(sessionMark(meta('远', '2026-09-18T23:00:00Z'), { ...ctx2, seen })).toBe('fresh');
  });

  it('表里有它自己那条时按时刻比（关机前它就是这样，之后别处又说了话 → 亮）', () => {
    const seen: SeenAt = { 远: '2026-09-18T20:00:00Z' };
    expect(seedBaselineIfFirstRun(seen, remote)).toBe(seen);
    expect(sessionMark(meta('远', '2026-09-18T23:00:00Z'), { ...ctx2, seen })).toBe('fresh');
    expect(sessionMark(meta('远', '2026-09-18T20:00:00Z'), { ...ctx2, seen })).toBeNull();
  });
});

/** 这一组不关心「哪一班是当前班次」，取「本机不在这两班里」的取值。 */
const ctx2 = { currentId: null, sendingSid: null, foreignReplying: false, seen: {} as SeenAt };

describe('本地留存：库里的值坏了不当作数据，非法值顺手删键（决策 217⑤）', () => {
  it('班次 id 写得进读得出，`null` 顺手删键', () => {
    saveSessionId('s-1');
    expect(localStorage.getItem(TALK_SESSION_KEY)).toBe('s-1');
    expect(loadSessionId()).toBe('s-1');
    saveSessionId(null);
    expect(localStorage.getItem(TALK_SESSION_KEY)).toBeNull();
    expect(loadSessionId()).toBeNull();
  });

  it('空串当作没有（不落一个空值到盘上）', () => {
    localStorage.setItem(TALK_SESSION_KEY, '');
    expect(loadSessionId()).toBeNull();
  });

  it('看过时刻表写得进读得出', () => {
    const seen: SeenAt = { a: '2026-09-18T10:00:00Z' };
    saveSeen(seen);
    expect(JSON.parse(localStorage.getItem(TALK_SEEN_KEY) ?? 'null')).toEqual(seen);
    expect(loadSeen()).toEqual(seen);
  });

  it('坏 JSON / 数组 / 标量一律当空表——不把坏值当数据，也不抛', () => {
    for (const junk of ['{', '"x"', '[1,2]', 'null', '3']) {
      localStorage.setItem(TALK_SEEN_KEY, junk);
      expect(loadSeen(), junk).toEqual({});
    }
  });

  it('没这个键时也是空表（首启）', () => {
    localStorage.removeItem(TALK_SEEN_KEY);
    expect(loadSeen()).toEqual({});
  });
});
