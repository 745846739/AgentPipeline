import type {
  ForemanAsk,
  ForemanBriefing,
  ForemanProposal,
  ForemanSegment,
  ForemanSession,
  ForemanTrace,
} from '../api/types';
import type { ForemanLiveStep, ForemanStreamState } from '../realtime/foreman';

/**
 * 对讲台时间线的回合构造（票 02；决策 251 的三块判断之一）。判断全在这里，
 * `Talk.svelte` 的 `turns` 只把四个响应式输入加配对谓词传进来。
 *
 * ## 为什么整体搬、而不是只抽每行的谓词
 *
 * 真正咬过人的 bug 长在**合并与排序**里——「台账那一行更全，故本地那行退场」的归属判定、
 * 提议与消息的相对位置，都写在行的**相互关系**上，逐行谓词那条路测不到（`buildTurns`
 * 吃一个平凡输入、返回整条时间线，关系于是能被整段断言钉住）。形状照 `lib/proposals.ts`：
 * **判断归模块，模板只渲染**。
 *
 * ## 三条边界
 *
 * **① 行分类不在这里判**（决策 252）：「这一行是什么」是后端给的 `m.kind`、
 * 「是不是值守播报」是 `m.proactive`——本模块**读字段、不解析正文哨兵**。此前组件里
 * `role` 三元式与 `startsWith` 两个判定点迟早不一致，正文即接口才是病。
 *
 * **② 配对与否是上游判好的布尔**（票 04 / 决策 259）：「这次失败是不是设备还没配对」
 * 由后端 403 带的 `kind` 说了算，在 `ApiError` 还在手的那层 catch 判好
 * （`lib/sharePairing.ts::isPairingRequired`），经 {@link TalkTurnsInput.pairingNeeded}
 * 传进来。本模块既不读报文也不认 `kind`——错误降级成流里的字符串之后 `kind` 已经丢了。
 *
 * **③ 不读时钟**：排序按 `created_at` 的 RFC3339 字符串比较，提议的过期态另有
 * `lib/proposals.ts` 按 `now` 算——故签名只有 `buildTurns(input)`，没有第二个参数。
 */

/** 时间线上的一轮（渲染形状；`Talk.svelte` 的模板直接消费它）。 */
export interface TurnView {
  key: string;
  /**
   * 发言者。`console` = **操作台记的一轮**（`role === 'system'`：提议的执行结果，决策 207）。
   *
   * 它必须与 `fm`（值班长的话）分开：那一行的内容是「提议已执行：…」，而**动手的是按下
   * 那颗钮的人**——挂在值班长的名牌下等于替它认领了它没做的事。三种角色、两种说话的立场，
   * 操作台是第三种。
   */
  kind: 'fm' | 'mine' | 'failed' | 'proposal' | 'console' | 'ask';
  /** 这一轮的**收口话**（live 轮是正在说的那一句）。 */
  content: string;
  /** 排进时间线的时刻（RFC3339）。三种在途轮（乐观轮 / 流式轮 / 失败轮）没有它，恒在末尾。 */
  at: string;
  /** 流式尾随方块光标（既有 `.streaming`，不新增动画位）。 */
  streaming: boolean;
  /** 断流/出错：这一轮只有已收到的部分。 */
  partial: boolean;
  /**
   * 这一轮**按发生顺序**的步骤（决策 273）：推理、中途说出口的话、工具调用。
   *
   * 三种来源归成同一个渲染形状（判断全在这里，模板只按序画）：
   * 落地轮读那一行的 `segments`（老行没有它，由 `thinking` + `traces` 两份聚合兜底）；
   * 在飞轮读流上的 {@link ForemanLiveStep}。
   * **收口那一句不在里面**——它是 {@link TurnView.content}，恒排在各步之后。
   */
  steps: TurnStep[];
  briefing: ForemanBriefing | null;
  /** 失败原因是「这台设备还没配对」（票 07；判据是后端给的 `kind`，决策 259）：只有它会挂配对入口。 */
  needsPairing: boolean;
  /** 提议轮带的那条提议（其余轮为 `null`）。 */
  proposal: ForemanProposal | null;
  /**
   * 提问轮带的问题与选项（决策 265，第三种轮型 `.turn.ask`；其余轮为 `null`）。
   *
   * 载荷由后端给（`message_wire` 的 `ask` 字段）——与 `kind` 同一条边界（决策 252）：
   * 读字段，不从正文里抠。
   */
  ask: ForemanAsk | null;
  /**
   * 这个问题**已被回答或被取代**（决策 265③）：它之后又出现过值班经理的话
   * （最大 `mine` 行 id 大于本行 id）——**纯派生，不存状态、不设过期机制**。
   * 每一轮都以一条 user 消息开场，故「下一轮开始了」⇔「这个问题已经不用再答」。
   * 为真时选项钮禁用（灰一档，与提议终态同口径）。
   */
  askAnswered: boolean;
  /**
   * 这一轮是**主动播报**（值守轮自己醒来说的话，票 06）。
   *
   * 与「回话」分开渲染的理由不是好看：回话是有人问的，播报是它自己说的——
   * 混成一种轮会让「它是不是在跟我说话」变成读不出来的一件事。
   */
  proactive: boolean;
  /**
   * 这一轮的归因类别词（决策 235①）：四类之一，由**后端解析**后随消息下来。
   *
   * `null` = 未定位或非助理轮——**不编一个假的类别**（决策 230 把「没有类别」也
   * 当成一项判据）。界面只渲染这一个词，不显示稳定标识、也不显示原因（那是排查面）。
   */
  attribution: string | null;
}

