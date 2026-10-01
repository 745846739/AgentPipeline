import type { ForemanMessage, ForemanProposal, ForemanSession } from '../api/types';
import type { ForemanStreamState } from '../realtime/foreman';
import {
  buildTurns,
  prettyArgs,
  thinkTicker,
  turnName,
  watchDraftExcerpt,
  WATCH_DRAFT_MAX,
  type TalkTurnsInput,
} from './talkTurns';

/**
 * 对讲台时间线的回合构造（票 02；决策 251 的三块判断之一）。
 *
 * 钉住三件**搬进模块才测得到**的事：
 *
 * ① **分类是字段透传**（决策 252）——`kind` / `proactive` 由后端给，本模块不解析正文；
 * 正文里长着 `【没跑起来】` 前缀的**操作台轮**不许被认成失败轮（判据在字段上，不在字符串上）。
 * ② **合并在行的相互关系上**——提议按时刻插进台账行之间、在飞三态恒在末尾；
 * 逐行谓词那条路测不到这些。
 * ③ **排序必须是确定的**（`Talk.svelte` 原 `:400-402` 注释）——同刻的兜底次序按 `rank`，
 * 换个输入次序结果不变，否则每次渲染都可能换位。
 */

function message(over: Partial<ForemanMessage> = {}): ForemanMessage {
  return {
    id: 1,
    session_id: 's1',
    role: 'assistant',
    content: '回话',
    prompt_tokens: 0,
    completion_tokens: 0,
    briefing: null,
    traces: null,
    created_at: '2026-09-23T10:00:00Z',
    kind: 'fm',
    proactive: false,
    ...over,
  };
}

function proposal(over: Partial<ForemanProposal> = {}): ForemanProposal {
  return {
    // 提议的 key 是 `p${id}`——id 这里取裸数字，免得键叠成 `pp1`。
    id: '1',
    session_id: 's1',
    tool: 'task',
    args: {},
    summary: '提议摘要',
    status: 'pending',
    // 来路（决策 294 / 票 09）：这个替身造的是正常来路的提议
    stopped_round: false,
    created_at: '2026-09-23T10:01:00Z',
    expires_at: '2026-09-23T10:11:00Z',
    resolved_at: null,
    ...over,
  };
}

function sessionOf(messages: ForemanMessage[], proposals: ForemanProposal[] = []): ForemanSession {
  return {
    session: {
      id: 's1',
      title: '班次',
      created_at: '2026-09-23T09:00:00Z',
      last_active_at: '2026-09-23T10:00:00Z',
      kind: 'talk',
      archived_at: null,
    },
    messages,
    proposals,
    total_tokens: 0,
    total_calls: 0,
    turn_in_flight: false,
    // 这一段自己的分页尺（决策 354④）：回合构造不读它，形状要与真载荷一致
    page_limit: 500,
    foreman: { agent_type: 'foreman', stage_key: 'architect-design', wired: true },
  };
}

function streamOf(over: Partial<ForemanStreamState> = {}): ForemanStreamState {
  return { steps: [], events: [], streaming: false, error: null, ...over };
}

/** 在飞轮末尾那一步正文（「正在说的那一句」）。 */
function textStep(text: string) {
  return { kind: 'text' as const, text };
}

/** 在飞轮里的一步推理。 */
function thinkStep(text: string) {
  return { kind: 'thinking' as const, text };
}

/** 在飞轮里的一次工具调用（相位是**此刻**的读数，落地那一份由 `ok` 给同一件事）。 */
function toolStep(tool: string, phase: 'start' | 'end' | 'error', argsSummary = 't-1') {
  return {
    kind: 'tool' as const,
    tool,
    args_summary: argsSummary,
    // 详情字段（决策 301）：原文随 start 就到，结果只在收场后非空。
    args: `{"id":"${argsSummary}"}`,
    result: phase === 'start' ? '' : `读到了 ${argsSummary}`,
    phase,
  };
}

function inputOf(over: Partial<TalkTurnsInput> = {}): TalkTurnsInput {
  return {
    session: null,
    pendingText: null,
    sending: false,
    following: false,
    stream: streamOf(),
    // 配对与否由上游按后端 `kind` 判好（决策 259）——本模块只收这枚布尔。
    pairingNeeded: false,
    ...over,
  };
}

