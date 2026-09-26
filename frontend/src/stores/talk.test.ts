import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ConversationDeltaEvent, ForemanMessage, ForemanSession } from '../api/types';
import { beginForemanStream, FOREMAN_LOST_TURN_SUFFIX } from '../realtime/foreman';
import { talk } from './talk.svelte';

/**
 * 对讲台的在飞现场（决策 275）——用户报的「切换界面后再回来，本轮之前的输出不见了」。
 *
 * 现场原先住在 `Talk.svelte` 的组件作用域里，页面一切走就被销毁。这里钉的是它搬进 store
 * 之后的两条承重判据：
 *
 * ① **切页面（同一班次）什么都不丢**：`watch()` 认到同一个 id 就不动现场；
 * ② **换班次仍然清现场**：那是决策 204③ / 220⑤ 的既有口径，本次一个字没改。
 *
 * 另加三支收口（决策 260）与增量闸门（只在等一轮期间攒）——它们此前只有 e2e 覆盖。
 */

const SESSION = 'sess-1';

function delta(text: string, sessionId = SESSION): ConversationDeltaEvent {
  return {
    type: 'conversation_delta',
    task_id: '',
    branch: '',
    run_id: 0,
    agent_type: 'foreman',
    session_id: sessionId,
    role: 'assistant',
    text,
    prompt_tokens: 0,
    completion_tokens: 0,
  };
}

/** 台账里的一行（本组只用得到它的 id——判据全是「尾部有没有更新的行」）。 */
function row(id: number): ForemanMessage {
  return {
    id,
    session_id: SESSION,
    role: 'assistant',
    content: '回话',
    prompt_tokens: 0,
    completion_tokens: 0,
    briefing: null,
    traces: null,
    created_at: '2026-09-25T00:05:00Z',
    kind: 'fm',
    proactive: false,
  };
}

function payload(over: Partial<ForemanSession> = {}): ForemanSession {
  return {
    session: {
      id: SESSION,
      title: '班次',
      created_at: '2026-09-25T00:00:00Z',
      last_active_at: '2026-09-25T00:10:00Z',
      kind: 'talk',
      archived_at: null,
    },
    messages: [],
    proposals: [],
    total_tokens: 0,
    total_calls: 0,
    turn_in_flight: false,
    foreman: { agent_type: 'foreman', stage_key: 'foreman', wired: true },
    ...over,
  };
}

/** 把 store 拨回「刚进这一页、还没在读一轮」的干净态（单例，用例之间必须互不残留）。 */
function reset(id: string | null = null): void {
  talk.sessionId = null;
  talk.sending = false;
  talk.resetLive();
  talk.foreign = { bySession: {} };
  talk.sessionId = id;
}

afterEach(() => {
  reset();
  vi.restoreAllMocks();
});

describe('在飞现场随页面来去（决策 275）', () => {
  it('切页面（同一班次）什么都不丢：步骤与乐观轮还在', () => {
    reset(SESSION);
    // 一轮正在跑：本机发出、增量在路上
    talk.sending = true;
    talk.pendingText = '看板里有哪些重试？';
    talk.stream = beginForemanStream();
    talk.note(delta('先看看板。'));
    talk.note({ ...delta('我要查一下。'), channel: 'reasoning' });

    // 页面被卸载又挂回来：`reload()` 走完会拿**同一个** id 再认一次
    talk.watch(SESSION);

    expect(talk.pendingText).toBe('看板里有哪些重试？');
    expect(talk.sending).toBe(true);
    expect(talk.stream.steps.map((step) => step.kind)).toEqual(['text', 'thinking']);
    // 而且增量照旧接得上（切走那段时间它一路攒着）
    talk.note(delta('t1 还在排队。'));
    expect(talk.stream.steps.map((step) => step.kind)).toEqual(['text', 'thinking', 'text']);
  });

  it('换班次照旧清现场（决策 204③ / 220⑤ 的口径一个字没改）', () => {
    reset(SESSION);
    talk.sending = true;
    talk.pendingText = '甲班的话';
    talk.stream = beginForemanStream();
    talk.note(delta('甲班的回话'));
    talk.followingSince = 7;
    talk.pairingNeeded = true;

    talk.watch('sess-2');

    expect(talk.sessionId).toBe('sess-2');
    expect(talk.pendingText, '乐观轮属于**那一班**，不许跟到这一班来').toBeNull();
    expect(talk.stream.steps).toEqual([]);
    expect(talk.followingSince).toBeNull();
    expect(talk.pairingNeeded).toBe(false);
  });

  it('「从没有班次到有班次」不算换班：第一句话自己开的那一班，现场说的就是它', () => {
    // 首启空 home：`send()` 先乐观地亮一轮，再去开一个新班次——这时若把现场清掉，
    // 人刚发出去的那句话会从屏上消失（`talk.watch(sid)` 就在那一步）
    reset(null);
    talk.sending = true;
    talk.pendingText = '第一句话';
    talk.stream = beginForemanStream();

    talk.watch(SESSION);

    expect(talk.pendingText).toBe('第一句话');
    expect(talk.stream.streaming).toBe(true);
  });
});