/**
 * 时间线上的**一步**（决策 273 的渲染形状）：三类步骤归成一个形状。
 *
 * 归并的理由是模板只该按序画：推理那一步与工具那一步在数据上本是两种东西
 * （一份文本 / 一次调用），而在时间线上它们是同一件事的两种样子——「这一轮里发生过的事」。
 */
export interface TurnStep {
  /** 渲染键（`<每个列表项>` 唯一的那个），由本模块按段序编号。 */
  key: string;
  kind: 'thinking' | 'text' | 'tool';
  /** 推理 / 中途那句话的原文；工具那一步是空串。 */
  text: string;
  /** 工具那一步的现场（其余步为 `null`）。 */
  tool: { name: string; argsSummary: string; state: 'running' | 'ok' | 'bad' } | null;
  /**
   * 这一步**正在攒**（在飞轮的末尾那一步）：推理的摘要因此写「正在想…」。
   * 落地轮恒假——落地那一行里每一步都已经收场了。
   */
  live: boolean;
}

/** 未编号的一步（{@link keyed} 补上 key）。 */
type StepDraft = Omit<TurnStep, 'key'>;

/** 给段序补上渲染键：本模块按段序编号，模板不自己数。 */
function keyed(steps: StepDraft[], turnKey: string): TurnStep[] {
  return steps.map((s, i) => ({ ...s, key: `${turnKey}-s${i}` }));
}

/** 落地段序里的一步（决策 273）。工具那一步落库时只有「成没成」，没有相位。 */
function stepFromSegment(seg: ForemanSegment): StepDraft {
  switch (seg.kind) {
    case 'thinking':
      return { kind: 'thinking', text: seg.text, tool: null, live: false };
    case 'text':
      return { kind: 'text', text: seg.text, tool: null, live: false };
    default:
      return {
        kind: 'tool',
        text: '',
        tool: {
          name: seg.tool,
          argsSummary: seg.args_summary,
          state: seg.ok ? 'ok' : 'bad',
        },
        live: false,
      };
  }
}

/** 工具痕迹（聚合视图那一份）→ 一步。老行走这条兜底。 */
function stepFromTrace(trace: ForemanTrace): StepDraft {
  return {
    kind: 'tool',
    text: '',
    tool: { name: trace.tool, argsSummary: trace.args_summary, state: trace.ok ? 'ok' : 'bad' },
    live: false,
  };
}

/**
 * 落地轮的一步步（决策 273）。
 *
 * **段序在就走段序**（顺序是它存在的全部理由）；不在（老行——那一列落地之前写下的）时由
 * 两份聚合视图兜底：推理整段在前、工具在后。兜底能给的只有这个次序：中途说过的话在那两列
 * 里根本没有（它此前也不显示），而「先想后查」比「话在前、过程在后」更接近真实。
 */
function landedSteps(m: { segments?: ForemanSegment[] | null; thinking?: string | null;
  traces: ForemanTrace[] | null }): StepDraft[] {
  const wire = m.segments ?? [];
  if (wire.length > 0) return wire.map(stepFromSegment);
  const out: StepDraft[] = [];
  if (m.thinking?.trim()) {
    out.push({ kind: 'thinking', text: m.thinking, tool: null, live: false });
  }
  for (const t of m.traces ?? []) out.push(stepFromTrace(t));
  return out;
}

