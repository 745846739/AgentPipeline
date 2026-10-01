import { beforeEach, describe, expect, it } from 'vitest';
import type { ChatMessage, ConversationSummary, NodeCommand, NodeConversation } from '../api/types';
import type { LiveDelta, LiveTool } from '../realtime/reduce';
import { buildTaskScene, sceneTurnMatches, type SceneTurn } from './taskScene';

/**
 * 「现场」时间线归约的判据（决策 349）。判断全在 `taskScene.ts`，这里把关系整段钉住：
 * 命令归哪一轮、孤儿命令怎么分组、「只有命令没有会话的节点」长什么形状、流式增量接到
 * 哪一头上、收口话从哪里摘——这些全是行与行的关系，逐行谓词测不到。
 */

function run(overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return {
    run_id: 1,
    stage: 'develop',
    node: 'execute',
    attempt: 1,
    agent_type: 'main',
    parent_run_id: null,
    prompt_tokens: 100,
    completion_tokens: 50,
    status: 'success',
    archived_at: null,
    ...overrides,
  };
}

function conv(
  messages: ChatMessage[],
  overrides: Partial<NodeConversation> = {},
): NodeConversation {
  return {
    id: 1,
    task_id: 't1',
    run_id: 1,
    stage: 'develop',
    node: 'execute',
    attempt: 1,
    agent_type: 'main',
    parent_run_id: null,
    messages_json: messages,
    metadata_json: null,
    prompt_tokens: 100,
    completion_tokens: 50,
    system_prompt: null,
    user_prompt: null,
    reasoning: null,
    created_at: '2026-09-29T10:00:00Z',
    ...overrides,
  };
}

function cmd(overrides: Partial<NodeCommand> = {}): NodeCommand {
  return {
    id: 7,
    task_id: 't1',
    run_id: 1,
    stage: 'develop',
    node: 'execute',
    source: 'agent',
    command: 'rtk read foo.ts',
    original_command: null,
    cwd: '',
    exit_code: 0,
    stdout_path: null,
    stdout_preview: 'hello',
    stderr_preview: null,
    duration_ms: 120,
    started_at: '2026-09-29T10:01:00Z',
    finished_at: '2026-09-29T10:01:01Z',
    ...overrides,
  };
}

function build(overrides: Partial<Parameters<typeof buildTaskScene>[0]> = {}) {
  return buildTaskScene({
    conversations: [],
    conversationFor: () => undefined,
    commands: [],
    liveDeltas: [],
    liveTools: [],
    commandOutputFor: () => null,
    ...overrides,
  });
}