describe('落地轮：分类读字段、不解析正文（决策 252 / 244 / 235①）', () => {
  it('四种 kind 原样透过，key 是 m<id>', () => {
    const kinds = ['mine', 'console', 'failed', 'fm'] as const;
    const out = buildTurns(
      inputOf({
        session: sessionOf(kinds.map((kind, i) => message({ id: i + 1, kind }))),
      }),
    );
    expect(out.map((t) => t.kind)).toEqual(['mine', 'console', 'failed', 'fm']);
    expect(out.map((t) => t.key)).toEqual(['m1', 'm2', 'm3', 'm4']);
  });

  it('failed 与 console 的边界在字段上：正文前缀骗不过它，字段说了算', () => {
    // 两个 role 相同、正文都长着失败前缀的行——只有 kind 区分谁是失败轮（决策 252：
    // 此前这里用 role 三元式 + startsWith 自己认，两个判定点迟早不一致）。
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 1, role: 'system', kind: 'console', content: '【没跑起来】看起来像失败' }),
          message({ id: 2, role: 'system', kind: 'failed', content: '正文里没有前缀也照样是 failed' }),
        ]),
      }),
    );
    expect(out[0].kind).toBe('console');
    expect(out[1].kind).toBe('failed');
    // 正文一个字不改——分类不许回头去动内容
    expect(out[0].content).toBe('【没跑起来】看起来像失败');
    expect(out[1].content).toBe('正文里没有前缀也照样是 failed');
  });

  it('proactive 与 kind 正交、原样透过（值守播报：fm + true）', () => {
    const out = buildTurns(
      inputOf({
        // 三条时刻错开——本条验的是字段透传，别让同刻的 rank 兜底次序掺进来（那是排序那组的事）
        session: sessionOf([
          message({ id: 1, kind: 'fm', proactive: true, content: '值守播报', created_at: '2026-09-23T10:00:00Z' }),
          message({ id: 2, kind: 'fm', proactive: false, content: '回话', created_at: '2026-09-23T10:01:00Z' }),
          message({ id: 3, kind: 'failed', proactive: false, created_at: '2026-09-23T10:02:00Z' }),
        ]),
      }),
    );
    expect(out.map((t) => t.proactive)).toEqual([true, false, false]);
  });

  it('步骤：段序在就走段序（顺序与种类原样，收口那句不在里面）', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({
            id: 1,
            content: '回话',
            segments: [
              { kind: 'thinking', text: '先看看板。' },
              { kind: 'tool', tool: 'read_task', args_summary: 't1', ok: true },
              { kind: 'text', text: '中间插一句。' },
              { kind: 'tool', tool: 'read_board', args_summary: '{}', ok: false },
            ],
          }),
        ]),
      }),
    );
    expect(out[0].steps.map((s) => s.kind)).toEqual(['thinking', 'tool', 'text', 'tool']);
    expect(out[0].steps.map((s) => s.key)).toEqual(['m1-s0', 'm1-s1', 'm1-s2', 'm1-s3']);
    // 详情字段缺省时 args 回落摘要那份（决策 301 的加性口径），result 空串
    expect(out[0].steps[1].tool).toEqual({
      name: 'read_task',
      argsSummary: 't1',
      args: 't1',
      result: '',
      state: 'ok',
    });
    expect(out[0].steps[2].text).toBe('中间插一句。');
    expect(out[0].steps[3].tool?.state).toBe('bad');
    // 段序里没有一个 `live`：落地的那一行每一步都已经收场
    expect(out[0].steps.every((s) => !s.live)).toBe(true);
  });

  it('老行（段序那一列落地之前写的）：由 thinking 与 traces 两份聚合兜底，空 / 纯空白都不算一步', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 1, thinking: '   ', traces: null }),
          message({
            id: 2,
            thinking: ' 推演 ',
            traces: [{ tool: 'read_board', args_summary: '{}', ok: false }],
          }),
        ]),
      }),
    );
    // 兜底能给到的次序只有「先想后查」——中途说过的话在那两列里根本没有（它此前也不显示）
    expect(out[0].steps).toEqual([]);
    expect(out[1].steps.map((s) => s.kind)).toEqual(['thinking', 'tool']);
    expect(out[1].steps[0].text, '不 trim 正文').toBe(' 推演 ');
    expect(out[1].steps[1].tool).toEqual({
      name: 'read_board',
      argsSummary: '{}',
      args: '{}',
      result: '',
      state: 'bad',
    });
  });

  it('attribution 用后端给的 label，未定位时 null——不编一个假的类别', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          // 稳定标识在、label 也在：**只透传 label**，不从标识自己再映射一遍（两份映射
          // 迟早给出两个词），界面只显示后端翻好的那一个。
          message({ id: 1, attribution: 'host', attribution_label: '宿主' }),
          // 未定位：label 为 null / 缺席，即便 reason 也在场也不显示类别。
          message({ id: 2, attribution: 'unlocated', attribution_label: null, attribution_reason: 'missing' }),
          message({ id: 3, attribution_label: undefined }),
        ]),
      }),
    );
    expect(out.map((t) => t.attribution)).toEqual(['宿主', null, null]);
  });

  it('落地轮恒非在飞：streaming / partial / needsPairing 都是假，步骤里也没有「正在攒」那一步', () => {
    const out = buildTurns(inputOf({ session: sessionOf([message()]) }));
    expect(out[0]).toMatchObject({
      streaming: false,
      partial: false,
      needsPairing: false,
      proposal: null,
      at: '2026-09-23T10:00:00Z',
    });
    expect(out[0].steps).toEqual([]);
  });

  it('中断时刻字段透传（票 03）：`interrupted_at` 原样过去，其余行恒 null', () => {
    // 与 attribution 同一条边界（决策 252）：后端给的字段只搬不判——界面不自己从
    // `status` 推时刻、也不从正文里抠「中断」两个字，两份判定点迟早不一致。
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 1, status: 'interrupted', interrupted_at: '2026-09-23T10:05:00Z' }),
          message({ id: 2, created_at: '2026-09-23T10:01:00Z' }),
          message({ id: 3, status: 'in_flight', created_at: '2026-09-23T10:02:00Z' }),
        ]),
      }),
    );
    expect(out.map((t) => t.interruptedAt)).toEqual([
      '2026-09-23T10:05:00Z',
      null,
      null,
    ]);
  });
});

describe('提议合流：按时刻插进台账行之间，不另起一段', () => {
  it('提议夹在两条消息中间，带自己的摘要与对象', () => {
    const p = proposal({ id: '9', created_at: '2026-09-23T10:01:00Z' });
    const out = buildTurns(
      inputOf({
        session: sessionOf(
          [
            message({ id: 1, kind: 'fm', created_at: '2026-09-23T10:00:00Z' }),
            message({ id: 2, kind: 'fm', created_at: '2026-09-23T10:02:00Z' }),
          ],
          [p],
        ),
      }),
    );
    expect(out.map((t) => t.key)).toEqual(['m1', 'p9', 'm2']);
    expect(out[1]).toMatchObject({
      kind: 'proposal',
      content: '提议摘要',
      proposal: p,
      at: '2026-09-23T10:01:00Z',
      streaming: false,
      partial: false,
      steps: [],
      attribution: null,
      proactive: false,
    });
  });

  it('班次没有提议时只有台账行（proposals 缺席 / 空表都不炸）', () => {
    const s = sessionOf([message({ id: 1 })]);
    // 空表
    expect(buildTurns(inputOf({ session: s })).map((t) => t.key)).toEqual(['m1']);
    // session 为 null（这台机器上一个班次都还没有）：空时间线，不是错误
    expect(buildTurns(inputOf({ session: null }))).toEqual([]);
    expect(buildTurns(inputOf({ session: { ...s, proposals: [] } })).map((t) => t.key)).toEqual(['m1']);
  });
});

