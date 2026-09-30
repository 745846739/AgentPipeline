import type {
  ChatMessage,
  CommandSource,
  ConversationSummary,
  NodeCommand,
  NodeConversation,
} from '../api/types';
import type { LiveDelta, LiveTool } from '../realtime/reduce';
import { summarizeArgs } from './format';

/**
 * 任务详情「现场」页签的时间线归约（决策 349）。
 *
 * ## 为什么整体归约、而不是在组件里各画各的
 *
 * 「会话」与「命令与输出」原本是两个页签、两套版面（消息气泡墙 / 命令行表）；对讲台把
 * 同一类东西（话、工具调用、过程）归成一叠**轮**——名牌 + 按发生顺序的步骤 + 收口话。
 * 合并成同一个展现形式时，真正的判断长在**行与行的关系**上：命令归哪一轮、消息怎么
 * 折成步骤、流式增量接到哪一头上。这些判据全部住在这里（纯函数，`SceneTimeline.svelte`
 * 只按序画），与 `lib/talkTurns.ts` 同一条纪律：**判断归模块，模板只渲染**。
 *
 * ## 四条边界
 *
 * **① run 的顺序按「时刻、缺了回落」**：`ConversationSummary` 没有 `created_at`
 * （时间在完整会话那一份上）。全局排序取「完整会话的 `created_at`，缺了回落该轮命令的
 * 最早 `started_at`，再缺落空（排末尾、按 run_id 定序）」——排序必须确定
 * （同 `lib/talkTurns.ts` 的要求）。
 *
 * **② 命令按 `run_id` 归轮；没有归属的（`run_id = null`，或指向没有会话轮的 run）按
 * `stage · node` 分组成合成轮**——「只有命令没有会话的节点」就是这样进时间线的，
 * 与会话轮同一个形状（名牌 + 步骤）。
 *
 * **③ 消息折步骤不读时钟**：`messages_json` 没有时间戳，轮内的顺序就是数组顺序；
 * 命令步骤排在该轮消息步骤之后（按 `started_at`）——消息与命令是两本账，轮内不假装
 * 能精确交织。
 *
 * **④ 流式增量（`liveDeltas` / `liveTools`）按 `run_id` 接到各自的轮尾**；摘要列表里
 * 还没有那个 run 时合成一条 live 轮兜在末尾——刷新的空窗期里流式输出不落地、也不能丢。
 */

/** 轮里的一步：三类（话 / 工具回执 / 命令回执）归成一个渲染形状。 */
export interface SceneStep {
  /** 渲染键，由本模块按来源与序号编好，模板不自己数。 */
  key: string;
  kind: 'text' | 'tool' | 'command';
  /** text 步的角色（user / assistant / system / tool）。tool / command 步为 null。 */
  role: ChatMessage['role'] | null;
  /** text 步的正文；其余步为空串。 */
  text: string;
  /** 这一步**正在冒**（流式增量）：只有 run 在飞时末尾才可能为真。 */
  streaming: boolean;
  /** 工具回执（模型发起的调用；模型侧的账）。 */
  tool: SceneTool | null;
  /** 命令回执（执行器侧的账，带退出码与输出）。 */
  command: SceneCommand | null;
}

/** 工具回执：摘要行走名牌那一行，参数与结果收在展开体里。 */
export interface SceneTool {
  name: string;
  /** 摘要行上的参数缩略（`summarizeArgs` 那一份）。 */
  argsSummary: string;
  /** 完整参数原串（能展开看；老数据只有摘要时与 `argsSummary` 同文）。 */
  args: string;
  /** 结果正文（tool 消息配上的那一份；空 = 还没回来）。 */
  result: string;
  phase: 'running' | 'ok' | 'bad';
}