describe('现场时间线 · 会话轮（决策 349）', () => {
  it('消息折成步骤：user / assistant 各是一步，最后一条 assistant 正文摘成收口话', () => {
    const turns = build({
      conversations: [run()],
      conversationFor: () =>
        conv([
          { role: 'user', content: '把这段写完' },
          { role: 'assistant', content: '先看一下现状。' },
          { role: 'assistant', content: '写完了，结论如下。' },
        ]),
    });

    expect(turns).toHaveLength(1);
    const turn = turns[0];
    expect(turn.key).toBe('r1');
    expect(turn.name).toBe('develop · execute');
    expect(turn.closing).toBe('写完了，结论如下。');
    expect(turn.closingStreaming).toBe(false);
    expect(turn.steps.map((s) => `${s.kind}:${s.role}`)).toEqual([
      'text:user',
      'text:assistant',
    ]);
    expect(turn.loaded).toBe(true);
  });

  it('tool 消息按 tool_call_id 配回所属调用；配不上自成一条；空 content 的 tool 行剔除', () => {
    const turns = build({
      conversations: [run()],
      conversationFor: () =>
        conv([
          { role: 'user', content: '查一下' },
          {
            role: 'assistant',
            content: null,
            tool_calls: [
              {
                id: 'call-1',
                type: 'function',
                function: { name: 'read_file', arguments: '{"path":"foo.ts"}' },
              },
            ],
          },
          { role: 'tool', tool_call_id: 'call-1', name: 'read_file', content: '文件内容' },
          { role: 'tool', tool_call_id: 'call-ghost', name: 'other', content: '孤儿结果' },
          { role: 'tool', tool_call_id: 'call-empty', content: '' },
        ]),
    });

    const steps = turns[0].steps;
    const toolSteps = steps.filter((s) => s.kind === 'tool');
    // 配回的那条（带参数与结果）+ 配不上的那条；空 content 的不渲染
    expect(toolSteps).toHaveLength(2);
    expect(toolSteps[0].tool?.name).toBe('read_file');
    expect(toolSteps[0].tool?.argsSummary).toContain('foo.ts');
    expect(toolSteps[0].tool?.result).toBe('文件内容');
    expect(toolSteps[1].tool?.name).toBe('other');
    expect(toolSteps[1].tool?.args).toBe('');
  });

  it('没装载的轮 loaded=false，等装载后重算（组件显示「正在读取会话…」）', () => {
    const turns = build({ conversations: [run()] });
    expect(turns[0].loaded).toBe(false);
    expect(turns[0].steps).toHaveLength(0);
  });

  it('子代理轮名牌带 ∟ 名；run 状态原样递给名牌旁的读数', () => {
    const turns = build({
      conversations: [run({ run_id: 2, agent_type: 'coder', status: 'failed' })],
    });
    expect(turns[0].sub).toBe('coder');
    expect(turns[0].status).toBe('failed');
    expect(turns[0].key).toBe('r2');
  });
});

describe('现场时间线 · 命令归轮与孤儿分组', () => {
  it('命令按 run_id 归进该轮、按 started_at 排；输出走完整/流式账，缺了回落 preview', () => {
    const turns = build({
      conversations: [run()],
      conversationFor: () => conv([{ role: 'user', content: '跑一下' }]),
      commands: [
        cmd({ id: 8, started_at: '2026-09-29T10:02:00Z', stdout_preview: null }),
        cmd({ id: 7, started_at: '2026-09-29T10:01:00Z' }),
      ],
      commandOutputFor: (c) => (c.id === 8 ? '完整输出' : null),
    });

    const cmdSteps = turns[0].steps.filter((s) => s.kind === 'command');
    expect(cmdSteps.map((s) => s.command?.id)).toEqual([7, 8]);
    expect(cmdSteps[0].command?.output).toBe('hello');
    expect(cmdSteps[1].command?.output).toBe('完整输出');
  });

  it('改写过的命令折叠行显示原串、rewritten 置真（决策 297 的两条都摆）', () => {
    const turns = build({
      commands: [
        cmd({
          run_id: null,
          command: 'rtk read foo.ts',
          original_command: 'cat foo.ts',
        }),
      ],
    });

    expect(turns).toHaveLength(1);
    expect(turns[0].name).toBe('develop · execute');
    expect(turns[0].runId).toBeNull();
    const c = turns[0].steps[0].command;
    expect(c?.command).toBe('cat foo.ts');
    expect(c?.actualCommand).toBe('rtk read foo.ts');
    expect(c?.rewritten).toBe(true);
  });

  it('「只有命令没有会话的节点」：孤儿命令按 stage · node 分组成合成轮，同节点并组', () => {
    const turns = build({
      commands: [
        cmd({ id: 1, run_id: null, stage: 'init', node: 'validate_input', started_at: '2026-09-29T09:00:00Z' }),
        cmd({ id: 2, run_id: null, stage: 'init', node: 'validate_input', started_at: '2026-09-29T09:01:00Z' }),
        cmd({ id: 3, run_id: null, stage: 'init', node: 'validate_output', started_at: '2026-09-29T09:02:00Z' }),
      ],
    });

    expect(turns.map((t) => t.name)).toEqual(['init · validate_input', 'init · validate_output']);
    expect(turns[0].steps).toHaveLength(2);
    expect(turns[0].steps.every((s) => s.kind === 'command')).toBe(true);
    expect(turns[0].at).toBe('2026-09-29T09:00:00Z');
  });

  it('指向不存在 run 的命令同样进孤儿桶（那一轮没有会话可挂）', () => {
    const turns = build({
      conversations: [run({ run_id: 1 })],
      commands: [cmd({ run_id: 99, stage: 'review', node: 'validate_output' })],
    });
    expect(turns).toHaveLength(2);
    // r1 没时刻排末尾；孤儿组按命令的 started_at 排在前
    expect(turns[0].name).toBe('review · validate_output');
    expect(turns[1].key).toBe('r1');
  });
});