describe('在飞三态：乐观轮 / 流式轮 / 失败轮（票 02 搬入，恒在末尾）', () => {
  it('pending：pendingText 在场时插一条乐观轮，恒在末尾（at 为空串）', () => {
    const out = buildTurns(
      inputOf({ session: sessionOf([message({ created_at: '2026-09-23T10:00:00Z' })]), pendingText: '我问的那句' }),
    );
    expect(out.map((t) => t.key)).toEqual(['m1', 'pending']);
    expect(out[1]).toMatchObject({
      kind: 'mine',
      content: '我问的那句',
      at: '',
      streaming: false,
      partial: false,
      needsPairing: false,
    });
    // pendingText 没了就没有乐观轮——它不是时间线的常客
    expect(buildTurns(inputOf({ session: sessionOf([message()]) })).map((t) => t.key)).toEqual(['m1']);
  });

  it('live：没收到增量时摆实情占位句，收到了摆正文', () => {
    const idle = buildTurns(inputOf({ sending: true }));
    expect(idle).toHaveLength(1);
    expect(idle[0]).toMatchObject({
      key: 'live',
      kind: 'fm',
      content: '值班长正在查台账…',
      streaming: false,
      partial: false,
    });
    expect(idle[0].steps).toEqual([]);

    const flowing = buildTurns(
      inputOf({ sending: true, stream: streamOf({ steps: [textStep('正在查……')], streaming: true }) }),
    );
    expect(flowing[0]).toMatchObject({
      key: 'live',
      content: '正在查……',
      streaming: true,
      partial: false,
    });
    // 末尾那一步正文是**回话**（正在说的那一句），不是「过程」里的一步
    expect(flowing[0].steps).toEqual([]);

    // 发送结束了、流里还攒着字（断流）：正文留着，降级为「已收到的部分」
    const broken = buildTurns(inputOf({ stream: streamOf({ steps: [textStep('收到一半')] }) }));
    expect(broken[0]).toMatchObject({ key: 'live', content: '收到一半', streaming: false, partial: true });

    // 什么都没发生：连 live 这一条都没有
    expect(buildTurns(inputOf())).toEqual([]);
  });

  /**
   * 段序在在飞轮上的两条判据（决策 273）——它们是「按实际顺序」这件事在**流上**的落点。
   */
  it('live：末尾那一步正文之前的各步，按发生顺序摆进 `steps`（推理 / 工具 / 中途的话）', () => {
    const out = buildTurns(
      inputOf({
        sending: true,
        stream: streamOf({
          streaming: true,
          steps: [
            thinkStep('先看看板。'),
            toolStep('read_board', 'end'),
            textStep('看过了，再去翻台账。'),
            thinkStep('该查 t1 了。'),
            toolStep('read_task', 'start'),
          ],
        }),
      }),
    );
    expect(out[0].steps.map((s) => [s.kind, s.tool?.state ?? null])).toEqual([
      ['thinking', null],
      ['tool', 'ok'],
      ['text', null],
      ['thinking', null],
      ['tool', 'running'],
    ]);
    // 末尾那一步是工具（还没开始说收口的话）：正文那一格仍是那句占位实情
    expect(out[0].content).toBe('值班长正在查台账…');
    // 正在攒的是**末尾**那一步：摘要因此说「正在想…」/「正在查…」
    expect(out[0].steps.map((s) => s.live)).toEqual([false, false, false, false, true]);
    // 流停了（收口 / 断流）之后，末尾那一步不再标「正在攒」——摘要因此从「正在想…」
    // 换成「思考过程 N 字」
    const stopped = buildTurns(
      inputOf({ sending: true, stream: streamOf({ steps: [thinkStep('想完了。')] }) }),
    );
    expect(stopped[0].steps[0].live).toBe(false);
  });

  it('live：正文被一次工具调用打断 → 那一段落定成「中途说的话」，新的一段正文重开', () => {
    // 第一次到达：只有正文 —— 它此刻是**回话**（还没被打断）
    const first = buildTurns(inputOf({ sending: true, stream: streamOf({ steps: [textStep('我先看一眼。')], streaming: true }) }));
    expect(first[0]).toMatchObject({ content: '我先看一眼。' });
    expect(first[0].steps).toEqual([]);

    // 工具调用到达：同一段文字留在段序里（它就是「中途说的话」），正文那一格让位
    const afterTool = buildTurns(
      inputOf({
        sending: true,
        stream: streamOf({
          streaming: true,
          steps: [textStep('我先看一眼。'), toolStep('read_board', 'start')],
        }),
      }),
    );
    expect(afterTool[0].content).toBe('值班长正在查台账…');
    expect(afterTool[0].steps.map((s) => s.kind)).toEqual(['text', 'tool']);
    expect(afterTool[0].steps[0].text).toBe('我先看一眼。');

    // 收尾：权威回话进来 → 又出现末尾那一步正文，它才是回话
    const settled = buildTurns(
      inputOf({
        sending: true,
        stream: streamOf({
          steps: [
            textStep('我先看一眼。'),
            toolStep('read_board', 'end'),
            textStep('看完了，没有待办。'),
          ],
        }),
      }),
    );
    expect(settled[0].content).toBe('看完了，没有待办。');
    expect(settled[0].steps.map((s) => s.kind)).toEqual(['text', 'tool']);
  });

  it('send-error：失败原因进正文；挂不挂配对入口只看上游判好的布尔（决策 259），报文字样说了不算', () => {
    const crossOrigin = buildTurns(
      inputOf({ stream: streamOf({ error: '跨源写请求被拒绝：Origin/Referer = x' }) }),
    );
    expect(crossOrigin).toHaveLength(1);
    expect(crossOrigin[0]).toMatchObject({
      key: 'send-error',
      kind: 'failed',
      content: '发送失败：跨源写请求被拒绝：Origin/Referer = x',
      needsPairing: false,
      at: '',
      streaming: false,
      partial: false,
    });

    // 报文里明晃晃写着「还没配对」，但上游按 kind 判为否（跨源 403 不带 kind）→ 不挂：
    // 字样能被后端任何一次改文案击穿，kind 不能。
    const spoofed = buildTurns(
      inputOf({ pairingNeeded: false, stream: streamOf({ error: '这台设备还没配对：…' }) }),
    );
    expect(spoofed[0].needsPairing).toBe(false);

    // 反过来：上游判了是，哪怕报文换了措辞也照挂
    const paired = buildTurns(
      inputOf({ pairingNeeded: true, stream: streamOf({ error: '措辞换了也不影响' }) }),
    );
    expect(paired[0].needsPairing).toBe(true);
  });

  /**
   * 同一个失败不许在时间线里摆两轮（决策 337）：后端在失败当场就把原因落成台账行
   * （决策 211④），本地那条传输层报文与它说的是一件事——台账那一行在场时本地那条
   * **一帧都不出现**（判在渲染上；只在收尾那一刻判一次会留下共存窗，e2e 20 次红 2 次）。
   */
  it('台账已接管这次失败时，本地那条失败轮不上时间线；没接管时它是唯一信号', () => {
    const ledgerFailed = sessionOf([
      message({ id: 1, kind: 'mine', content: '这句话要能改几个字再发' }),
      message({ id: 2, kind: 'failed', content: '【没跑起来】这一轮没跑起来（llm_auth）：…' }),
    ]);
    const stream = streamOf({ error: 'provider 鉴权失败' });

    const owned = buildTurns(
      inputOf({ session: ledgerFailed, stream, ledgerOwnsFailure: true }),
    );
    expect(owned.map((t) => t.key)).toEqual(['m1', 'm2']);
    expect(owned.some((t) => t.key === 'send-error')).toBe(false);

    // 请求根本没到后端（断网 / 代理 502 / 配对 403 在进 handler 之前）：台账不会多出新行，
    // 本地这条就是**唯一**的信号——它必须还在（这条是上一条的反面，防「一律不摆」）。
    const notOwned = buildTurns(
      inputOf({ session: ledgerFailed, stream, ledgerOwnsFailure: false }),
    );
    expect(notOwned.map((t) => t.key)).toEqual(['m1', 'm2', 'send-error']);
  });

  it('partial 的边界：在流（streaming）就不是断流；流停了但有字才是', () => {
    const streaming = buildTurns(
      inputOf({ sending: true, stream: streamOf({ steps: [textStep('一半')], streaming: true }) }),
    );
    expect(streaming[0].partial).toBe(false);

    const stalled = buildTurns(inputOf({ stream: streamOf({ steps: [textStep('一半')] }) }));
    expect(stalled[0].partial).toBe(true);

    // 发送中、流还没开、零个字：有这一轮（说明「对面在动」），但它既不是在流也不是断流
    const opening = buildTurns(inputOf({ sending: true, stream: streamOf() }));
    expect(opening[0]).toMatchObject({ key: 'live', streaming: false, partial: false });
  });

  it('三种在飞态都在时，次序是 pending → live → send-error，且都在落地行之后', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([message({ created_at: '2026-09-23T10:00:00Z' })]),
        pendingText: '乐观',
        sending: true,
        stream: streamOf({ steps: [textStep('一半')], error: '断了' }),
      }),
    );
    expect(out.map((t) => t.key)).toEqual(['m1', 'pending', 'live', 'send-error']);
  });

  it('只有 error、没有在发也没有正文：只出失败轮，不出空白的 live 轮', () => {
    const out = buildTurns(inputOf({ stream: streamOf({ error: '断了' }) }));
    expect(out.map((t) => t.key)).toEqual(['send-error']);
  });

  it('跟一轮（决策 260）：后端说在跑、本机没在发，也要出那一轮', () => {
    // 刷新之后接上的那一轮：`sending` 是假（那一趟 POST 随旧页面走了），但它在跑——
    // 增量已经在往 `stream` 里攒，界面得有一轮来承载它，否则字收下了却没地方显示。
    const following = buildTurns(
      inputOf({ following: true, stream: streamOf({ steps: [textStep('正在答')] }) }),
    );
    expect(following.map((t) => t.key)).toEqual(['live']);
    expect(following[0]).toMatchObject({ kind: 'fm', content: '正在答', streaming: false });

    // 一个增量都还没到：照旧摆那句「对面在动」的实情（与 `sending` 那一支逐字同一句）
    const opening = buildTurns(inputOf({ following: true }));
    expect(opening[0]).toMatchObject({ key: 'live', content: '值班长正在查台账…' });
  });

  it('没在跟也没在发：只有字、没有轮——那一支由「流里已经有步骤」兜着', () => {
    // 边界：`following` 为假时单靠段序里已有的步骤也出轮（收尾之后到达的尾巴仍要有地方落），
    // 这一条钉的是「两条来源各管各的，不互相替代」
    const out = buildTurns(inputOf({ stream: streamOf({ steps: [textStep('尾巴')] }) }));
    expect(out.map((t) => t.key)).toEqual(['live']);
  });
});

