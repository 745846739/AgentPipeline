import { describe, expect, it } from 'vitest';
import type { ConversationDeltaEvent } from '../api/types';
import {
  appendForemanDelta,
  beginForemanStream,
  emptyForemanStream,
  failForemanStream,
  settleForemanStream,
} from './foreman';

/**
 * 值班长流式归约（票 03 的用例口径）：文本累积的顺序、收尾、断流三件事。
 *
 * 与 `reduce.test.ts` 同一姿态的纯函数测试——不触网、不读时钟，故不需要 DOM /
 * fetch 替身就能钉住「不丢字」这条硬约束。
 */

/** 工头增量事件（task_id 空、agent_type = "foreman"，决策 182⑥）。 */
function delta(text: string): ConversationDeltaEvent {
  return {
    type: 'conversation_delta',
    task_id: '',
    branch: '',
    run_id: 0,
    agent_type: 'foreman',
    role: 'assistant',
    text,
    prompt_tokens: 1,
    completion_tokens: 1,
  };
}

describe('foreman 流式归约', () => {
  it('增量按到达顺序累积，且累积期间保持流式态', () => {
    let state = beginForemanStream();
    expect(state.streaming).toBe(true);

    state = appendForemanDelta(state, delta('夜班'));
    state = appendForemanDelta(state, delta('安静，'));
    state = appendForemanDelta(state, delta('没有待办。'));

    expect(state.text).toBe('夜班安静，没有待办。');
    expect(state.streaming).toBe(true);
    expect(state.error).toBeNull();
  });

  it('非工头增量 / 非增量事件旁落（返回同一 state）', () => {
    const state = appendForemanDelta(beginForemanStream(), delta('已到'));
    // 别的任务的增量（agent_type = main）不得拼进值班长的话里
    expect(appendForemanDelta(state, { ...delta('别人的'), agent_type: 'main' })).toBe(state);
    // 同一条流上的其它事件类型与文本无关
    expect(
      appendForemanDelta(state, {
        type: 'tool_event',
        task_id: '',
        branch: '',
        run_id: 0,
        tool: 'read_task',
        phase: 'end',
        args_summary: 'x',
      }),
    ).toBe(state);
  });

  it('收尾：非空回话收敛为回话，并熄灭方块光标', () => {
    const streamed = appendForemanDelta(beginForemanStream(), delta('半句'));
    const settled = settleForemanStream(streamed, '完整回话');
    expect(settled.text).toBe('完整回话');
    expect(settled.streaming).toBe(false);
    expect(settled.error).toBeNull();
  });

  it('收尾：空 / 全空白回话不清掉已到达的文字', () => {
    const streamed = appendForemanDelta(beginForemanStream(), delta('已到达的部分'));
    expect(settleForemanStream(streamed, '').text).toBe('已到达的部分');
    expect(settleForemanStream(streamed, '   \n ').text).toBe('已到达的部分');
    expect(settleForemanStream(streamed, null).text).toBe('已到达的部分');
    // 两者皆空仍是空，不凭空造一句
    expect(settleForemanStream(emptyForemanStream(), '').text).toBe('');
  });

  it('断流：已收到的部分原文保留，只多一个说明', () => {
    let state = beginForemanStream();
    state = appendForemanDelta(state, delta('我查到 develop 工位'));
    const failed = failForemanStream(state, '连接中断');
    expect(failed.text).toBe('我查到 develop 工位');
    expect(failed.streaming).toBe(false);
    expect(failed.error).toBe('连接中断');
  });

  it('开新一轮：丢掉上一轮的残留（上一轮的文字不得串进这一轮）', () => {
    const previous = appendForemanDelta(beginForemanStream(), delta('上一轮的话'));
    expect(previous.text).toBe('上一轮的话');
    expect(beginForemanStream().text).toBe('');
  });
});