/** 在飞轮的一步（流上那一份）。 */
function stepFromLive(step: ForemanLiveStep): StepDraft {
  switch (step.kind) {
    case 'thinking':
      return { kind: 'thinking', text: step.text, tool: null, live: false };
    case 'text':
      return { kind: 'text', text: step.text, tool: null, live: false };
    default:
      return {
        kind: 'tool',
        text: '',
        tool: {
          name: step.tool,
          argsSummary: step.args_summary,
          // 相位 → 三态：正在查 / 查到了 / 没查到（落库那一份由 `ok` 给同一份读数）。
          state: step.phase === 'start' ? 'running' : step.phase === 'error' ? 'bad' : 'ok',
        },
        live: false,
      };
  }
}

/**
 * 在飞轮的段序与「正在说的那一句」（决策 273）。
 *
 * **末尾那一步正文不是步骤，是回话**：它此刻正在往外冒，收尾时会被权威回话（POST 的
 * `reply` / 台账那一行）换掉；而被一次工具调用打断时，它就落定成「中途说的话」——
 * 那正是同一份数据在两个时态下的样子，判据只在这一个函数里，模板不猜。
 */
function liveSteps(stream: ForemanStreamState): { steps: StepDraft[]; reply: string } {
  const all = stream.steps.map(stepFromLive);
  const last = all[all.length - 1];
  const reply = last && last.kind === 'text' ? last.text : '';
  const steps = reply ? all.slice(0, -1) : all;
  // 末尾那一步是**正在攒**的那一步（流还在动）：推理的摘要因此说「正在想…」。
  const tail = steps[steps.length - 1];
  if (stream.streaming && tail) steps[steps.length - 1] = { ...tail, live: true };
  return { steps, reply };
}

/** {@link buildTurns} 的四个响应式输入加一个回调——全都是平凡值，组件原样传入。 */
export interface TalkTurnsInput {
  /** 会话台账（`messages` 归落地轮、`proposals` 归提议轮）。`null` = 这台机器还没有班次。 */
  session: ForemanSession | null | undefined;
  /** 正在发的那句话（台账里还没有它的回话，故先以乐观轮显示）。`null` = 没在发。 */
  pendingText: string | null;
  /** 发送在途（流还没开时也先亮一轮，免得人以为没按上）。 */
  sending: boolean;
  /**
   * 正在**跟**一轮（决策 260）：刷新之后从服务端重新接上的那一轮，本机没有它那一趟 POST。
   *
   * 与 `sending` 并列成「此刻在等一轮回话」的两条来源——`sending` 是本机发出的，
   * 这里是**别人的**（另一台设备发的，或本机刷新前发的那一趟）。两个都为假时那一段
   * 到达的增量不属于任何一轮，产出的仍然是「流里已经有字」那一支。
   */
  following: boolean;
  /** 流式归约状态：正文 / 思考 / 现场工具 / 是否仍在流 / 断流错误。 */
  stream: ForemanStreamState;
  /**
   * 当前这条流错误**是不是配对缺失**——由上游在 `ApiError` 还在手时按后端 `kind` 判好
   * （决策 259，见上「边界 ②」）。只在 `stream.error` 在场时有意义（失败轮才读它）。
   */
  pairingNeeded: boolean;
}

/**
 * 时间线归约：台账行与提议**按时刻合并排序**（不是把提议另起一段——提议是那一轮里发生
 * 的事，先后次序本身是信息），再把三种在飞轮追加在末尾，最后返回。
 *
 * 同刻的兜底次序按 `kind`：人的话 → 提议 / 操作台 / 失败 → 值班长的话。时钟是同一台机器的，
 * 同刻基本只出现在 `ManualClock` 的用例里，但**排序必须是确定的**（否则每次渲染都可能换位）。
 *
 * 三段的时刻天然分得开——值班经理的话先落库，提议在工具调用时落库，值班长的回话最后落库。
 */