describe('排序：同刻的兜底次序确定、异刻按时间（原 :400-402「排序必须是确定的」）', () => {
  it('同刻的兜底次序按 rank：人打头、值班长收尾，中档保持输入次序（稳定排序）', () => {
    const at = '2026-09-23T10:00:00Z';
    const msgs = [
      message({ id: 1, kind: 'fm', created_at: at }),
      message({ id: 2, kind: 'mine', created_at: at }),
      message({ id: 3, kind: 'console', created_at: at }),
      message({ id: 4, kind: 'failed', created_at: at }),
    ];
    const props = [proposal({ id: '1', created_at: at })];
    // rank: mine=0、中档 console/failed/提议=1、fm=2；同档同刻靠稳定排序吃输入次序。
    // 提议是在消息**之后**压入的，故中档里它排在中档消息后头。
    expect(buildTurns(inputOf({ session: sessionOf(msgs, props) })).map((t) => t.key)).toEqual([
      'm2',
      'm3',
      'm4',
      'p1',
      'm1',
    ]);
    // 换两种输入次序：跨档结论一字不变（m2 恒第一、m1 恒最后），中档跟着输入走——
    // 这正是「确定」的含义：次序由 rank + 输入定，不由渲染时序撞运气。
    expect(
      buildTurns(inputOf({ session: sessionOf([...msgs].reverse(), props) })).map((t) => t.key),
    ).toEqual(['m2', 'm4', 'm3', 'p1', 'm1']);
    expect(
      buildTurns(
        inputOf({ session: sessionOf([msgs[2], msgs[0], msgs[3], msgs[1]], props) }),
      ).map((t) => t.key),
    ).toEqual(['m2', 'm3', 'm4', 'p1', 'm1']);
  });

  it('异刻按时间排，与 kind 无关（值班长的话在先、人的话在后照样人在后）', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf(
          [
            message({ id: 1, kind: 'mine', created_at: '2026-09-23T10:05:00Z' }),
            message({ id: 2, kind: 'fm', created_at: '2026-09-23T09:00:00Z' }),
            message({ id: 3, kind: 'fm', created_at: '2026-09-23T10:02:00Z' }),
          ],
          [proposal({ id: '1', created_at: '2026-09-23T10:01:00Z' })],
        ),
      }),
    );
    expect(out.map((t) => t.key)).toEqual(['m2', 'p1', 'm3', 'm1']);
  });
});

describe('提问轮（决策 265，第三种轮型）：载荷读字段、「已答」按行序纯派生', () => {
  const ask = { question: '这张票怎么处理？', options: ['修一下', '搁置'] };

  it('kind=ask 原样透过、ask 载荷带过来，后到的话还没来 → 未答', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 1, kind: 'mine', content: '拿个主意' }),
          message({ id: 2, kind: 'ask', content: '等你选。', ask }),
        ]),
      }),
    );
    const turn = out.find((t) => t.kind === 'ask');
    expect(turn).toBeDefined();
    expect(turn!.ask).toEqual(ask);
    expect(turn!.askAnswered).toBe(false);
    expect(turn!.key).toBe('m2');
  });

  it('开场那句 mine 在 ask **之前**，不算已答——判据是「之后还有人的话」', () => {
    // 每一轮都以一条 user 消息开场（后端 `say` 先落人的话）：它 id 小于 ask 行，
    // 不该把刚抛出的问题判成已答——265③「免机制」靠的就是这个不变量。
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 1, kind: 'mine', content: '拿个主意' }),
          message({ id: 2, kind: 'ask', ask }),
        ]),
      }),
    );
    expect(out.find((t) => t.kind === 'ask')!.askAnswered).toBe(false);
  });

  it('下一轮开场（后到的 mine）即已答 / 被取代：选项钮该灰', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 1, kind: 'mine' }),
          message({ id: 2, kind: 'ask', ask }),
          message({ id: 3, kind: 'mine', content: '还是搁置吧' }),
          message({ id: 4, kind: 'fm', content: '好' }),
        ]),
      }),
    );
    expect(out.find((t) => t.kind === 'ask')!.askAnswered).toBe(true);
  });

  it('别的轮没有 ask 载荷（null / false），透传与合并不受影响', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 1, kind: 'mine' }),
          message({ id: 2, kind: 'fm' }),
          message({ id: 3, kind: 'ask', ask }),
        ]),
      }),
    );
    const [a, b, c] = out;
    expect(a.ask).toBeNull();
    expect(a.askAnswered).toBe(false);
    expect(b.ask).toBeNull();
    expect(c.kind).toBe('ask');
    expect(c.ask!.options).toHaveLength(2);
  });
});

