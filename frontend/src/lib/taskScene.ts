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
 *
 * **⑤ 直播流按到达序交织折步（决策 359①）**：增量与工具回执靠归约发的同一只到达序
 * （`seq`）归并，连续同类并步、换类另起——「先想 → 查 → 再想 → 说」保真，工具回执
 * 不再整体堆在正文上方；`reasoning` 声道自成思考步（决策 244）。**流式与否由 run 的
 * 台账状态把关**（增量只进不出，落地后台账不再报 running）：跑完的轮不再亮光标。
 *
 * **⑥ 多次尝试分主次（决策 359③）**：同一 `stage · node · agent` 里 attempt 是后端
 * 计的重试序，比最新一代小的整轮折起（`primary = false`），内容一个字不删。
 *
 * **⑦ 轮首的 prompt 与落地思考（决策 360）**：完整会话读齐后，阶段 prompt（组装后的
 * 两段原文快照，决策 211② 落的那两列）与落库的 `reasoning`（决策 360 起新列）插在
 * 轮首、转录之前——prompt 是这一轮的输入，思考是动笔前的草稿。直播里的 reasoning
 * 声道在落地思考在场时不再折步：同一份思考只摆一遍。
 */

/** 轮里的一步：五类（阶段 prompt / 话 / 思考 / 工具回执 / 命令回执）归成一个渲染形状。 */
export interface SceneStep {
  /** 渲染键，由本模块按来源与序号编好，模板不自己数。 */
  key: string;
  kind: 'prompt' | 'text' | 'thinking' | 'tool' | 'command';
  /** text 步的角色（user / assistant / system / tool）。其余步为 null。 */
  role: ChatMessage['role'] | null;
  /** text / thinking 步的正文；其余步为空串。 */
  text: string;
  /** 这一步**正在冒**（流式增量）：只有 run 在飞时末尾才可能为真。 */
  streaming: boolean;
  /** 阶段 prompt（决策 360：组装后的两段原文快照）；其余步为 null。 */
  prompt: ScenePrompt | null;
  /** 工具回执（模型发起的调用；模型侧的账）。 */
  tool: SceneTool | null;
  /** 命令回执（执行器侧的账，带退出码与输出）。 */
  command: SceneCommand | null;
}

/** 阶段 prompt 的两段原文（决策 211② 的快照列，决策 360 起上现场时间线）。 */
export interface ScenePrompt {
  system: string | null;
  user: string | null;
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
  /**
   * 会话轮的尝试号（后端按 (task, stage, node) 计的重试序；子代理挂父节点的号）。
   * 合成轮 / live 轮为 1。名牌上 attempt > 1 才亮「第 N 次」。
   */
  attempt: number;
  /**
   * 同一 `stage · node · agent` 里是不是**最新一代**（决策 359③）：旧一代整轮折起、
   * 最新一代全幅展示——多次重试的主次就靠它分。同代并行的子代理（同 attempt）都是主。
   */
  primary: boolean;
  /** run 的台账状态（success / timeout / failed / …）；非 success 名牌旁亮一枚。 */
  status: string | null;
  /** 完整会话读到了没有（没读到时轮里显示「正在读取会话…」）。 */
  loaded: boolean;
  /**
   * 这个 run 的直播增量被上限丢弃过（决策 362①）：折叠步序最前摆一行非交互的
   * 「更早的增量已省略」。丢掉的没落库、取不回来，故**不可交互**——不能复用 `MoreRow`
   * 的「加载更多」（那会撒谎）。
   */
  droppedLive: boolean;
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
  /**
   * 被条数上限丢弃过增量的 run（决策 362①）。归约只据此决定**哪几轮**摆省略行——
   * 丢掉的条数与正文本就不在手里。
   */
  liveDroppedRuns?: Record<number, true>;
  /** 命令输出解析（完整 > 流式；账在组件 / store 手里，归约只问）。 */
  commandOutputFor: (c: NodeCommand) => string | null;
  commandErrorFor?: (c: NodeCommand) => string | null;
}

/** 未编号的一步（{@link keyedSteps} 补键用）。 */
type StepDraft = Omit<SceneStep, 'key'>;

function textStep(role: ChatMessage['role'], text: string, streaming = false): StepDraft {
  return { kind: 'text', role, text, streaming, prompt: null, tool: null, command: null };
}

/**
 * 思考步：直播里来自 `reasoning` 声道（决策 244），落地后来自会话行的 `reasoning` 列
 * （决策 360——此前推理只活在直播里，刷新即整段消失；落库的这份让历史轮也有得展开）。
 */
