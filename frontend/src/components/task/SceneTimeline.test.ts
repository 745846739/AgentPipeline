import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ChatMessage, ConversationSummary, NodeCommand, NodeConversation } from '../../api/types';
import SceneTimeline from './SceneTimeline.svelte';

/**
 * 「现场」页签的接线（决策 349）：归约判据在 `lib/taskScene.test.ts` 钉住，
 * 这里钉组件层的四件事——轮怎么画、命令回执怎么展开、流式输出常显、窗口化与过滤。
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

function conv(messages: ChatMessage[], runId = 1): NodeConversation {
  return {
    id: runId,
    task_id: 't1',
    run_id: runId,
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
  };
}

function cmd(overrides: Partial<NodeCommand> = {}): NodeCommand {
  return {
    id: 7,
    task_id: 't1',
    run_id: null,
    stage: 'develop',
    node: 'execute',
    source: 'agent',
    command: 'rtk read foo.ts',
    original_command: null,
    cwd: '',
    exit_code: 0,
    stdout_path: '/tmp/out',
    stdout_preview: 'preview 行',
    stderr_preview: null,
    duration_ms: 1200,
    started_at: '2026-09-29T10:01:00Z',
    finished_at: '2026-09-29T10:01:01Z',
    ...overrides,
  };
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('现场时间线 · 轮与收口话', () => {
  it('名牌按 stage · node 写；收口话与消息步骤都在那一轮里', () => {
    render(SceneTimeline, {
      props: {
        conversations: [run({ run_id: 1, agent_type: 'coder' })],
        conversationFor: () =>
          conv([
            { role: 'user', content: '开始' },
            { role: 'assistant', content: '结论写在这里。' },
          ]),
        commands: [],
        commandOutputFor: () => null,
      },
    });

    const turn = document.querySelector('article.turn') as HTMLElement;
    expect(turn.textContent).toContain('develop · execute');
    expect(turn.textContent).toContain('∟ coder');
    expect(turn.textContent).toContain('YOU');
    expect(turn.textContent).toContain('开始');
    // 收口话经 markdown 渲染
    expect(turn.textContent).toContain('结论写在这里。');
  });

  it('完整会话没装载的轮显示「正在读取会话…」，空任务给空态', () => {
    render(SceneTimeline, {
      props: { conversations: [run()], conversationFor: () => undefined, commands: [], commandOutputFor: () => null },
    });
    expect(screen.getByText('正在读取会话…')).toBeTruthy();

    document.body.innerHTML = '';
    render(SceneTimeline, {
      props: { conversations: [], conversationFor: () => undefined, commands: [], commandOutputFor: () => null },
    });
    expect(screen.getByText(/这个任务还没有现场记录/)).toBeTruthy();
  });
});

describe('现场时间线 · 命令回执', () => {
  it('在跑的那条流式输出常显，不用点开', () => {
    render(SceneTimeline, {
      props: {
        conversations: [],
        conversationFor: () => undefined,
        commands: [cmd({ exit_code: null, command: 'cargo test' })],
        commandOutputFor: () => '正在跑的第 1 个用例…',
      },
    });

    const live = document.querySelector('.cmd-live');
    expect(live?.textContent).toContain('正在跑的第 1 个用例…');
    expect(document.querySelector('.rcpt.cmd.running')).toBeTruthy();
  });

  it('收口的命令折叠；点开才拉完整输出，退出码与「改写」两条都摆（决策 297）', async () => {
    const onloadCommand = vi.fn();
    render(SceneTimeline, {
      props: {
        conversations: [],
        conversationFor: () => undefined,
        commands: [
          cmd({ command: 'rtk read foo.ts', original_command: 'cat foo.ts', exit_code: 2 }),
        ],
        commandOutputFor: () => null,
        onloadCommand,
      },
    });

    const details = document.querySelector('details.rcpt.cmd') as HTMLDetailsElement;
    expect(details.open).toBe(false);
    expect(details.textContent).toContain('cat foo.ts');
    expect(details.textContent).toContain('改写');

    await fireEvent.click(details.querySelector('summary') as HTMLElement);
    expect(details.open).toBe(true);
    expect(onloadCommand).toHaveBeenCalledWith(7);
    expect(details.textContent).toContain('→ 实际执行：rtk read foo.ts');
    // preview 兜底也在
    expect(details.textContent).toContain('preview 行');
  });

  it('已经拿到完整/流式输出的命令点开不再重复发请求', async () => {
    const onloadCommand = vi.fn();
    render(SceneTimeline, {
      props: {
        conversations: [],
        conversationFor: () => undefined,
        commands: [cmd()],
        commandOutputFor: () => '完整输出全文',
        onloadCommand,
      },
    });

    const details = document.querySelector('details.rcpt.cmd') as HTMLDetailsElement;
    await fireEvent.click(details.querySelector('summary') as HTMLElement);
    expect(onloadCommand).not.toHaveBeenCalled();
    expect(details.textContent).toContain('完整输出全文');
  });
});

describe('现场时间线 · 过滤与窗口化（决策 319）', () => {
  it('关键词过滤收窄轮次；没命中说一句而不是空屏', async () => {
    render(SceneTimeline, {
      props: {
        conversations: [run({ run_id: 1 }), run({ run_id: 2, stage: 'review', node: 'validate_output' })],
        conversationFor: () => undefined,
        commands: [],
        commandOutputFor: () => null,
      },
    });

    expect(document.querySelectorAll('article.turn').length).toBe(2);
    const input = screen.getByLabelText('按阶段、节点、消息正文、命令行或输出过滤现场时间线');
    await fireEvent.input(input, { target: { value: 'review' } });
    expect(document.querySelectorAll('article.turn').length).toBe(1);

    await fireEvent.input(input, { target: { value: '查无此项' } });
    expect(screen.getByText('没有匹配的轮次。')).toBeTruthy();
  });

  it('单轮步骤超窗显尾部 50 条，「已省略」行点开补上一页', async () => {
    const messages: ChatMessage[] = Array.from({ length: 120 }, (_, i) => ({
      role: i % 2 === 0 ? ('user' as const) : ('assistant' as const),
      content: `消息 ${i + 1}`,
    }));
    render(SceneTimeline, {
      props: {
        conversations: [run()],
        conversationFor: () => conv(messages),
        commands: [],
        commandOutputFor: () => null,
      },
    });

    // 最新的一条永远在场；首屏显尾部 50 条（119 步 − 50 = 省略 69；
    // 尾窗里 user 步占一半——末位 assistant 已摘成收口话）
    expect(document.body.textContent).toContain('消息 120');
    expect(document.querySelectorAll('.who').length).toBe(25);
    expect(document.body.textContent).toContain('已省略前 69 条');
  });
});
