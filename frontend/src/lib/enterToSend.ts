/**
 * 「回车即提交」的输入法护栏（决策 184）。
 *
 * **问题是次序与时机，不只是标志位。** 中文 / 日文输入法用**回车确认候选词**（在中文
 * 输入法里敲英文也是候选词），而这一次回车同时是一次 `keydown`：
 *
 * | 浏览器 | 确认候选词那一次的次序 | 只查 `event.isComposing` 的结果 |
 * |---|---|---|
 * | Chromium / Firefox | `keydown`（`isComposing = true`）→ `compositionend` | 挡得住 |
 * | WebKit（Safari / **桌面壳的 WKWebView**） | `compositionend` → **`keydown`（`isComposing = false`）** | **挡不住：选字即发送** |
 *
 * 桌面包就是 WKWebView（决策 153），所以「输入法里敲英文再回车直接发出去」是必现的。
 * 故护栏三重，任何一重单独都不够：
 * 1. `event.isComposing`（Chromium 路线）；
 * 2. `keyCode === 229`（「这次按键属于输入法」的通用旧信号，某些 IME 只给这一条）；
 * 3. **组合结束后的一小段时间窗**（WebKit 路线）：`compositionend` 之后的
 *    [`COMPOSITION_SETTLE_MS`] 之内，回车一律按「选字」处理。
 *
 * 第 3 重是时间窗而不是「吃掉下一个回车」：后者的代价是人**先按回车选字、再按一次回车
 * 发送**时那次发送被吞掉（他得按第三次）。窗口取 50ms——WebKit 那两次事件相隔在一个
 * 任务量级（亚毫秒），而人手连按两次回车最快也在 100ms 上下，两者不会混淆。
 * 这也是 CodeMirror 一类编辑器的既有做法（它们在 `compositionend` 后留一个短窗口）。
 *
 * 判据住在这里（纯函数 + 一个不碰 DOM、时钟可注入的小状态机）而不是组件里，是为了能直接
 * 钉住时序：组件只负责把 `compositionstart` / `compositionend` / `keydown` 三件事喂进来。
 */

/** 组合结束后仍然按「选字」处理的时间窗（毫秒）。理由见模块头。 */
export const COMPOSITION_SETTLE_MS = 50;

/** `keydown` 里本模块用得到的字段（结构化类型：测试不必造真的 `KeyboardEvent`）。 */
export interface EnterKeyEvent {
  key: string;
  shiftKey: boolean;
  /** 浏览器原生标志。WebKit 在确认候选词那一次**不正确**（见模块头）。 */
  isComposing?: boolean;
  /** 229 = 「这次按键属于输入法」。已废弃但所有引擎都还在发，是第二条独立信号。 */
  keyCode?: number;
}

/**
 * 这一次 `keydown` 是不是「组合中的一部分」——是则**不由我们处理**。
 *
 * `keyCode === 229` 与 `isComposing` 是两条独立信号：某些 IME（尤其 Windows 上的
 * 拼音）只给其中之一。
 */
export function isCompositionKeydown(event: EnterKeyEvent): boolean {
  return event.isComposing === true || event.keyCode === 229;
}

/**
 * 这一次 `keydown` 该不该当成「提交」。
 *
 * 需要同时满足：是回车、没按 Shift、不在组合中、不属于输入法的那一次。
 * 调用方拿到 `true` 后自行 `preventDefault()` 并提交——本函数**不改事件**（纯判据）。
 */
export function shouldSubmitOnEnter(event: EnterKeyEvent, composing: boolean): boolean {
  if (event.key !== 'Enter' || event.shiftKey) return false;
  if (composing || isCompositionKeydown(event)) return false;
  return true;
}

/** 取当前毫秒时间戳（测试注入假时钟）。 */
export type NowFn = () => number;

/**
 * 输入法组合态的小状态机（不碰 DOM、时钟可注入，故可直接单测）。
 *
 * [`CompositionGuard::active`] 覆盖三种「现在不该提交」的情形：
 * 正在组合；刚 `compositionend`（同一个任务里 WebKit 的 `keydown` 还在路上）；
 * 在 [`COMPOSITION_SETTLE_MS`] 窗口内（事件跨了任务也挡得住）。
 */
export class CompositionGuard {
  private composing = false;
  private endedAt: number | null = null;

  constructor(private readonly now: NowFn = () => Date.now()) {}

  /** `compositionstart`：进入组合态。 */
  start(): void {
    this.composing = true;
    this.endedAt = null;
  }

  /** `compositionend`：退出组合态，但从这一刻起开一个短窗口（理由见模块头）。 */
  end(): void {
    this.composing = false;
    this.endedAt = this.now();
  }

  /** 现在是否应当把回车当成「选字」而不是「提交」。 */
  active(): boolean {
    if (this.composing) return true;
    if (this.endedAt === null) return false;
    return this.now() - this.endedAt < COMPOSITION_SETTLE_MS;
  }
}
