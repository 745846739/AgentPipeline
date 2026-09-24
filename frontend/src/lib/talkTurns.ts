import type {
  ForemanAsk,
  ForemanBriefing,
  ForemanProposal,
  ForemanSession,
  ForemanTrace,
} from '../api/types';
import type { ForemanLiveTool, ForemanStreamState } from '../realtime/foreman';

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
  content: string;
  /** 排进时间线的时刻（RFC3339）。三种在途轮（乐观轮 / 流式轮 / 失败轮）没有它，恒在末尾。 */
  at: string;
  /** 流式尾随方块光标（既有 `.streaming`，不新增动画位）。 */
  streaming: boolean;
  /** 断流/出错：这一轮只有已收到的部分。 */
  partial: boolean;
  /** 该轮工具痕迹（台账查读），空数组 = 这一轮没翻台账。 */
  traces: ForemanTrace[];
  /**
   * 这一轮的**推理 / 思考**原文（决策 244）。`null` = 这一轮没产推理（多数模型如此）。
   *
   * 两种来源：在途轮来自实时流（`stream.thinking`），落地轮来自台账那一行的
   * `thinking` 列——**同一份内容的两个时态**，界面只渲染它，不关心哪来的。
   */
  thinking: string | null;
  /**
   * 这一轮**正在发生**的工具调用（决策 244，只在途轮非空）。
   *
   * 与 `traces`（落库那一份）分工：`traces` 说「这一轮查过什么」（轮次结束后才有），
   * 这里说「此刻在查什么」。落地之后这一栏就空了——那时 `traces` 已经把同一件事说完。
   */
  liveTools: ForemanLiveTool[];
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
      traces: m.traces ?? [],
      // 推理留痕（决策 244）：空串与 null 都当作「这一轮没产推理」，界面不渲染那一块。
      thinking: m.thinking?.trim() ? m.thinking : null,
      liveTools: [],
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
        traces: [],
        thinking: null,
        liveTools: [],
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
      traces: [],
      thinking: null,
      liveTools: [],
      briefing: null,
      needsPairing: false,
      proposal: null,
      ask: null,
      askAnswered: false,
      proactive: false,
      attribution: null,
    });
  }
  if (sending || following || stream.text) {
    out.push({
      key: 'live',
      kind: 'fm',
      // 还没收到第一个增量时不摆空白：给一句"对面在动"的实情，光标说明还在流
      content: stream.text || '值班长正在查台账…',
      at: '',
      streaming: stream.streaming,
      partial: !stream.streaming && stream.text.length > 0,
      traces: [],
      thinking: stream.thinking.trim() ? stream.thinking : null,
      liveTools: stream.tools,
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
      traces: [],
      thinking: null,
      liveTools: [],
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