/**
 * 名牌那张表（决策 271）：**值守轮的失败账不叫「发送失败」**。
 *
 * 2026-09-24 的实测里，值守轮因 provider 断供连失 37 轮，而那些行在时间线上顶着
 * 「发送失败」——那一批里值班经理一个字节都没发出去。判据是两个后端字段：
 * `failed` + `proactive`（后者由后端从自家前缀派出来，界面不解析正文）。
 */
describe('名牌（turnName）', () => {
  it('人的那一轮失败：发送失败（本地那一轮与台账那一行同名）', () => {
    expect(turnName({ kind: 'failed', proactive: false })).toBe('发送失败');
  });

  it('值守轮的失败账：值守 · 没跑起来（不再冒充「发送失败」）', () => {
    expect(turnName({ kind: 'failed', proactive: true })).toBe('值守 · 没跑起来');
  });

  it('其余各档逐字保留', () => {
    expect(turnName({ kind: 'mine', proactive: false })).toBe('值班经理');
    expect(turnName({ kind: 'console', proactive: false })).toBe('操作台');
    expect(turnName({ kind: 'fm', proactive: false })).toBe('值班长');
    expect(turnName({ kind: 'fm', proactive: true })).toBe('值班长 · 值守');
  });
});

/**
 * 值守台账上的名牌与占位句（票 04 / 决策 286）：**按班次类型写，不靠 `proactive` 猜**。
 *
 * 存量旧行不回填（裁决 12），而值守账里将来落下的每一行都出自值守轮——「这一行是谁的」
 * 由账本类型一个判据给出，逐行的 `proactive` 在这本账上没有第二句话可说。人的账里
 * 「值班长 · 值守」那档照旧（上面的用例钉着），两本账各念各的表。
 */
describe('值守台账（ledgerKind = watch）：名牌与占位句按账本类型（票 04）', () => {
  it('名牌：fm 行一律「值守」—— proactive 在场与否都不改读法', () => {
    expect(turnName({ kind: 'fm', proactive: true }, 'watch')).toBe('值守');
    expect(turnName({ kind: 'fm', proactive: false }, 'watch')).toBe('值守');
  });

  it('名牌：其余各档逐字；失败账不问 proactive', () => {
    expect(turnName({ kind: 'failed', proactive: true }, 'watch')).toBe('值守 · 没跑起来');
    expect(turnName({ kind: 'failed', proactive: false }, 'watch')).toBe('值守 · 没跑起来');
    expect(turnName({ kind: 'mine', proactive: false }, 'watch')).toBe('值班经理');
    expect(turnName({ kind: 'console', proactive: false }, 'watch')).toBe('操作台');
  });

  it('buildTurns：同一行播报在两本账里 key 不变，名牌判据跟着 ledgerKind 走', () => {
    const row = message({ id: 7, kind: 'fm', proactive: false, content: '1 号任务到了合入审批' });
    const talkLedger = buildTurns(inputOf({ session: sessionOf([row]) }));
    const watchLedger = buildTurns(
      inputOf({ session: sessionOf([row]), ledgerKind: 'watch' }),
    );
    expect(talkLedger[0].key).toBe('m7');
    expect(watchLedger[0].key).toBe('m7');
    // 两本账的行分类都是后端给的那一个（决策 252 不变）；变的只是名牌的读法
    expect(talkLedger[0].kind).toBe('fm');
    expect(watchLedger[0].kind).toBe('fm');
  });

  it('在飞占位句：值守账说「值守正在跑…」，人的账照旧', () => {
    const watch = buildTurns(inputOf({ following: true, ledgerKind: 'watch' }));
    expect(watch).toHaveLength(1);
    expect(watch[0]).toMatchObject({ key: 'live', kind: 'fm', content: '值守正在跑…' });

    const talkSide = buildTurns(inputOf({ following: true }));
    expect(talkSide[0].content).toBe('值班长正在查台账…');
  });

  it('「转去对话」的摘录：全文不超上限原样、超了截断补省略号、首尾空白收掉', () => {
    const short = '  1 号任务到了合入审批  ';
    expect(watchDraftExcerpt(short)).toBe('1 号任务到了合入审批');
    expect(watchDraftExcerpt('夜班播报')).toBe('夜班播报');

    const long = '长'.repeat(WATCH_DRAFT_MAX + 10);
    const cut = watchDraftExcerpt(long);
    expect(cut).toHaveLength(WATCH_DRAFT_MAX + 1); // 截 280 字 + 省略号
    expect(cut.endsWith('…')).toBe(true);
    // 边界：恰好在上限之内的不截
    expect(watchDraftExcerpt('长'.repeat(WATCH_DRAFT_MAX))).toHaveLength(WATCH_DRAFT_MAX);
  });
});

/**
 * 快照与直播的**拼接**（票 02）：在途半截行是基准，直播只接 `seq > seq0` 的尾巴。
 *
 * 「中途刷新」那条用户诉求的下半边（票 02 of talk-live-identity 起改判）：半截行**不再
 * 单独成轮**——它从台账位置摘出，与尾巴**拼成一条 live 轮**（渲染键就是行 id），
 * 「回来接上」的形态与「本机在发」不可区分。判据在 `realtime/foreman.test.ts::spliceAccepts`
 * （三支各有单测），这里钉它们接到时间线上的样子。
 */