/** 命令回执（决策 297 的两条都摆：折叠行原串，展开时带实际执行的那条）。 */
export interface SceneCommand {
  id: number;
  /** 折叠行显示的那条：有原串显示原串（`original_command ?? command`）。 */
  command: string;
  /** 实际执行的命令串。 */
  actualCommand: string;
  /** 真的改写过（展开时才需要把两条都摆出来）。 */
  rewritten: boolean;
  source: CommandSource;
  startedAt: string;
  exitCode: number | null;
  durationMs: number | null;
  /**
   * 输出正文：完整 / 流式取调用方解析的那一份，缺了回落 `stdout_preview`；
   * 都没有时为 null（`stdoutFile` 为真时组件显示「未取回」的实情）。
   */
  output: string | null;
  /** 完整输出的读取错误（票 12 / R2-16：读不回来就说失败）。 */
  outputError: string | null;
  /** 台账说这条命令卸载过完整输出文件（`stdout_path` 非空）。 */
  stdoutFile: boolean;
}

/** 现场时间线上的一轮（渲染形状；`SceneTimeline.svelte` 直接消费它）。 */
export interface SceneTurn {
  key: string;
  /** 名牌：`stage · node`，会话轮带尝试次数与子代理名。 */
  name: string;
  /** 名牌旁的次级读数（子代理名）。 */
  sub: string;
  /** 排序时刻（RFC3339；空串排末尾——在飞轮没有时刻）。 */
  at: string;
  /** 这个 run 此刻在冒增量（流式光标挂在收口话 / 末步上）。 */
  streaming: boolean;
  /** 会话轮的 run_id；合成轮（命令分组 / live 兜底）为 null。 */
  runId: number | null;
  /** run 的台账状态（success / timeout / failed / …）；非 success 名牌旁亮一枚。 */
  status: string | null;
  /** 完整会话读到了没有（没读到时轮里显示「正在读取会话…」）。 */
  loaded: boolean;
  steps: SceneStep[];
  /**
   * 收口话：最后一条 assistant 正文（对讲台的「回话位」）。还在冒增量时它就是
   * 正在说的那一句（`closingStreaming` 挂光标，等宽不渲染 markdown）。
   */
  closing: string;
  closingStreaming: boolean;
  /** 会话的元数据卡（原样递给 `MetadataCard`）。 */
  metadata: unknown;
  tokens: { prompt: number; completion: number } | null;
}

/** {@link buildTaskScene} 的输入：数据与两个输出解析回调，全是平凡值。 */
export interface TaskSceneInput {
  conversations: ConversationSummary[];
  /** 完整会话（已装载的那几轮；没装载的轮 `loaded = false`）。 */
  conversationFor: (runId: number) => NodeConversation | undefined;
  commands: NodeCommand[];
  liveDeltas: LiveDelta[];
  liveTools: LiveTool[];
  /** 命令输出解析（完整 > 流式；账在组件 / store 手里，归约只问）。 */
  commandOutputFor: (c: NodeCommand) => string | null;
  commandErrorFor?: (c: NodeCommand) => string | null;
}

/** 未编号的一步（{@link keyedSteps} 补键用）。 */
type StepDraft = Omit<SceneStep, 'key'>;

function textStep(role: ChatMessage['role'], text: string, streaming = false): StepDraft {
  return { kind: 'text', role, text, streaming, tool: null, command: null };
}

function toolStep(name: string, args: string, result: string, phase: SceneTool['phase']): StepDraft {
  return {
    kind: 'tool',
    role: null,
    text: '',
    streaming: false,
    tool: { name, argsSummary: summarizeArgs(args), args, result, phase },
    command: null,
  };
}

function commandStep(c: NodeCommand, input: TaskSceneInput): StepDraft {
  const original = c.original_command;
  return {
    kind: 'command',
    role: null,
    text: '',
    streaming: false,
    tool: null,
    command: {
      id: c.id,
      command: original ?? c.command,
      actualCommand: c.command,
      rewritten: original !== null && original !== c.command,
      source: c.source,
      startedAt: c.started_at,
      exitCode: c.exit_code,
      durationMs: c.duration_ms,
      output: input.commandOutputFor(c) ?? c.stdout_preview,
      outputError: input.commandErrorFor?.(c) ?? null,
      stdoutFile: c.stdout_path !== null,
    },
  };
}

