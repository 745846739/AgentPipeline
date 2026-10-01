/**
 * reducer 的**累积代价** bench（决策 361，票 04）。
 *
 * 只记录、不设闸（同 `taskScene.bench.ts`）。它要回答的是决策 362 那条病的量级：
 * `liveDeltas` 每来一条增量都要把整个数组拷一遍再写回 state，而这份代价随累积条数近似
 * 线性上升。**决策 362 已给这扇窗上了上限**（`LIVE_WINDOW_LIMIT = 10_000`，环形缓冲丢最早），
 * 故这里量的是「窗口封顶之后单条追加还花多少」——三档灌入量（1 千 / 正好到顶 / 到顶之后
 * 再灌四倍）的读数**应当持平**，那就是「上限真的生效」的另一种证据。
 *
 * 全是纯函数，故可重复、无网络、无浏览器。
 */

import { bench, describe } from 'vitest';

import type { ConversationDeltaEvent } from '../api/types';
import { emptyTaskDetailState, reduceTaskDetail } from './reduce';

/** 灌入量（条）：未到顶 / 正好到顶 / 到顶之后再灌四倍（窗口应原样封住）。 */
const FEED = [1_000, 10_000, 40_000] as const;

function delta(): ConversationDeltaEvent {
  return {
    type: 'conversation_delta',
    task_id: 't1',
    branch: 'develop',
    run_id: 1,
    agent_type: 'main',
    channel: 'content',
    role: 'assistant',
    text: '一段增量文本',
    prompt_tokens: 0,
    completion_tokens: 3,
  };
}

/** 摆到「已经收过 `n` 条增量」的那一态（上限由 reducer 自己施加，这里不预判）。 */
function seeded(n: number) {
  let state = emptyTaskDetailState();
  for (let i = 0; i < n; i += 1) state = reduceTaskDetail(state, delta());
  return state;
}

describe('reduceTaskDetail · liveDeltas 累积下的单条追加', () => {
  for (const n of FEED) {
    const state = seeded(n);
    bench(`已灌入 ${n.toLocaleString('en-US')} 条 → 再来一条`, () => {
      reduceTaskDetail(state, delta());
    });
  }
});
