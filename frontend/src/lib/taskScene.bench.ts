/**
 * `buildTaskScene` 的纯函数 bench（决策 361，票 04）。
 *
 * **只记录、不设闸**：这里的数字是毫秒，而毫秒在 CI 上必然抖动，做成断言就是引进一条
 * 必然 flaky 的门。它回答的是另一个问题——**归约代价随规模怎么长**——而这条曲线决定
 * 「窗口化 / 虚拟滚动要不要提上日程」。运行方式见 `docs/testing.md` §10（`npm run bench`），
 * **不进 `make check`**。
 *
 * 规模取 spec「排查结论」里那组实测数据：本机最坏 48 轮 / 1303 消息 / 433 命令（决策 349
 * 合并现场页签之后的一次全量归约，本机实测 3.3 ms）。再加一档**四倍**规模，用来看它是不是
 * 线性的——单点数字说明不了趋势。
 */

import { bench, describe } from 'vitest';

import type {
  ChatMessage,
  ConversationSummary,
  NodeCommand,
  NodeConversation,
} from '../api/types';
import { buildTaskScene } from './taskScene';

interface Fixture {
  conversations: ConversationSummary[];
  conversationFor: (runId: number) => NodeConversation | undefined;
  commands: NodeCommand[];
}

/**
 * 造一份「像真的」现场：每轮含系统段 / 用户段 / 若干 assistant+tool 往返 / 思考留痕，
 * 每轮挂若干命令。形状照 `taskScene.ts` 的归约输入来（少了任何一段都会让被测路径短掉，
 * 那样测出来的毫秒数没有意义）。
 */
function scene(runs: number, msgsPerRun: number, cmdsPerRun: number): Fixture {
  const conversations: ConversationSummary[] = [];
  const full = new Map<number, NodeConversation>();

  for (let r = 0; r < runs; r += 1) {
    const runId = r + 1;
    conversations.push({
      run_id: runId,
      stage: 'develop',
      node: 'execute',
      attempt: 1,
      agent_type: 'main',
      parent_run_id: null,
      prompt_tokens: 1_200,
      completion_tokens: 800,
      status: 'success',
      archived_at: null,
    });
    const messages: ChatMessage[] = [{ role: 'user', content: `第 ${runId} 轮的用户段` }];
    for (let m = 0; m < msgsPerRun; m += 1) {
      messages.push({
        role: 'assistant',
        content: `第 ${runId} 轮第 ${m} 段回话：这里是一段中等长度的正文，用来把 markdown 渲染之前的文本量撑到真实量级。`,
      });
      messages.push({
        role: 'tool',
        content: `工具回执 ${m}：\n${'一行输出\n'.repeat(6)}`,
        name: 'run_command',
      });
    }
    full.set(runId, {
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
      prompt_tokens: 1_200,
      completion_tokens: 800,
      system_prompt: '系统段：你是一个执行代理。'.repeat(40),
      user_prompt: '用户段：把这个阶段做完。'.repeat(20),
      reasoning: '思考留痕：先看现状，再看缺口。'.repeat(30),
      created_at: '2026-10-01T10:00:00Z',
    });
  }

  const commands: NodeCommand[] = [];
  let id = 1;
  for (let r = 0; r < runs; r += 1) {
    for (let c = 0; c < cmdsPerRun; c += 1) {
      commands.push({
        id: id++,
        task_id: 't1',
        run_id: r + 1,
        stage: 'develop',
        node: 'execute',
        source: 'agent',
        command: `rtk npm test -- --run  #${c}`,
        original_command: null,
        cwd: '/tmp/wt',
        exit_code: 0,
        stdout_path: null,
        stdout_preview: 'PASS src/lib.test.ts\n'.repeat(8),
        stderr_preview: null,
        duration_ms: 420,
        started_at: '2026-10-01T10:00:00Z',
        finished_at: '2026-10-01T10:00:01Z',
      });
    }
  }

  return {
    conversations,
    conversationFor: (runId) => full.get(runId),
    commands,
  };
}

describe('buildTaskScene · 落地数据全量归约', () => {
  const medium = scene(48, 12, 9); // ≈ 48 轮 / 1.2k 消息 / 432 命令：实测最坏那一档
  const large = scene(192, 12, 9); // 四倍：看曲线是不是线性

  bench('48 轮 / ~1.2k 消息 / 432 命令', () => {
    buildTaskScene({
      ...medium,
      liveDeltas: [],
      liveTools: [],
      commandOutputFor: () => null,
    });
  });

  bench('192 轮 / ~4.8k 消息 / 1728 命令（四倍规模）', () => {
    buildTaskScene({
      ...large,
      liveDeltas: [],
      liveTools: [],
      commandOutputFor: () => null,
    });
  });
});