describe('快照与直播的拼接：在途半截行是基准（票 02）', () => {
  const half = () =>
    message({
      id: 5,
      status: 'in_flight',
      content: '快照里已有的半句',
      seq: 12,
      created_at: '2026-09-23T10:02:00Z',
    });

  it('半截行与尾巴拼成一条 live 轮（键 = 行 id）：seq0 之前的字不重复', () => {
    const stream = streamOf({
      // 到达时**不筛**（筛在渲染时按基准做）——两件事都攒着，正是这条判据的输入
      events: [
        { kind: 'delta', channel: 'content', text: '快照里已有的半句', ledger_id: 5, seq: 12 },
        { kind: 'delta', channel: 'content', text: '之后才说的字', ledger_id: 5, seq: 13 },
      ],
      steps: [textStep('快照里已有的半句'), textStep('之后才说的字')],
      streaming: true,
    });
    const turns = buildTurns(
      inputOf({
        session: sessionOf([message({ id: 4, role: 'user', kind: 'mine', content: '问' }), half()]),
        following: true,
        stream,
      }),
    );

    // 半截行从台账位置摘出：不再有落地式的 m5，只有一条拼好的轮，键沿用行 id
    expect(turns.filter((t) => t.kind !== 'mine').map((t) => t.key)).toEqual(['m5']);
    const live = turns.find((t) => t.key === 'm5');
    expect(live?.kind).toBe('fm');
    expect(live?.streaming).toBe(true);
    // content = 已落库的正文 + 快照之后的增量，全文不丢字、不重复
    expect(live?.content).toBe('快照里已有的半句之后才说的字');
  });

  it('尾巴为空且流没亮：半截行照落地式渲染，不摆占位句那一轮', () => {
    const stream = streamOf({
      events: [{ kind: 'delta', channel: 'content', text: '快照里已有的半句', ledger_id: 5, seq: 12 }],
      steps: [textStep('快照里已有的半句')],
      streaming: false,
    });
    const turns = buildTurns(
      inputOf({ session: sessionOf([half()]), following: true, stream }),
    );
    expect(turns.map((t) => t.key)).toEqual(['m5']);
  });

  it('没有在途行（快照里还没有半截行）：直播照旧摆整条流，键取事件上的行 id', () => {
    const stream = streamOf({
      events: [{ kind: 'delta', channel: 'content', text: '整条流', ledger_id: 5, seq: 3 }],
      steps: [textStep('整条流')],
      streaming: true,
    });
    const turns = buildTurns(inputOf({ session: sessionOf([message()]), following: true, stream }));
    // 内容照旧是整条流；键由决策 354③ 定——首条带 `ledger_id` 的事件已到，故是 `m5`
    // （快照里那条半截行随后读回来时用的也是同一个键，折叠态因此跨过这一拍）。
    const live = turns.find((t) => t.key === 'm5');
    expect(live?.content).toBe('整条流');
  });

  it('尾巴里有工具：已落库的正文定格成「中途说的话」，回话位让给工具后的新话', () => {
    // 刷库形状（foreman.rs::LiveState）：content 列是**这一次调用**正在冒的正文；
    // 工具收场后它挪进段序、下一次调用的正文从空处长出来——拼接据此分岔。
    const stream = streamOf({
      events: [
        { kind: 'tool', tool: 'read_task', args_summary: 't-1', phase: 'end', ledger_id: 5, seq: 13 },
        { kind: 'delta', channel: 'content', text: '查到了，结论是…', ledger_id: 5, seq: 14 },
      ],
      streaming: true,
    });
    const turns = buildTurns(
      inputOf({ session: sessionOf([half()]), following: true, stream }),
    );

    const live = turns.find((t) => t.key === 'm5');
    expect(live?.content, '回话位是工具之后的新话').toBe('查到了，结论是…');
    expect(
      live?.steps.map((s) => s.kind),
      '已落库的半句在中途话的位置，工具照排',
    ).toEqual(['text', 'tool']);
  });

  it('快照里已收场的段序进前缀：拼接轮的步骤序 = 落地段序 + 尾巴', () => {
    const stream = streamOf({
      events: [{ kind: 'delta', channel: 'content', text: '接着冒的字', ledger_id: 5, seq: 13 }],
      streaming: true,
    });
    const turns = buildTurns(
      inputOf({
        session: sessionOf([
          message({
            id: 5,
            status: 'in_flight',
            content: '已落库的半句',
            seq: 12,
            segments: [{ kind: 'thinking', text: '想过什么' }],
          }),
        ]),
        following: true,
        stream,
      }),
    );

    const live = turns.find((t) => t.key === 'm5');
    expect(live?.steps.map((s) => s.kind)).toEqual(['thinking']);
    expect(live?.steps[0]?.text).toBe('想过什么');
    expect(live?.content).toBe('已落库的半句接着冒的字');
  });
});

/**
 * 乐观轮去重（票 02 of talk-live-identity）：POST 在途时返回的快照里**已经有 user 行**，
 * 乐观轮再摆一遍就是同一句话说两遍——台账里已有同文一句时让位。
 */
describe('乐观轮与台账 user 行的去重（票 02 of talk-live-identity）', () => {
  it('台账里已有同文的一句：乐观轮退场', () => {
    const turns = buildTurns(
      inputOf({
        session: sessionOf([message({ id: 4, role: 'user', kind: 'mine', content: '同一句' })]),
        pendingText: '同一句',
        sending: true,
      }),
    );
    expect(turns.filter((t) => t.kind === 'mine')).toHaveLength(1);
    expect(turns.some((t) => t.key === 'pending')).toBe(false);
  });

  it('台账里没有这句（正常发送途中的快照）：乐观轮照旧在', () => {
    const turns = buildTurns(
      inputOf({ pendingText: '新的一句', sending: true }),
    );
    expect(turns.some((t) => t.key === 'pending' && t.content === '新的一句')).toBe(true);
  });
});

/**
 * 展开详情的两件文案 / 行为规格（决策 301）。
 *
 * 它们都住在模块里而不是组件的回调里：取哪一行、折成什么形状——都是**规格**（换一个实现
 * 就该红），不是排版细节。
 */
describe('展开详情的规格（决策 301）', () => {
  describe('thinkTicker：收起行里那一行「它想到哪了」', () => {
    it('取最后一行非空文本——推理是逐行往外写的，最后一行就是它此刻停在哪', () => {
      expect(thinkTicker('先看一遍\n再查台账\n正在核对第 3 条')).toBe('正在核对第 3 条');
    });

    it('末尾的空行不算「最新」——流式下每一段都跟在换行之后', () => {
      expect(thinkTicker('第一行\n第二行\n\n  \n')).toBe('第二行');
    });

    it('行内空白折成单空格（它要落在 nowrap 的一行里，换行符会把摘要行撑成两块）', () => {
      expect(thinkTicker('not  quite\t\t这里   还有空白')).toBe('not quite 这里 还有空白');
    });

    it('一个字都没有时回空串——模板据此只显示「正在想…」', () => {
      expect(thinkTicker('')).toBe('');
      expect(thinkTicker('\n\n   \n')).toBe('');
    });

    it('只有一行时就是它自己', () => {
      expect(thinkTicker('正在想一件事')).toBe('正在想一件事');
    });
  });

  describe('prettyArgs：参数原串 → 看得懂的正文', () => {
    it('能 parse 的 JSON 美化两空格（人要在展开体里读它，不是机读）', () => {
      expect(prettyArgs('{"task_id":"t1","n":2}')).toBe(
        '{\n  "task_id": "t1",\n  "n": 2\n}',
      );
    });

    it('parse 不了的照原文吐——绝不在界面上替它编一个结构', () => {
      expect(prettyArgs('--flag value')).toBe('--flag value');
      expect(prettyArgs('{"task_id": ')).toBe('{"task_id": ');
    });

    it('空串与纯空白都由模板另说（回空串，不编「{}」）', () => {
      expect(prettyArgs('')).toBe('');
      expect(prettyArgs('   \n ')).toBe('');
    });

    it('JSON 标量也算 parse 得动：原样给回去（不做多余包装）', () => {
      expect(prettyArgs('123')).toBe('123');
    });
  });

});