describe('现场时间线 · 流式增量', () => {
  /** 在飞的 run（status=running）才有流式（决策 359①：光标由 run 的台账状态把关）。 */
  const flying = (overrides: Partial<ConversationSummary> = {}): ConversationSummary =>
    run({ status: 'running', ...overrides });
  let seq = 0;
  const delta = (
    text: string,
    runId = 1,
    role = 'assistant',
    channel: 'content' | 'reasoning' = 'content',
  ): LiveDelta => ({ run_id: runId, agent_type: 'main', role, channel, text, seq: seq++ });
  const tool = (
    name: string,
    phase: LiveTool['phase'],
    runId = 1,
  ): LiveTool => ({
    run_id: runId,
    tool: name,
    phase,
    args_summary: `${name} 的参数`,
    args: '',
    result: phase === 'start' ? '' : '结果',
    seq: seq++,
  });
  const resetSeq = () => {
    seq = 0;
  };

  beforeEach(() => resetSeq());

  it('增量接到所属轮尾：连续同角色并成一条；末步是正文时流式光标挂在那一步', () => {
    const turns = build({
      conversations: [flying()],
      liveDeltas: [delta('正在写 '), delta('实现'), delta('（中途读数）', 1, 'tool')],
    });

    expect(turns[0].streaming).toBe(true);
    // 末条增量是工具事件：assistant 那两句定格成「中途说过的话」，收口位空着
    expect(turns[0].closing).toBe('');
    expect(turns[0].closingStreaming).toBe(false);
    const texts = turns[0].steps.filter((s) => s.kind === 'text');
    expect(texts.map((s) => `${s.role}:${s.text}`)).toEqual([
      'assistant:正在写 实现',
      'tool:（中途读数）',
    ]);
    expect(texts[texts.length - 1].streaming).toBe(true);
  });

  it('落地会话已有收口话时增量照常作步骤排（不顶掉落地的收尾）', () => {
    const turns = build({
      conversations: [flying()],
      conversationFor: () => conv([{ role: 'user', content: '开始' }], {
        messages_json: [
          { role: 'user', content: '开始' },
          { role: 'assistant', content: '上一轮的结论。' },
        ],
      }),
      liveDeltas: [delta('新一轮正在跑')],
    });

    expect(turns[0].closing).toBe('上一轮的结论。');
    expect(turns[0].closingStreaming).toBe(false);
    expect(turns[0].steps.some((s) => s.text === '新一轮正在跑')).toBe(true);
  });

  it('思考自成一步（决策 244 / 359①）：reasoning 与正文分开折，连续思考并一条、换类另起', () => {
    const turns = build({
      conversations: [flying()],
      liveDeltas: [
        delta('先看看板。', 1, 'assistant', 'reasoning'),
        delta('再查台账。', 1, 'assistant', 'reasoning'),
        delta('结论是……', 1, 'assistant', 'content'),
      ],
    });

    expect(turns[0].closing).toBe('结论是……');
    // 正文摘成收口话后，步骤里只剩思考那一大步
    const kinds = turns[0].steps.map((s) => `${s.kind}:${s.text}`);
    expect(kinds).toEqual(['thinking:先看看板。再查台账。']);
  });

  it('直播流按到达序交织（决策 359①）：思考 → 工具 → 思考 → 正文，工具不再整体堆顶', () => {
    const turns = build({
      conversations: [flying()],
      liveDeltas: [
        { ...delta('想读一下文件', 1, 'assistant', 'reasoning'), seq: 0 },
        { ...delta('文件说完了', 1, 'assistant', 'reasoning'), seq: 2 },
        { ...delta('读到了：内容如下', 1, 'assistant', 'content'), seq: 3 },
      ],
      liveTools: [{ ...tool('read_file', 'end'), seq: 1 }],
    });
    // 正文（seq 3，最后到达）摘成收口话：步骤序保真「思考 → 工具 → 再思考」
    expect(turns[0].steps.map((s) => s.kind)).toEqual([
      'thinking',
      'tool',
      'thinking',
    ]);
    expect(turns[0].closing).toBe('读到了：内容如下');
  });

  it('reasoning 收尾不摘收口话（思考不是回话），流式光标挂到思考步上', () => {
    const turns = build({
      conversations: [flying()],
      liveDeltas: [
        delta('先说结论。', 1, 'assistant', 'content'),
        delta('等等，再想想。', 1, 'assistant', 'reasoning'),
      ],
    });

    expect(turns[0].closing).toBe('');
    const last = turns[0].steps[turns[0].steps.length - 1];
    expect(last.kind).toBe('thinking');
    expect(last.streaming).toBe(true);
  });

  it('run 落地（台账不再报 running）后增量还在，也不再亮流式（决策 359①）', () => {
    const turns = build({
      conversations: [run({ status: 'success' })],
      liveDeltas: [delta('写完了。')],
    });

    expect(turns[0].streaming).toBe(false);
    expect(turns[0].closingStreaming).toBe(false);
    expect(turns[0].steps.some((s) => s.streaming)).toBe(false);
    // 收口话照常摘出（最后一次到达仍是 assistant 正文）
    expect(turns[0].closing).toBe('写完了。');
  });

  it('liveTools 折成工具回执（start=running / error=bad / end=ok），原文参数与结果进展开体', () => {
    const tools: LiveTool[] = [
      { run_id: 1, tool: 'run_command', phase: 'start', args_summary: 'ls', args: '{"cmd":"ls"}', result: '', seq: 0 },
      { run_id: 1, tool: 'read_file', phase: 'error', args_summary: 'x.ts', args: '', result: '炸了', seq: 1 },
      { run_id: 1, tool: 'grep', phase: 'end', args_summary: 'todo', args: '', result: '命中', seq: 2 },
    ];
    const turns = build({ conversations: [flying()], liveTools: tools });
    const receipts = turns[0].steps.filter((s) => s.kind === 'tool');
    expect(receipts.map((s) => s.tool?.phase)).toEqual(['running', 'bad', 'ok']);
    expect(receipts[0].tool?.args).toBe('{"cmd":"ls"}');
    expect(receipts[1].tool?.result).toBe('炸了');
  });

  it('摘要列表还没有那个 run 时合成 live 轮兜在末尾（刷新空窗期不丢输出）', () => {
    const turns = build({ liveDeltas: [delta('现场冒出的增量', 77)] });
    expect(turns).toHaveLength(1);
    expect(turns[0].key).toBe('live77');
    expect(turns[0].streaming).toBe(true);
    expect(turns[0].closing).toBe('现场冒出的增量');
  });

  it('live 轮末步是思考时同样兜得住：不摘收口、思考步亮光标', () => {
    const turns = build({
      liveDeltas: [{ ...delta('正在想。', 77, 'assistant', 'reasoning'), seq: 9 }],
    });
    expect(turns[0].closing).toBe('');
    const last = turns[0].steps[turns[0].steps.length - 1];
    expect(last.kind).toBe('thinking');
    expect(last.streaming).toBe(true);
  });
});