function byStartedAt(a: NodeCommand, b: NodeCommand): number {
  if (a.started_at !== b.started_at) return a.started_at < b.started_at ? -1 : 1;
  return a.id - b.id;
}

/** 同一 run 的连续同角色增量并成一条（与旧会话页签的 `mergedDeltas` 同一判据）。 */
function mergeDeltas(deltas: LiveDelta[]): { role: string; agent: string; text: string }[] {
  const out: { role: string; agent: string; text: string }[] = [];
  for (const d of deltas) {
    const last = out[out.length - 1];
    if (last && last.role === d.role && last.agent === d.agent_type) last.text += d.text;
    else out.push({ role: d.role, agent: d.agent_type, text: d.text });
  }
  return out;
}

/**
 * 一轮会话的 `messages_json` → 步骤草稿 + 收口话。
 *
 * tool 消息按 `tool_call_id` **配回**它所属的那次调用（assistant 消息里 `tool_calls`
 * 的那一步），结果进回执的展开体；配不上的（老数据 / 截断）自成一条只有结果的回执，
 * 空 content 的 tool 行照旧剔除（旧会话页签的既有判据）。最后一条 assistant 正文是
 * 收口话，从步骤里摘出去——它就是「回话位」。
 */
function stepsFromMessages(messages: ChatMessage[]): { steps: StepDraft[]; closing: string } {
  const steps: StepDraft[] = [];
  const callIndex = new Map<string, number>();
  for (const m of messages) {
    if (m.role === 'system') {
      steps.push(textStep('system', m.content ?? ''));
    } else if (m.role === 'user') {
      steps.push(textStep('user', m.content ?? ''));
    } else if (m.role === 'assistant') {
      if (m.content) steps.push(textStep('assistant', m.content));
      for (const call of m.tool_calls ?? []) {
        callIndex.set(call.id, steps.length);
        steps.push(toolStep(call.function.name, call.function.arguments, '', 'ok'));
      }
    } else if (m.content) {
      // tool 结果：配回所属调用；配不上自成一条（只有结果、没有参数可摊）。
      const idx = m.tool_call_id != null ? callIndex.get(m.tool_call_id) : undefined;
      if (idx !== undefined) {
        const base = steps[idx];
        const tool = base.tool as SceneTool;
        steps[idx] = { ...base, tool: { ...tool, result: m.content } };
      } else {
        steps.push(toolStep(m.name ?? 'tool', '', m.content, 'ok'));
      }
    }
  }
  // 收口话 = 最后一步若是 assistant 正文，摘出去。
  const last = steps[steps.length - 1];
  if (last && last.kind === 'text' && last.role === 'assistant') {
    steps.pop();
    return { steps, closing: last.text };
  }
  return { steps, closing: '' };
}

/** 步骤草稿按来源编号补键（命令天然有 id，其余按 kind 计数；模板不自己数）。 */
function keyedSteps(drafts: StepDraft[], turnKey: string): SceneStep[] {
  const counter = new Map<string, number>();
  return drafts.map((d) => {
    const source = d.kind === 'command' ? `c${d.command?.id ?? ''}` : d.kind;
    const i = counter.get(source) ?? 0;
    counter.set(source, i + 1);
    return { ...d, key: `${turnKey}-${source}${i}` };
  });
}