export function buildTurns(input: TalkTurnsInput): TurnView[] {
  const { session, pendingText, sending, following, stream, pairingNeeded } = input;
  const messages = session?.messages ?? [];
  // 「提问之后人又开过口」的判据（决策 265③，纯派生）：最大 mine 行 id 大于该行 id。
  const maxMineId = messages.reduce((mx, m) => (m.kind === 'mine' && m.id > mx ? m.id : mx), 0);
  const stamped: { view: TurnView; rank: number }[] = messages.map((m) => ({
    // 排序与分类**同一处判定**（决策 252）：`kind` 是后端给的，界面不再各判一遍。
    // 提问轮与回话同为值班长那一轮的产物，同刻兜底与 `fm` 同档。
    rank: m.kind === 'mine' ? 0 : m.kind === 'fm' || m.kind === 'ask' ? 2 : 1,
    view: {
      key: `m${m.id}`,
      kind: m.kind,
      content: m.content,
      at: m.created_at,
      streaming: false,
      partial: false,
      // 段序（决策 273）：落地那一行的 segments 是权威；老行由两份聚合视图兜底。
      steps: keyed(landedSteps(m), `m${m.id}`),
      briefing: m.briefing,
      needsPairing: false,
      proposal: null,
      // 提问载荷与「已答」派生（决策 265③）：载荷后端给，已答按行序纯派生。
      ask: m.ask ?? null,
      askAnswered: m.ask != null && maxMineId > m.id,
      // 值守播报（决策 209④）：与 `kind` 正交的那个布尔（决策 252③），也由后端判。
      proactive: m.proactive,
      // 归因类别（决策 235① / 238）：**用后端解析并翻好的那一份**（`attribution_label`），
      // 界面不自己从稳定标识再映射一遍——两份映射迟早给出两个词，而「四类各一个词」
      // 是同一件事。未定位时后端给 null，界面就不显示（不编一个假的类别）。
      attribution: m.attribution_label ?? null,
    },
  }));
  for (const p of session?.proposals ?? []) {
    stamped.push({
      rank: 1,
      view: {
        key: `p${p.id}`,
        kind: 'proposal',
        content: p.summary,
        at: p.created_at,
        streaming: false,
        partial: false,
        steps: [],
        briefing: null,
        needsPairing: false,
        proposal: p,
        ask: null,
        askAnswered: false,
        proactive: false,
        attribution: null,
      },
    });
  }
  stamped.sort((a, b) => (a.view.at === b.view.at ? a.rank - b.rank : a.view.at < b.view.at ? -1 : 1));
  const out: TurnView[] = stamped.map((s) => s.view);
  if (pendingText) {
    out.push({
      key: 'pending',
      kind: 'mine',
      content: pendingText,
      at: '',
      streaming: false,
      partial: false,
      steps: [],
      briefing: null,
      needsPairing: false,
      proposal: null,
      ask: null,
      askAnswered: false,
      proactive: false,
      attribution: null,
    });
  }
  if (sending || following || stream.steps.length > 0) {
    const live = liveSteps(stream);
    out.push({
      key: 'live',
      kind: 'fm',
      // 还没收到第一个增量时不摆空白：给一句"对面在动"的实情，光标说明还在流
      content: live.reply || '值班长正在查台账…',
      at: '',
      streaming: stream.streaming,
      partial: !stream.streaming && live.reply.length > 0,
      steps: keyed(live.steps, 'live'),
      briefing: null,
      needsPairing: false,
      proposal: null,
      ask: null,
      askAnswered: false,
      proactive: false,
      attribution: null,
    });
  }
  if (stream.error) {
    out.push({
      key: 'send-error',
      kind: 'failed',
      content: `发送失败：${stream.error}`,
      at: '',
      streaming: false,
      partial: false,
      steps: [],
      briefing: null,
      needsPairing: pairingNeeded,
      proposal: null,
      ask: null,
      askAnswered: false,
      proactive: false,
      attribution: null,
    });
  }
  return out;
}

/**
 * 这一轮的名牌写什么（决策 252 的判定点收口 + 决策 271）。
 *
 * **值守轮的失败账与人的那一轮失败必须分开写**（决策 271）：2026-09-24 的实测里，
 * 值守轮因 provider 断供连失 37 轮，而那些行在时间线上顶着「发送失败」——那一批里值班经理
 * 一个字节都没发出去，读起来却像是他的话发不出去。判据是后端给的两个字段
 * （`kind` = 没跑起来、`proactive` = 这一行属于值守轮自己醒来的那一轮）：
 * 界面**不解析正文前缀**（决策 252 的边界一个字不动）。
 *
 * 为什么抽成函数：模板里那段嵌套三元式没有机器门（`Talk.svelte` 的断言只有 e2e），
 * 而这张表是**文案规格**——决策 199 要求它可追溯、可钉住。函数在 `talkTurns.test.ts` 里逐档断言。
 */
export function turnName(turn: Pick<TurnView, 'kind' | 'proactive'>): string {
  switch (turn.kind) {
    case 'failed':
      return turn.proactive ? '值守 · 没跑起来' : '发送失败';
    case 'mine':
      return '值班经理';
    case 'console':
      return '操作台';
    // 认不出的也落值班长一侧（与 `message_wire` 的兜底同口径）：提案轮自带一块渲染，
    // 走不到这里。
    default:
      return turn.proactive ? '值班长 · 值守' : '值班长';
  }
}