function thinkingStep(text: string): StepDraft {
  return {
    kind: 'thinking',
    role: null,
    text,
    streaming: false,
    prompt: null,
    tool: null,
    command: null,
  };
}

/** 阶段 prompt 步（决策 360）：两段原文都在才不空；默认收起，展开看全文。 */
function promptStep(system: string | null, user: string | null): StepDraft | null {
  if (!system && !user) return null;
  return {
    kind: 'prompt',
    role: null,
    text: '',
    streaming: false,
    prompt: { system, user },
    tool: null,
    command: null,
  };
}

function toolStep(name: string, args: string, result: string, phase: SceneTool['phase']): StepDraft {
  return {
    kind: 'tool',
    role: null,
    text: '',
    streaming: false,
    prompt: null,
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
    prompt: null,
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

/**
 * 直播流折步（决策 359①）：增量与工具回执**按到达序**交织成一步一步，再按对讲台的
 * 同一条规则归并（决策 273）——连续同类并进末步，换了种类另起一步。
 *
 * 替换掉的是「三只桶各攒各的」：工具一摞、命令一摞、正文一摊，拼出来工具永远整体堆在
 * 正文上方、思考与回话黏成一条——用户报的「工具执行一直在最上方、思考没分段」就是那个
 * 形状。`seq` 是归约时**同一只计数器**发的到达序（`LiveDelta.seq` / `LiveTool.seq`），
 * 两份数组的交织序只有它答得出来。
 *
 * 正文按**角色**分开并步（assistant 的话与 tool 角色的中途读数不是同一种发言）；
 * 思考步自成一类（决策 244：reasoning 是草稿，不是回话），同样连续并步。
 */
function foldLiveStream(deltas: LiveDelta[], tools: LiveTool[]): StepDraft[] {
  const items: { ord: number; d?: LiveDelta; t?: LiveTool }[] = [
    ...deltas.map((d) => ({ ord: d.seq, d })),
    ...tools.map((t) => ({ ord: t.seq, t })),
  ].sort((a, b) => a.ord - b.ord);

  const steps: StepDraft[] = [];
  for (const item of items) {
    if (item.t) {
      const t = item.t;
      steps.push({
        kind: 'tool',
        role: null,
        text: '',
        streaming: false,
        prompt: null,
        tool: {
          name: t.tool,
          // 有原文就在本地派摘要（与落地回执同一来源）；老后端只有服务端摘要时用它。
          argsSummary: t.args ? summarizeArgs(t.args) : t.args_summary,
          args: t.args,
          result: t.result,
          phase: t.phase === 'start' ? 'running' : t.phase === 'error' ? 'bad' : 'ok',
        },
        command: null,
      });
      continue;
    }
    const d = item.d;
    if (!d) continue;
    const draft =
      d.channel === 'reasoning'
        ? thinkingStep(d.text)
        : textStep(d.role as ChatMessage['role'], d.text);
    const last = steps[steps.length - 1];
    if (last && last.kind === draft.kind && last.role === draft.role) {
      last.text += draft.text;
      continue;
    }
    steps.push(draft);
  }
  return steps;
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
  const { conversations, commands, liveDeltas, liveTools, liveDroppedRuns = {} } = input;
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

  // 同一 stage · node · agent 的**最新一代**（决策 359③）：attempt 是后端按
  // (task, stage, node) 计的重试序（子代理挂父节点的号），比最大值小的都是旧一代。
  // 同代并行的子代理（同 attempt，如一次父执行里的两次 spawn）都是主——它们不是重试。
  const newestAttempt = new Map<string, number>();
  for (const s of conversations) {
    const gk = `${s.stage}|${s.node}|${s.agent_type}`;
    newestAttempt.set(gk, Math.max(newestAttempt.get(gk) ?? 1, s.attempt));
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
    const key = `r${runId}`;
    const { steps: msgSteps, closing: landedClosing } = full
      ? stepsFromMessages(full.messages_json)
      : { steps: [] as StepDraft[], closing: '' };
    // 直播流（决策 359①）：增量与工具回执按到达序交织折步。落地会话先排（历史），
    // 命令账最后排（两本账，不假装能精确交织）——收口话与流式光标在折步里落位，
    // 再拼上命令，免得命令顶走「正在说的那一句」。
    // 落地思考接管（决策 360）：会话行的 `reasoning` 就是同一批直播增量的最终去向，
    // 它在场时直播的 reasoning 声道不再折步——同一份思考只摆一遍。
    const reasoningLanded = !!full?.reasoning;
    const liveReasoning = reasoningLanded
      ? (deltasByRun.get(runId) ?? []).filter((d) => d.channel !== 'reasoning')
      : (deltasByRun.get(runId) ?? []);
    const stream = foldLiveStream(liveReasoning, toolsByRun.get(runId) ?? []);
    // run 的台账状态把关流式（决策 359①）：增量只进不出，run 落地后台账不再报
    // running——光标与「在冒」随之下线，否则跑完的轮永远亮着「正在说」。
    const live = liveReasoning.length > 0 && summary.status === 'running';
    let closing = landedClosing;
    let closingStreaming = false;
    if (!closing) {
      const last = stream[stream.length - 1];
      if (last && last.kind === 'text' && last.role === 'assistant') {
        stream.pop();
        closing = last.text;
        closingStreaming = live;
      }
    }
    // 流式光标只有一处：收口位在冒挂收口话；否则挂折步末尾的正文 / 思考（最新的输出
    // 在哪，光标就在哪——工具回执自己带「运行中…」，不需要光标）。
    if (!closingStreaming && live && stream.length > 0) {
      const last = stream[stream.length - 1];
      if (last.kind === 'text' || last.kind === 'thinking') {
        stream[stream.length - 1] = { ...last, streaming: true };
      }
    }
    // 轮首的两步（决策 360）：阶段 prompt（两段原文快照）在前，落地的思考在后——
    // prompt 是这一轮的输入，思考是模型动笔前的草稿，都排在转录（正文 / 工具回执）之前。
    const leading: StepDraft[] = [];
    const prompt = promptStep(full?.system_prompt ?? null, full?.user_prompt ?? null);
    if (prompt) leading.push(prompt);
    if (full?.reasoning) leading.push(thinkingStep(full.reasoning));
    const drafts: StepDraft[] = [
      ...leading,
      ...msgSteps,
      ...stream,
      ...[...runCommands].sort(byStartedAt).map((c) => commandStep(c, input)),
    ];
    // 排序时刻：完整会话的 created_at → 该轮命令的最早 started_at → 空（排末尾）。
    const at =
      full?.created_at ??
      [...runCommands].sort(byStartedAt)[0]?.started_at ??
      '';
    const gk = `${summary.stage}|${summary.node}|${summary.agent_type}`;
    episodes.push({
      rank: 0,
      turn: {
        key,
        name: `${summary.stage} · ${summary.node}`,
        sub: summary.agent_type !== 'main' ? summary.agent_type : '',
        at,
        streaming: live,
        runId,
        attempt: summary.attempt,
        primary: summary.attempt >= (newestAttempt.get(gk) ?? 1),
        status: summary.status,
        loaded: full !== undefined,
        droppedLive: liveDroppedRuns[runId] === true,
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
        attempt: 1,
        primary: true,
        status: null,
        loaded: true,
        droppedLive: false,
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
    const stream = foldLiveStream(deltas, toolsByRun.get(runId) ?? []);
    let closing = '';
    let closingStreaming = false;
    const last = stream[stream.length - 1];
    if (last && last.kind === 'text' && last.role === 'assistant') {
      stream.pop();
      closing = last.text;
      closingStreaming = true;
    } else if (last && (last.kind === 'text' || last.kind === 'thinking')) {
      stream[stream.length - 1] = { ...last, streaming: true };
    }
    const drafts: StepDraft[] = [
      ...stream,
      ...[...(commandsByRun.get(runId) ?? [])].sort(byStartedAt).map((c) => commandStep(c, input)),
    ];
    episodes.push({
      rank: 2,
      turn: {
        key,
        name: deltas[0]?.agent_type || '现场',
        sub: '',
        at: '',
        streaming: true,
        runId,
        attempt: 1,
        primary: true,
        status: null,
        loaded: false,
        droppedLive: liveDroppedRuns[runId] === true,
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
    // 阶段 prompt 的两段原文也在滤网里（决策 360：它是现场的一部分，找得到才点得开）
    if (s.prompt) haystacks.push(s.prompt.system ?? '', s.prompt.user ?? '');
    if (s.tool) haystacks.push(s.tool.name, s.tool.argsSummary, s.tool.result);
    if (s.command)
      haystacks.push(s.command.command, s.command.actualCommand, s.command.output ?? '');
  }
  return haystacks.some((h) => h.toLowerCase().includes(q));
}