describe('现场时间线 · 多次尝试分主次（决策 359③）', () => {
  it('同一 stage · node · agent 的旧一代 primary=false，最新一代 primary=true', () => {
    const turns = build({
      conversations: [
        run({ run_id: 1, attempt: 1, status: 'failed' }),
        run({ run_id: 2, attempt: 2, status: 'failed' }),
        run({ run_id: 3, attempt: 3, status: 'running' }),
      ],
    });

    expect(turns.map((t) => [t.attempt, t.primary])).toEqual([
      [1, false],
      [2, false],
      [3, true],
    ]);
  });

  it('同代并行的子代理（同 attempt）都是主——它们不是重试', () => {
    const turns = build({
      conversations: [
        run({ run_id: 1, attempt: 2, agent_type: 'coder' }),
        run({ run_id: 2, attempt: 2, agent_type: 'coder' }),
      ],
    });

    expect(turns.map((t) => t.primary)).toEqual([true, true]);
  });

  it('不同节点的尝试互不影响', () => {
    const turns = build({
      conversations: [
        run({ run_id: 1, node: 'validate_input', attempt: 1 }),
        run({ run_id: 2, node: 'validate_output', attempt: 1 }),
      ],
    });

    expect(turns.map((t) => t.primary)).toEqual([true, true]);
  });

  it('attempt 带到轮上；合成轮 / live 轮恒为第 1 次且是主', () => {
    const turns = build({
      conversations: [run({ run_id: 1, attempt: 2 })],
      commands: [cmd({ run_id: null })],
      liveDeltas: [{ run_id: 77, agent_type: 'main', role: 'assistant', channel: 'content', text: 'x', seq: 0 }],
    });
    const conv = turns.find((t) => t.runId === 1);
    const orphan = turns.find((t) => t.runId === null && t.key.startsWith('g'));
    const live = turns.find((t) => t.key === 'live77');
    expect(conv?.attempt).toBe(2);
    expect(orphan?.attempt).toBe(1);
    expect(orphan?.primary).toBe(true);
    expect(live?.attempt).toBe(1);
    expect(live?.primary).toBe(true);
  });
});