describe('增量闸门与收口（决策 260 的三支）', () => {
  it('没在等一轮时增量不攒（省得收尾后到达的尾巴凭空造一轮）', () => {
    reset(SESSION);
    talk.note(delta('不属于任何一轮的字'));
    expect(talk.stream.steps).toEqual([]);
  });

  it('在跟一轮时（本机没在发）增量照旧接进分段', () => {
    reset(SESSION);
    talk.followingSince = 0;
    talk.note(delta('接着说的'));
    expect(talk.stream.steps).toEqual([{ kind: 'text', text: '接着说的' }]);
  });

  it('还在跑：一个字都不动（锚点也不许跟着往前爬）', () => {
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = beginForemanStream();
    talk.note(delta('正在答'));

    talk.syncFollowing(payload({ messages: [row(1), row(2)], turn_in_flight: true }));

    expect(talk.followingSince, '锚点必须留在接手那一刻').toBe(2);
    expect(talk.stream.steps).toHaveLength(1);
  });

  it('落地：在飞那一段退场（台账那一行接管），仍有一轮在跑就重新立锚点', () => {
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = beginForemanStream();
    talk.note(delta('半截'));

    talk.syncFollowing(payload({ messages: [row(1), row(2), row(3)], turn_in_flight: false }));

    expect(talk.stream.steps).toEqual([]);
    expect(talk.followingSince).toBeNull();

    // 「回话落了库、而同一班紧接着又起了一轮」（值守轮插进来）：跟的是**新那一轮**
    talk.stream = beginForemanStream();
    talk.followingSince = 2;
    talk.syncFollowing(payload({ messages: [row(1), row(2), row(3)], turn_in_flight: true }));
    expect(talk.followingSince, '新那一轮的锚点按落库后的台账重记').toBe(3);
  });

  it('死轮：半截字留着、只多一句说明（决策 260 裁决③）', () => {
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = beginForemanStream();
    talk.note(delta('说了一半就断'));

    talk.syncFollowing(payload({ messages: [row(1), row(2)], turn_in_flight: false }));

    expect(talk.stream.steps).toEqual([{ kind: 'text', text: '说了一半就断' }]);
    expect(talk.stream.error).toBe(FOREMAN_LOST_TURN_SUFFIX);
    expect(talk.followingSince).toBeNull();
  });

  it('收尾的两声：`settleTurn` 清现场并给台账代次 +1；`markLedgerStale` 只加代次', () => {
    reset(SESSION);
    talk.stream = beginForemanStream();
    talk.pendingText = '问句';
    const before = talk.ledgerEpoch;

    // 失败那一支：本地那条失败轮（在 `stream` 里）要留着，故只提醒重读
    talk.markLedgerStale();
    expect(talk.ledgerEpoch).toBe(before + 1);
    expect(talk.pendingText, '失败那一支不许顺手清现场').toBe('问句');

    // 成功 / 作废那一支：现场退场 + 提醒重读（在屏的那一页据此重读一次台账）
    talk.settleTurn();
    expect(talk.ledgerEpoch).toBe(before + 2);
    expect(talk.pendingText).toBeNull();
    expect(talk.stream.steps).toEqual([]);
  });

  it('本机在发时不接手（两条来源各收各的口）', () => {
    reset(SESSION);
    talk.sending = true;
    talk.syncFollowing(payload({ messages: [row(1)], turn_in_flight: true }));
    expect(talk.followingSince, '本机这一趟的收尾归 send() 管').toBeNull();
  });
});
