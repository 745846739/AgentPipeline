import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import type { ChatMessage, ConversationSummary, NodeConversation } from '../../api/types';
import ConversationViewer from './ConversationViewer.svelte';

/**
 * 会话页签的窗口化（spec list-windowing 票 03）：run 药丸墙过滤 + 选中 run 的
 * 消息列表切片显尾部。过滤纯函数自身的判据在 `lib/runFilter.test.ts`；
 * 切片判据在 `lib/windowSlice.test.ts`——这里钉的是接线：药丸墙怎么收窄、
 * 长会话首屏画多少、点「已省略」行能展开。
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

function longConversation(n: number): NodeConversation {
  return conv(
    Array.from({ length: n }, (_, i) => ({
      role: i % 2 === 0 ? ('user' as const) : ('assistant' as const),
      content: `消息 ${i + 1}`,
    })),
  );
}

function bubbles(): HTMLElement[] {
  return Array.from(document.querySelectorAll('.msg'));
}

describe('会话页签：run 药丸过滤（票 03）', () => {
  it('过滤框收窄药丸墙；没命中时说一句而不是空屏', async () => {
    render(ConversationViewer, {
      props: {
        conversations: [
          run({ run_id: 1, stage: 'develop', node: 'execute' }),
          run({ run_id: 2, stage: 'review', node: 'validate_output' }),
        ],
        selectedRunId: null,
        onselect: () => {},
      },
    });

    expect(document.querySelectorAll('.runchip').length).toBe(2);
    const input = screen.getByLabelText('按阶段、节点、子代理或 run id 过滤 run 行');
    await fireEvent.input(input, { target: { value: 'review' } });
    expect(document.querySelectorAll('.runchip').length).toBe(1);

    await fireEvent.input(input, { target: { value: 'merge' } });
    expect(document.querySelectorAll('.runchip').length).toBe(0);
    expect(screen.getByText('没有匹配的 run 行。')).toBeTruthy();
  });
});

describe('会话页签：消息列表切片显尾部（票 03）', () => {
  it('500+ 条的长会话首屏只画 50 条，顶部一行「已省略」', () => {
    render(ConversationViewer, {
      props: {
        conversations: [run()],
        selectedRunId: 1,
        onselect: () => {},
        getConversation: () => longConversation(537),
      },
    });

    // 首屏 50 条：最新的两条（尾部）在场，最旧的不在
    expect(screen.getByText('消息 537')).toBeTruthy();
    expect(screen.getByText('消息 488')).toBeTruthy();
    expect(screen.queryByText('消息 487')).toBeNull();
    expect(screen.getByRole('button', { name: '已省略前 487 条，点此展开' })).toBeTruthy();
  });

  it('点「已省略」行：再画 50 条，省略计数跟着缩', async () => {
    render(ConversationViewer, {
      props: {
        conversations: [run()],
        selectedRunId: 1,
        onselect: () => {},
        getConversation: () => longConversation(537),
      },
    });

    await fireEvent.click(screen.getByRole('button', { name: /已省略前 487 条/ }));
    expect(screen.getByText('消息 438')).toBeTruthy();
    expect(screen.queryByText('消息 437')).toBeNull();
    expect(screen.getByRole('button', { name: '已省略前 437 条，点此展开' })).toBeTruthy();
  });

  it('没超上限：全量渲染，不出现「已省略」行', () => {
    render(ConversationViewer, {
      props: {
        conversations: [run()],
        selectedRunId: 1,
        onselect: () => {},
        getConversation: () => longConversation(30),
      },
    });

    expect(bubbles().length).toBe(30);
    expect(screen.queryByRole('button', { name: /已省略前/ })).toBeNull();
  });
});
