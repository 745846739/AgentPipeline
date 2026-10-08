import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  TALK_DRAFT_KEY,
  TALK_DRAFT_MAX_AGE_MS,
  clearTalkDraft,
  loadTalkDraft,
  writeTalkDraft,
} from './talkDraft';

/** 决策 217⑤ 的三件套：装载恢复、发送后清零、7 天过期（票 22 的单测那一格）。 */
describe('talkDraft（决策 217①⑤：输入草稿只进 localStorage）', () => {
  beforeEach(() => {
    localStorage.removeItem(TALK_DRAFT_KEY);
    vi.restoreAllMocks();
  });

  const draft = (over: Partial<{ sessionId: string; text: string; at: number }> = {}) => ({
    sessionId: 's1',
    text: '半句草稿',
    at: 1_000_000,
    ...over,
  });

  it('写进去读得回来，原文一字不差', () => {
    writeTalkDraft(draft({ text: ' 前后空格也留着\n换行 ' }));
    expect(loadTalkDraft(1_000_000)).toEqual(draft({ text: ' 前后空格也留着\n换行 ' }));
  });

  it('键不在 → null，且不新建键（没有与「坏」是两回事）', () => {
    expect(loadTalkDraft(1_000_000)).toBeNull();
    expect(localStorage.getItem(TALK_DRAFT_KEY)).toBeNull();
  });

  it('清零后读不到（发送成功那一趟，决策 217⑤）', () => {
    writeTalkDraft(draft());
    clearTalkDraft();
    expect(loadTalkDraft(1_000_000)).toBeNull();
    expect(localStorage.getItem(TALK_DRAFT_KEY)).toBeNull();
  });

  it('7 天内的还活着，7 天外的整条删掉（217⑤ 上界）', () => {
    const now = 10_000_000;
    writeTalkDraft(draft({ at: now - TALK_DRAFT_MAX_AGE_MS }));
    expect(loadTalkDraft(now)).toEqual(draft({ at: now - TALK_DRAFT_MAX_AGE_MS }));

    writeTalkDraft(draft({ at: now - TALK_DRAFT_MAX_AGE_MS - 1 }));
    expect(loadTalkDraft(now)).toBeNull();
    expect(localStorage.getItem(TALK_DRAFT_KEY)).toBeNull();
  });

  it('JSON 坏掉 / 形状不对 → 删键并回 null（决策 217④「非法即删键」）', () => {
    for (const raw of ['{', 'null', '"str"', '{"text":"x","at":1}', '{"sessionId":"s1","at":1}', '{"sessionId":"s1","text":"x","at":"now"}']) {
      localStorage.setItem(TALK_DRAFT_KEY, raw);
      expect(loadTalkDraft(1_000_000), `脏值 ${raw}`).toBeNull();
      expect(localStorage.getItem(TALK_DRAFT_KEY), `脏值 ${raw} 应被删掉`).toBeNull();
    }
  });

  it('存储不可用：读回 null、写与删都不抛（丢草稿好过把打字走成抛错）', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('denied');
    });
    expect(loadTalkDraft(1)).toBeNull();
    vi.restoreAllMocks();

    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('denied');
    });
    expect(() => writeTalkDraft(draft())).not.toThrow();
    vi.restoreAllMocks();

    vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
      throw new Error('denied');
    });
    expect(() => clearTalkDraft()).not.toThrow();
    vi.restoreAllMocks();
  });
});
