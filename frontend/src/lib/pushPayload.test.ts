import { describe, expect, it } from 'vitest';

import { BOARD_HOME_HASH, notificationFrom, notificationTarget } from './pushPayload';

/**
 * service worker 两个处理器的纯判据（pwa-webpush 票 03）。
 *
 * 这两族是票面点名要在 vitest 里钉住的：「payload → 通知形状」与「payload → 导航目标」。
 * 判据全是纯函数（解析本身留在调用方、origin 由调用方给），故不需要 service worker
 * 环境——那正是把它们切出来的理由。
 */

describe('推送报文的归一（payload → 通知形状）', () => {
  it('服务端那份三件原样收下', () => {
    const notice = notificationFrom({
      title: '[AgentPipeline] t1 task_pending',
      body: 'task_pending（t1）',
      url: '#/task/t1',
    });
    expect(notice).toEqual({
      title: '[AgentPipeline] t1 task_pending',
      body: 'task_pending（t1）',
      url: '#/task/t1',
    });
  });

  it('标题缺失 / 空白时给兜底标题（showNotification 的空标题会被拒收）', () => {
    expect(notificationFrom({ body: 'x', url: '#/task/t1' }).title).toBe('[AgentPipeline]');
    expect(notificationFrom({ title: '   ', body: 'x' }).title).toBe('[AgentPipeline]');
  });

  it('字符串载荷：JSON 文本照解，非 JSON 文本当正文（都不丢）', () => {
    const asJson = notificationFrom('{"title":"标题","body":"正文","url":"#/talk?session=s1"}');
    expect(asJson.title).toBe('标题');
    expect(asJson.url).toBe('#/talk?session=s1');

    const asText = notificationFrom(' 一句人话 ');
    expect(asText.title).toBe('[AgentPipeline]');
    expect(asText.body).toBe('一句人话');
    expect(asText.url).toBe(BOARD_HOME_HASH);
  });

  it('null / 别的形状：给一条能显示的兜底，不让通知静默消失', () => {
    for (const raw of [null, undefined, 42, [], true]) {
      const notice = notificationFrom(raw);
      expect(notice.title).toBe('[AgentPipeline]');
      // 落点是不是兜底由 `notificationTarget` 说了算（归一与判目标各管一件事）：
      // 形状不认时它一定给出看板首页，故这里断言的是「点下去不会没地方去」。
      expect(notificationTarget(notice.url, 'https://x')).toBe('https://x/#/');
    }
  });
});

describe('点通知去哪儿（payload → 导航目标）', () => {
  const origin = 'https://203.0.113.10';

  it('认自己那几条 hash 深链：拼成同源绝对地址', () => {
    expect(notificationTarget('#/task/t1', origin)).toBe('https://203.0.113.10/#/task/t1');
    expect(notificationTarget('#/task/t1?run=42', origin)).toBe(
      'https://203.0.113.10/#/task/t1?run=42',
    );
    expect(notificationTarget('#/talk?session=s1', origin)).toBe(
      'https://203.0.113.10/#/talk?session=s1',
    );
  });

  it('不可达 / 不可信一律降级看板首页（票面：降级而不是白屏）', () => {
    for (const url of [
      '',
      '   ',
      null,
      undefined,
      'https://evil.example/#/task/t1',
      '//evil.example/#/task/t1',
      '/#/task/t1',
      '#//evil.example',
      'javascript:alert(1)',
    ]) {
      expect(notificationTarget(url, origin)).toBe('https://203.0.113.10/#/');
    }
    expect(notificationTarget('#/', origin)).toBe('https://203.0.113.10/#/');
  });
});