/**
 * 在飞轮的渲染键（决策 354③）：**首条带 `ledger_id` 的事件到达即改用 `m<ledger_id>`**。
 *
 * 这一条键的存在性取代了整台「折叠态搬运机」（`settlingTurn` + `carryLiveStepOpen` +
 * `carryLiveTurnOpen` + 组件 effect 的两段提交）。钉三件：
 *   ① 乐观段（无事件）仍是 `live`，首事件一到就换键；
 *   ② **换键瞬间没有任何可折叠内容**——乐观段零步骤，故 `live-s<i>` 从来没被写进折叠表；
 *   ③ **收口后键不变**：在飞轮与落地轮的步骤键逐字相同（决策 301 的「手动展开不被自动打回」）。
 */
describe('在飞轮的渲染键（决策 354③）', () => {
  /** 一条带位置戳的正文增量（`ledger_id` = 台账里那条在途半截行的 id）。 */
  function stampedDelta(text: string, ledgerId: number, seq = 1) {
    return { kind: 'delta' as const, channel: 'content' as const, text, ledger_id: ledgerId, seq };
  }

  it('乐观段是 `live`；首条带 `ledger_id` 的事件一到就换成 `m<id>`', () => {
    // 还没收到任何事件：只剩那句「对面在动」的占位实情
    const optimistic = buildTurns(inputOf({ sending: true }));
    expect(optimistic.map((t) => t.key)).toEqual(['live']);

    const firstEvent = buildTurns(
      inputOf({
        sending: true,
        stream: streamOf({
          events: [stampedDelta('第一段', 9)],
          steps: [textStep('第一段')],
          streaming: true,
        }),
      }),
    );
    expect(firstEvent.map((t) => t.key)).toEqual(['m9']);
  });

  it('换键瞬间没有可折叠内容：乐观段零步骤，故 `live-s<i>` 从来没进过折叠表', () => {
    // 乐观段一个步骤都没有——「过程」那一组画不出东西，推理 / 工具行也不存在，
    // 于是切换点上折叠表里不可能有 `live` / `live-s<i>` 的条目（换键不丢人的操作）。
    expect(buildTurns(inputOf({ sending: true }))[0].steps).toEqual([]);

    const firstEvent = buildTurns(
      inputOf({
        sending: true,
        stream: streamOf({
          events: [{ kind: 'delta', channel: 'reasoning', text: '先想', ledger_id: 9, seq: 1 }],
          steps: [thinkStep('先想')],
          streaming: true,
        }),
      }),
    );
    // 首事件渲染出来的步骤键**从一开始就是** `m9-s0`
    expect(firstEvent[0].steps.map((s) => s.key)).toEqual(['m9-s0']);
  });

  it('收口后键不变：在飞轮与落地轮的步骤键逐字相同', () => {
    const live = buildTurns(
      inputOf({
        sending: true,
        stream: streamOf({
          events: [{ kind: 'delta', channel: 'reasoning', text: '想过什么', ledger_id: 9, seq: 1 }],
          steps: [thinkStep('想过什么')],
          streaming: true,
        }),
      }),
    );
    expect(live.map((t) => t.key)).toEqual(['m9']);

    // 收口：流倒空、台账里**同一个 id** 的那一行接管（`settleTurn` → `reload`）。
    const landed = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 9, kind: 'fm', segments: [{ kind: 'thinking', text: '想过什么' }] }),
        ]),
      }),
    );
    expect(landed.map((t) => t.key)).toEqual(['m9']);
    expect(landed[0].steps.map((s) => s.key)).toEqual(live[0].steps.map((s) => s.key));
  });

  it('快照里那条在途半截行优先于事件：拼进去的那一路仍用行 id（既有路径）', () => {
    // 事件上的 `ledger_id` 与快照那条半截行是**同一条行**；base 在手时以它为准
    // （事件可能一条都还没到，而快照已经说了这一轮在半途）。
    const turns = buildTurns(
      inputOf({
        session: sessionOf([message({ id: 5, status: 'in_flight', content: '半句', seq: 3 })]),
        following: true,
        stream: streamOf({ steps: [textStep('半句')], streaming: true }),
      }),
    );
    expect(turns.map((t) => t.key)).toEqual(['m5']);
  });

  it('事件不带 `ledger_id`（老后端 / 流水线事件）：键仍是 `live`，与从前逐字一致', () => {
    const turns = buildTurns(
      inputOf({
        sending: true,
        stream: streamOf({ steps: [textStep('老后端只说这些')], streaming: true }),
      }),
    );
    expect(turns.map((t) => t.key)).toEqual(['live']);
  });

  /**
   * 收口那一拍的**同键共存**：`reload` 已经把落地那一行写进 `session`，而 store 还
   * 没倒空现场（`settleTurn` 在 `reload` 返回之后）——这一拍里在飞轮与落地轮本是同一条行。
   *
   * 两条都摆出来会是同一轮说两遍，而且**同一个渲染键出现两次**（`Talk.svelte` 是
   * `{#each turns as turn (turn.key)}`，同键是坏形状）。故在飞轮在那一拍整条退场：
   * 台账那一行是权威（带完整回话与段序），在飞轮只是上一个时态的残影。
   */
  it('落地行在场时在飞轮整条退场：同一轮不许摆两遍、同一个渲染键不许出现两次', () => {
    const settled = message({
      id: 9,
      kind: 'fm',
      content: '权威回话',
      segments: [{ kind: 'thinking', text: '想过什么' }],
    });
    const turns = buildTurns(
      inputOf({
        session: sessionOf([settled]),
        sending: true,
        // 现场还没倒空：事件仍攥着这一轮的位置戳（`settleForemanStream` 保住的最后一份）
        stream: streamOf({
          events: [
            { kind: 'delta', channel: 'reasoning', text: '想过什么', ledger_id: 9, seq: 1 },
            { kind: 'delta', channel: 'content', text: '权威回话', ledger_id: 9, seq: 2 },
          ],
          steps: [thinkStep('想过什么'), textStep('权威回话')],
          streaming: false,
        }),
      }),
    );

    expect(turns.map((t) => t.key)).toEqual(['m9']);
    expect(new Set(turns.map((t) => t.key)).size).toBe(turns.length);
    expect(turns[0].content).toBe('权威回话');
    // 步骤键仍是那一套（折叠态在切换前后是同一份）
    expect(turns[0].steps.map((s) => s.key)).toEqual(['m9-s0']);
  });

  it('中断行（重启恢复标的终态）同样让在飞轮退场：那一条行才是这一轮的终点', () => {
    const turns = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 9, kind: 'fm', status: 'interrupted', interrupted_at: '2026-09-23T10:05:00Z' }),
        ]),
        following: true,
        stream: streamOf({
          events: [{ kind: 'delta', channel: 'content', text: '断之前说的', ledger_id: 9, seq: 1 }],
          steps: [textStep('断之前说的')],
          streaming: false,
        }),
      }),
    );

    expect(turns.map((t) => t.key)).toEqual(['m9']);
    expect(turns[0].interruptedAt).toBe('2026-09-23T10:05:00Z');
  });
});