/** 现场时间线归约本体（边界见模块注释）。 */
export function buildTaskScene(input: TaskSceneInput): SceneTurn[] {
  const { conversations, commands, liveDeltas, liveTools } = input;
  const deltasByRun = new Map<number, LiveDelta[]>();
  for (const d of liveDeltas) {
    const list = deltasByRun.get(d.run_id) ?? [];
    list.push(d);
    deltasByRun.set(d.run_id, list);
  }
  const toolsByRun = new Map<number, LiveTool[]>();
  for (const t of liveTools) {
    const list = toolsByRun.get(t.run_id) ?? [];
    list.push(t);
    toolsByRun.set(t.run_id, list);
  }

  const knownRuns = new Set(conversations.map((c) => c.run_id));
  /** run_id → 该轮的命令（归轮用）；没有归属的进孤儿桶。 */
  const commandsByRun = new Map<number, NodeCommand[]>();
  const orphans: NodeCommand[] = [];
  for (const c of commands) {
    if (c.run_id !== null && knownRuns.has(c.run_id)) {
      const list = commandsByRun.get(c.run_id) ?? [];
      list.push(c);
      commandsByRun.set(c.run_id, list);
    } else {
      orphans.push(c);
    }
  }

  interface Episode {
    turn: SceneTurn;
    rank: number;
  }
  const episodes: Episode[] = [];

  for (const summary of conversations) {
    const runId = summary.run_id;
    const full = input.conversationFor(runId);
    const runCommands = commandsByRun.get(runId) ?? [];
    const deltas = deltasByRun.get(runId) ?? [];
    const key = `r${runId}`;
    const { steps: msgSteps, closing: landedClosing } = full
      ? stepsFromMessages(full.messages_json)
      : { steps: [] as StepDraft[], closing: '' };
    const drafts: StepDraft[] = [...msgSteps];
    for (const t of toolsByRun.get(runId) ?? []) {
      drafts.push(
        toolStep(
          t.tool,
          t.args_summary,
          '',
          t.phase === 'start' ? 'running' : t.phase === 'error' ? 'bad' : 'ok',
        ),
      );
    }
    drafts.push(...[...runCommands].sort(byStartedAt).map((c) => commandStep(c, input)));
    // 收口话：落地的那一份优先；没有落地会话（或它没有收尾正文）时，末条 assistant
    // 增量顶到回话位——它此刻多半正在冒。
    let closing = landedClosing;
    let closingStreaming = false;
    const merged = mergeDeltas(deltas);
    if (!closing && merged.length > 0 && merged[merged.length - 1].role === 'assistant') {
      closing = merged[merged.length - 1].text;
      closingStreaming = deltas.length > 0;
      merged.pop();
    }
    for (const d of merged) drafts.push(textStep(d.role as ChatMessage['role'], d.text));
    // 流式光标只有一处：收口位在冒时挂收口话；否则挂最后一条正文步（最新的输出在哪，
    // 光标就在哪——工具回执自己带「运行中…」，不需要光标）。
    if (!closingStreaming && deltas.length > 0 && drafts.length > 0) {
      const lastDraft = drafts[drafts.length - 1];
      if (lastDraft.kind === 'text') {
        drafts[drafts.length - 1] = { ...lastDraft, streaming: true };
      }
    }
    // 排序时刻：完整会话的 created_at → 该轮命令的最早 started_at → 空（排末尾）。
    const at =
      full?.created_at ??
      [...runCommands].sort(byStartedAt)[0]?.started_at ??
      '';
    episodes.push({
      rank: 0,
      turn: {
        key,
        name: `${summary.stage} · ${summary.node}`,
        sub: summary.agent_type !== 'main' ? summary.agent_type : '',
        at,
        streaming: deltas.length > 0,
        runId,
        status: summary.status,
        loaded: full !== undefined,
        steps: keyedSteps(drafts, key),
        closing,
        closingStreaming,
        metadata: full?.metadata_json ?? null,
        tokens: { prompt: summary.prompt_tokens, completion: summary.completion_tokens },
      },
    });
  }

  // 孤儿命令按 stage · node 分组（保留首见顺序）：只有命令没有会话的节点由此进时间线。
  const orphanGroups: { key: string; commands: NodeCommand[]; at: string }[] = [];
  const groupIndex = new Map<string, number>();
  for (const c of orphans) {
    const gk = `${c.stage}|${c.node}`;
    const i = groupIndex.get(gk);
    if (i === undefined) {
      groupIndex.set(gk, orphanGroups.length);
      orphanGroups.push({ key: `g${orphanGroups.length}`, commands: [c], at: c.started_at });
    } else {
      const group = orphanGroups[i];
      group.commands.push(c);
      if (c.started_at < group.at) group.at = c.started_at;
    }
  }
  for (const group of orphanGroups) {
    const steps = [...group.commands].sort(byStartedAt).map((c) => commandStep(c, input));
    episodes.push({
      rank: 1,
      turn: {
        key: group.key,
        name: `${group.commands[0].stage} · ${group.commands[0].node}`,
        sub: '',
        at: group.at,
        streaming: false,
        runId: null,
        status: null,
        loaded: true,
        steps: keyedSteps(steps, group.key),
        closing: '',
        closingStreaming: false,
        metadata: null,
        tokens: null,
      },
    });
  }

  // 摘要列表还没有那个 run、但增量已经在冒：合成一条 live 轮兜在末尾（刷新空窗期）。
  for (const [runId, deltas] of deltasByRun) {
    if (knownRuns.has(runId)) continue;
    const key = `live${runId}`;
    const merged = mergeDeltas(deltas);
    const drafts: StepDraft[] = [];
    for (const t of toolsByRun.get(runId) ?? []) {
      drafts.push(
        toolStep(
          t.tool,
          t.args_summary,
          '',
          t.phase === 'start' ? 'running' : t.phase === 'error' ? 'bad' : 'ok',
        ),
      );
    }
    drafts.push(
      ...[...(commandsByRun.get(runId) ?? [])].sort(byStartedAt).map((c) => commandStep(c, input)),
    );
    let closing = '';
    let closingStreaming = false;
    if (merged.length > 0 && merged[merged.length - 1].role === 'assistant') {
      closing = merged[merged.length - 1].text;
      closingStreaming = true;
      merged.pop();
    }
    for (const d of merged) drafts.push(textStep(d.role as ChatMessage['role'], d.text));
    episodes.push({
      rank: 2,
      turn: {
        key,
        name: deltas[0]?.agent_type || '现场',
        sub: '',
        at: '',
        streaming: true,
        runId,
        status: null,
        loaded: false,
        steps: keyedSteps(drafts, key),
        closing,
        closingStreaming,
        metadata: null,
        tokens: null,
      },
    });
  }

  return episodes
    .sort((a, b) => {
      // 两边都没时刻（在飞 / 缺数据）：rank 先分（会话轮 → 合成轮 → live 兜底），
      // 同档再按 run_id 定序——排序必须确定，不许随输入顺序漂。
      if (a.turn.at === '' && b.turn.at === '') {
        if (a.rank !== b.rank) return a.rank - b.rank;
        return (a.turn.runId ?? 0) - (b.turn.runId ?? 0);
      }
      if (a.turn.at !== b.turn.at) {
        if (a.turn.at === '') return 1;
        if (b.turn.at === '') return -1;
        return a.turn.at < b.turn.at ? -1 : 1;
      }
      return a.rank - b.rank;
    })
    .map((e) => e.turn);
}

/**
 * 现场时间线的**轮级**关键词过滤：命中名牌、次级读数、状态、任一步骤的正文 / 工具名 /
 * 命令串或收口话即留下。大小写不敏感；空查询全留。
 */
export function sceneTurnMatches(turn: SceneTurn, rawQuery: string): boolean {
  const q = rawQuery.trim().toLowerCase();
  if (!q) return true;
  const haystacks: string[] = [turn.name, turn.sub, turn.status ?? '', turn.closing];
  for (const s of turn.steps) {
    haystacks.push(s.text);
    if (s.tool) haystacks.push(s.tool.name, s.tool.argsSummary, s.tool.result);
    if (s.command)
      haystacks.push(s.command.command, s.command.actualCommand, s.command.output ?? '');
  }
  return haystacks.some((h) => h.toLowerCase().includes(q));
}