describe('现场时间线 · 全局排序', () => {
  it('按完整会话的 created_at 排；没装载的轮回落该轮命令的最早 started_at', () => {
    const turns = build({
      conversations: [run({ run_id: 2 }), run({ run_id: 1 })],
      conversationFor: (rid) =>
        rid === 2
          ? conv([], { run_id: 2, id: 2, created_at: '2026-09-29T11:00:00Z' })
          : undefined,
      commands: [cmd({ run_id: 1, started_at: '2026-09-29T10:30:00Z' })],
    });
    expect(turns.map((t) => t.key)).toEqual(['r1', 'r2']);
  });

  it('两边都没时刻的轮排末尾（按 run_id 定序）；live 轮永远在最后', () => {
    const turns = build({
      conversations: [run({ run_id: 5 }), run({ run_id: 3 })],
      liveDeltas: [
        { run_id: 77, agent_type: 'main', role: 'assistant', channel: 'content', text: 'x', seq: 0 },
      ],
    });
    expect(turns.map((t) => t.key)).toEqual(['r3', 'r5', 'live77']);
  });
});

describe('现场时间线 · 轮级过滤（sceneTurnMatches）', () => {
  const turns = build({
    conversations: [run({ run_id: 1, status: 'failed' })],
    commands: [cmd({ run_id: null, command: 'cargo test --quiet', original_command: null })],
  });
  // 孤儿命令组按 started_at 排前、r1 没时刻排末尾：过滤的那一轮要挑会话轮
  const turn = turns.find((t) => t.runId === 1) as SceneTurn;

  it('命中名牌 / 状态 / 命令串 / 输出即留下', () => {
    expect(sceneTurnMatches(turn, 'develop')).toBe(true);
    expect(sceneTurnMatches(turn, 'failed')).toBe(true);
  });

  it('孤儿命令组命中命令串与输出', () => {
    const orphan = turns.find((t) => t.runId === null) as SceneTurn;
    expect(sceneTurnMatches(orphan, 'CARGO')).toBe(true);
    expect(sceneTurnMatches(orphan, 'hello')).toBe(true);
  });

  it('没命中即滤掉；空查询全留', () => {
    expect(sceneTurnMatches(turn, '找不到的词')).toBe(false);
    expect(sceneTurnMatches(turn, '')).toBe(true);
    expect(sceneTurnMatches(turn, '   ')).toBe(true);
  });
});

