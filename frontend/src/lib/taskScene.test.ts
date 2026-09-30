import { describe, expect, it } from 'vitest';
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
  const delta = (text: string, runId = 1, role = 'assistant'): LiveDelta => ({
    run_id: runId,
    agent_type: 'main',
    role,
    text,
  });

  it('增量接到所属轮尾：连续同角色并成一条；末步是正文时流式光标挂在那一步', () => {
    const turns = build({
      conversations: [run()],
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
      conversations: [run()],
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

  it('liveTools 按相位折成工具回执（start=running / error=bad / end=ok）', () => {
    const tools: LiveTool[] = [
      { run_id: 1, tool: 'run_command', phase: 'start', args_summary: 'ls' },
      { run_id: 1, tool: 'read_file', phase: 'error', args_summary: 'x.ts' },
      { run_id: 1, tool: 'grep', phase: 'end', args_summary: 'todo' },
    ];
    const turns = build({ conversations: [run()], liveTools: tools });
    const phases = turns[0].steps.filter((s) => s.kind === 'tool').map((s) => s.tool?.phase);
    expect(phases).toEqual(['running', 'bad', 'ok']);
  });

  it('摘要列表还没有那个 run 时合成 live 轮兜在末尾（刷新空窗期不丢输出）', () => {
    const turns = build({ liveDeltas: [delta('现场冒出的增量', 77)] });
    expect(turns).toHaveLength(1);
    expect(turns[0].key).toBe('live77');
    expect(turns[0].streaming).toBe(true);
    expect(turns[0].closing).toBe('现场冒出的增量');
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
      liveDeltas: [{ run_id: 77, agent_type: 'main', role: 'assistant', text: 'x' }],
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
