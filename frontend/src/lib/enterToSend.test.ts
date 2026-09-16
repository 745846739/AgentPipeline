import { describe, expect, it } from 'vitest';

import {
  COMPOSITION_SETTLE_MS,
  CompositionGuard,
  isCompositionKeydown,
  shouldSubmitOnEnter,
  type EnterKeyEvent,
} from './enterToSend';

/** 造一次 `keydown`（只填本模块用得到的字段）。 */
function key(over: Partial<EnterKeyEvent> = {}): EnterKeyEvent {
  return { key: 'Enter', shiftKey: false, ...over };
}

/** 手动推进的假时钟（毫秒）。 */
function clock() {
  let ms = 1000;
  return {
    now: () => ms,
    advance: (delta: number) => {
      ms += delta;
    },
  };
}

describe('回车提交的输入法护栏（决策 184）', () => {
  it('普通回车提交；Shift+Enter 不提交（换行留给 textarea）', () => {
    expect(shouldSubmitOnEnter(key(), false)).toBe(true);
    expect(shouldSubmitOnEnter(key({ shiftKey: true }), false)).toBe(false);
  });

  it('非回车键一律不提交', () => {
    for (const k of ['a', 'Escape', 'Process', 'Unidentified']) {
      expect(shouldSubmitOnEnter(key({ key: k }), false)).toBe(false);
    }
  });

  it('Chromium 路线：isComposing 为真的那一次回车不提交', () => {
    expect(shouldSubmitOnEnter(key({ isComposing: true }), false)).toBe(false);
  });

  it('keyCode 229 也挡：只给这一条信号的 IME 不会被漏掉', () => {
    expect(isCompositionKeydown(key({ keyCode: 229 }))).toBe(true);
    expect(shouldSubmitOnEnter(key({ keyCode: 229 }), false)).toBe(false);
    // 组合态里 keyCode 仍是 13 的引擎（WebKit）由 composing 这一重挡住
    expect(shouldSubmitOnEnter(key({ keyCode: 13 }), true)).toBe(false);
  });

  it('WebKit 路线（同一个任务）：compositionend 之后立刻到达的那次回车不提交', () => {
    const c = clock();
    const guard = new CompositionGuard(c.now);

    guard.start();
    expect(guard.active()).toBe(true);
    // 组合中：isComposing 已经是 false（WebKit 的次序），仍必须挡住
    expect(shouldSubmitOnEnter(key({ isComposing: false }), guard.active())).toBe(false);

    // 确认候选词 → compositionend → 紧接着的 keydown（就是被误判成发送的那次）
    guard.end();
    expect(guard.active()).toBe(true);
    expect(shouldSubmitOnEnter(key({ isComposing: false }), guard.active())).toBe(false);
  });

  it('WebKit 路线（跨了任务）：窗口内到达的那次回车仍不提交', () => {
    // 事件分派跨任务时（真实 WKWebView 上不保证永远同一任务），仍要挡得住
    const c = clock();
    const guard = new CompositionGuard(c.now);
    guard.start();
    guard.end();
    c.advance(COMPOSITION_SETTLE_MS - 1);
    expect(shouldSubmitOnEnter(key(), guard.active())).toBe(false);
  });

  it('窗口过后：人手下一次回车照常提交', () => {
    const c = clock();
    const guard = new CompositionGuard(c.now);
    guard.start();
    guard.end();
    c.advance(COMPOSITION_SETTLE_MS);
    expect(shouldSubmitOnEnter(key(), guard.active())).toBe(true);
  });

  it('先选字再发送（连按两次回车）：第二次不会被吞掉', () => {
    // 这是「吃掉下一个回车」那种做法会踩的坑——窗口法不会
    const c = clock();
    const guard = new CompositionGuard(c.now);
    guard.start();
    guard.end();
    // 人手连按两次回车的最快间隔也在一百毫秒上下
    c.advance(COMPOSITION_SETTLE_MS + 50);
    expect(shouldSubmitOnEnter(key(), guard.active())).toBe(true);
  });

  it('没有组合过时不设窗口（普通回车不受影响）', () => {
    const guard = new CompositionGuard(clock().now);
    expect(guard.active()).toBe(false);
    expect(shouldSubmitOnEnter(key(), guard.active())).toBe(true);
  });

  it('再次进入组合态会重置窗口（上一轮的残留不影响下一轮）', () => {
    const c = clock();
    const guard = new CompositionGuard(c.now);
    guard.start();
    guard.end();
    c.advance(COMPOSITION_SETTLE_MS * 4);
    expect(guard.active()).toBe(false);
    guard.start();
    expect(guard.active()).toBe(true);
    guard.end();
    expect(guard.active()).toBe(true);
    c.advance(COMPOSITION_SETTLE_MS);
    expect(guard.active()).toBe(false);
  });
});