/**
 * 两条并行在途行（决策 260：值守轮 + 人的轮）的呈现（决策 363④，票 12）。
 *
 * 旧判据只认**第一条**在途行当基准，且把 `ledger_id` 对不上的增量一并放行——另一条行的
 * 字于是被折进基准行渲染一遍，而它在自己那一轮里已经渲染过（两条并发时重字）。现判据
 * **按 `ledger_id` 逐行分流**：每条在途行各自一个基准、各自折进自己那一轮、按行的位置
 * 排在时间线里；不带 `ledger_id` 的事件归第一条（老后端 / 流水线事件）。
 */
describe('两条并行在途行各自成轮（决策 363④）', () => {
  const inFlight = (id: number, over: Partial<ForemanMessage> = {}): ForemanMessage =>
    message({
      id,
      status: 'in_flight',
      seq: 5,
      created_at: `2026-09-23T10:0${id}:00Z`,
      ...over,
    });

  /** 值守那条（proactive=true，先开）与人的那条（后开），两条都在飞。 */
  const twoLines = (): ForemanMessage[] => [
    inFlight(5, { content: '甲半句', proactive: true, created_at: '2026-09-23T10:01:00Z' }),
    inFlight(6, { content: '乙半句', created_at: '2026-09-23T10:02:00Z' }),
  ];

  it('两条行各占一轮：各自的增量只折进自己那一轮，不串台、不重字', () => {
    const stream = streamOf({
      events: [
        { kind: 'delta', channel: 'content', text: '甲后说', ledger_id: 5, seq: 6 },
        { kind: 'delta', channel: 'content', text: '乙后说', ledger_id: 6, seq: 6 },
      ],
      streaming: true,
    });
    const turns = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 4, kind: 'mine', content: '问' }),
          ...twoLines(),
        ]),
        following: true,
        stream,
      }),
    );

    // 各占一轮，按行的位置（时刻）排：m4 → m5 → m6
    expect(turns.map((t) => t.key)).toEqual(['m4', 'm5', 'm6']);
    const five = turns.find((t) => t.key === 'm5');
    const six = turns.find((t) => t.key === 'm6');
    expect(five?.content).toBe('甲半句甲后说');
    expect(six?.content).toBe('乙半句乙后说');
    // 互不串台：谁也不含对方那半句
    expect(five?.content).not.toContain('乙');
    expect(six?.content).not.toContain('甲');
    // 逐行读那一行自己的字段：值守那条与人的那条名牌分得开
    expect(five?.proactive).toBe(true);
    expect(six?.proactive).toBe(false);
  });

  it('没有 ledger_id 的事件归第一条在途行（老后端 / 流水线事件）', () => {
    const stream = streamOf({
      events: [{ kind: 'delta', channel: 'content', text: '无归属的字' }],
      streaming: true,
    });
    const turns = buildTurns(
      inputOf({ session: sessionOf(twoLines()), following: true, stream }),
    );

    expect(turns.find((t) => t.key === 'm5')?.content).toBe('甲半句无归属的字');
    expect(turns.find((t) => t.key === 'm6')?.content).toBe('乙半句');
  });

  it('一条行已经落地时，它的字不渗进还在飞的那一轮', () => {
    const stream = streamOf({
      events: [{ kind: 'delta', channel: 'content', text: '乙在冒', ledger_id: 6, seq: 6 }],
      streaming: true,
    });
    const turns = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 5, kind: 'fm', content: '甲已落地' }),
          inFlight(6, { content: '乙半句', created_at: '2026-09-23T10:02:00Z' }),
        ]),
        following: true,
        stream,
      }),
    );

    expect(turns.find((t) => t.key === 'm5')?.content).toBe('甲已落地');
    expect(turns.find((t) => t.key === 'm6')?.content).toBe('乙半句乙在冒');
    expect(turns.filter((t) => t.key === 'm6')).toHaveLength(1);
  });

  it('两条行都死（流没亮、尾巴为空）时不摆占位轮：各自按落地式渲染', () => {
    const turns = buildTurns(
      inputOf({ session: sessionOf(twoLines()) }),
    );
    expect(turns.map((t) => t.key)).toEqual(['m5', 'm6']);
    expect(turns.map((t) => t.streaming)).toEqual([false, false]);
  });

  it('行还没进快照的新轮：它的字不丢（暂时归基准行），等下一份快照把它接走', () => {
    // `ledger_id` 认不出（不在快照里）= 新轮的第一批增量比快照先到。**不能丢**——
    // 丢在这儿会让那一段直播静默消失；旧口径的「宁可多收」正是为这个窗口留的。
    const stream = streamOf({
      events: [
        { kind: 'delta', channel: 'content', text: '新轮的第一批字', ledger_id: 9, seq: 1 },
      ],
      streaming: true,
    });
    const turns = buildTurns(
      inputOf({ session: sessionOf(twoLines()), following: true, stream }),
    );

    expect(turns.find((t) => t.key === 'm5')?.content).toBe('甲半句新轮的第一批字');
    expect(turns.find((t) => t.key === 'm6')?.content).toBe('乙半句');
  });

  it('已落地行的迟到增量不渗进在飞轮（那一行本身就是权威）', () => {
    const stream = streamOf({
      events: [{ kind: 'delta', channel: 'content', text: '迟到的一条', ledger_id: 4, seq: 99 }],
      streaming: true,
    });
    const turns = buildTurns(
      inputOf({
        session: sessionOf([message({ id: 4, kind: 'fm', content: '甲已落地' }), ...twoLines()]),
        following: true,
        stream,
      }),
    );

    expect(turns.find((t) => t.key === 'm4')?.content).toBe('甲已落地');
    expect(turns.find((t) => t.key === 'm5')?.content).toBe('甲半句');
    expect(turns.find((t) => t.key === 'm6')?.content).toBe('乙半句');
  });
});