describe('现场时间线 · 轮首的 prompt 与落地思考（决策 360）', () => {
  const delta = (
    text: string,
    runId = 1,
    role = 'assistant',
    channel: 'content' | 'reasoning' = 'content',
  ): LiveDelta => ({ run_id: runId, agent_type: 'main', role, channel, text, seq: 0 });

  it('完整会话带 prompt 快照与 reasoning 时，轮首依次是 prompt 步与思考步，转录跟在其后', () => {
    const turns = build({
      conversations: [run()],
      conversationFor: () =>
        conv([{ role: 'assistant', content: '写完了，结论如下。' }], {
          system_prompt: '你是架构师。',
          user_prompt: '请设计登录。',
          reasoning: '先想结构，再想元数据。',
        }),
    });

    expect(turns[0].steps.map((s) => s.kind)).toEqual(['prompt', 'thinking']);
    expect(turns[0].steps[0].prompt).toEqual({ system: '你是架构师。', user: '请设计登录。' });
    expect(turns[0].steps[1].text).toBe('先想结构，再想元数据。');
    // 收口话照旧从转录里摘，不受轮首两步影响
    expect(turns[0].closing).toBe('写完了，结论如下。');
  });

  it('prompt 两段都空、reasoning 为 null 的轮不加空步（历史行 / 不产推理的模型）', () => {
    const turns = build({
      conversations: [run()],
      conversationFor: () => conv([{ role: 'assistant', content: '结论。' }]),
    });

    expect(turns[0].steps.map((s) => s.kind)).toEqual([]);
  });

  it('落地思考在场时，直播的 reasoning 声道不再折步——同一份思考只摆一遍', () => {
    const turns = build({
      conversations: [run({ status: 'success' })],
      conversationFor: () =>
        conv([{ role: 'assistant', content: '写完了。' }], { reasoning: '先想结构。' }),
      liveDeltas: [
        delta('先想结构。', 1, 'assistant', 'reasoning'),
        delta('写完了。', 1, 'assistant', 'content'),
      ],
    });

    const thinking = turns[0].steps.filter((s) => s.kind === 'thinking');
    expect(thinking).toHaveLength(1);
    expect(thinking[0].text).toBe('先想结构。');
  });

  it('没落地的轮（直播中）reasoning 增量照旧折成思考步（决策 244 的直播口径不动）', () => {
    const turns = build({
      conversations: [run({ status: 'running' })],
      conversationFor: () => undefined,
      liveDeltas: [delta('正在想。', 1, 'assistant', 'reasoning')],
    });

    expect(turns[0].steps.map((s) => s.kind)).toEqual(['thinking']);
  });

  it('轮级过滤命中 prompt 两段原文即留下', () => {
    const turns = build({
      conversations: [run()],
      conversationFor: () =>
        conv([{ role: 'assistant', content: '结论。' }], {
          system_prompt: '夜班流水线的守则',
          user_prompt: '实现分段折步',
        }),
    });
    const turn = turns[0];

    expect(sceneTurnMatches(turn, '守则')).toBe(true);
    expect(sceneTurnMatches(turn, '分段折步')).toBe(true);
    expect(sceneTurnMatches(turn, '找不到的词')).toBe(false);
  });
});
