import type { ForemanMessage, ForemanProposal, ForemanSession } from '../api/types';
import type { ForemanStreamState } from '../realtime/foreman';
import { buildTurns, type TalkTurnsInput } from './talkTurns';

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
      archived_at: null,
    },
    messages,
    proposals,
    total_tokens: 0,
    total_calls: 0,
    turn_in_flight: false,
    foreman: { agent_type: 'foreman', stage_key: 'architect-design', wired: true },
  };
}

function streamOf(over: Partial<ForemanStreamState> = {}): ForemanStreamState {
  return { text: '', thinking: '', tools: [], streaming: false, error: null, ...over };
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

  it('thinking：空串与纯空白都当作没产推理，非空的原样保留（不 trim 正文）', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([
          message({ id: 1, thinking: undefined }),
          message({ id: 2, thinking: '' }),
          message({ id: 3, thinking: '   ' }),
          message({ id: 4, thinking: ' 推演 ' }),
        ]),
      }),
    );
    expect(out.map((t) => t.thinking)).toEqual([null, null, null, ' 推演 ']);
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

  it('落地轮恒非在飞：streaming / partial / liveTools / needsPairing 都是假', () => {
    const out = buildTurns(inputOf({ session: sessionOf([message()]) }));
    expect(out[0]).toMatchObject({
      streaming: false,
      partial: false,
      liveTools: [],
      needsPairing: false,
      proposal: null,
      at: '2026-09-23T10:00:00Z',
    });
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
      thinking: null,
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

  it('live：没收到增量时摆实情占位句，收到了摆正文；tools / thinking 跟着流', () => {
    const idle = buildTurns(inputOf({ sending: true }));
    expect(idle).toHaveLength(1);
    expect(idle[0]).toMatchObject({
      key: 'live',
      kind: 'fm',
      content: '值班长正在查台账…',
      streaming: false,
      partial: false,
      liveTools: [],
    });

    const flowing = buildTurns(
      inputOf({
        sending: true,
        stream: streamOf({
          text: '正在查……',
          streaming: true,
          thinking: '  ',
          tools: [{ tool: 'list_dir', args_summary: 'src', phase: 'start' }],
        }),
      }),
    );
    expect(flowing[0]).toMatchObject({
      key: 'live',
      content: '正在查……',
      streaming: true,
      partial: false,
      thinking: null,
    });
    expect(flowing[0].liveTools).toEqual([{ tool: 'list_dir', args_summary: 'src', phase: 'start' }]);

    // 发送结束了、流里还攒着字（断流）：正文留着，降级为「已收到的部分」
    const broken = buildTurns(inputOf({ stream: streamOf({ text: '收到一半' }) }));
    expect(broken[0]).toMatchObject({ key: 'live', content: '收到一半', streaming: false, partial: true });

    // 什么都没发生：连 live 这一条都没有
    expect(buildTurns(inputOf())).toEqual([]);
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

  it('partial 的边界：在流（streaming）就不是断流；流停了但有字才是', () => {
    const streaming = buildTurns(inputOf({ sending: true, stream: streamOf({ text: '一半', streaming: true }) }));
    expect(streaming[0].partial).toBe(false);

    const stalled = buildTurns(inputOf({ stream: streamOf({ text: '一半', streaming: false }) }));
    expect(stalled[0].partial).toBe(true);

    // 发送中、流还没开、零个字：有这一轮（说明「对面在动」），但它既不是在流也不是断流
    const opening = buildTurns(inputOf({ sending: true, stream: streamOf({ text: '', streaming: false }) }));
    expect(opening[0]).toMatchObject({ key: 'live', streaming: false, partial: false });
  });

  it('三种在飞态都在时，次序是 pending → live → send-error，且都在落地行之后', () => {
    const out = buildTurns(
      inputOf({
        session: sessionOf([message({ created_at: '2026-09-23T10:00:00Z' })]),
        pendingText: '乐观',
        sending: true,
        stream: streamOf({ text: '一半', streaming: false, error: '断了' }),
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
    const following = buildTurns(inputOf({ following: true, stream: streamOf({ text: '正在答' }) }));
    expect(following.map((t) => t.key)).toEqual(['live']);
    expect(following[0]).toMatchObject({ kind: 'fm', content: '正在答', streaming: false });

    // 一个增量都还没到：照旧摆那句「对面在动」的实情（与 `sending` 那一支逐字同一句）
    const opening = buildTurns(inputOf({ following: true }));
    expect(opening[0]).toMatchObject({ key: 'live', content: '值班长正在查台账…' });
  });

  it('没在跟也没在发：只有字、没有轮——那一支由「流里已经有字」兜着', () => {
    // 边界：`following` 为假时单靠 `stream.text` 也出轮（收尾之后到达的尾巴仍要有地方落），
    // 这一条钉的是「两条来源各管各的，不互相替代」
    const out = buildTurns(inputOf({ stream: streamOf({ text: '尾巴' }) }));
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
