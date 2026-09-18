/**
 * 班次的两枚标记与「看过哪条」的本地留存（决策 220③）。
 *
 * 「回话中允许换班次」之所以敢放开，是因为那把 UI 锁本来就只是第三层（决策 220①：真正的
 * 隔离是 `appendForemanDelta` 的班次守卫与 `send()` 里的 `generation` 比对）。锁撤掉之后，
 * 「那一轮回话去哪了」由 ⋯ 班次列表里的**两枚标记**接手——
 *
 * - **「正在回话」**（琥珀档的小灯 + 文字，**不加动画位**）：本机发出且未落地，**或** SSE 里
 *   带**别的** `session_id` 的增量（`/foreman/stream` 把全部工头增量广播给所有订阅者，
 *   这个字段此前只被用来「丢掉不匹配的」）。后者是一份**纯前端**映射，不进 localStorage
 *   ——它描述的是「此刻」，刷新即空是对的。
 * - **「有新动静」**（次级必读档）：`last_active_at` 晚于本机记的「上次打开它的时候」。
 *
 * 判据全在这里（含「正在回话」压过「有新动静」的优先关系）：组件里只剩渲染
 * （与 `lib/talkStops.ts` 同一姿态）。
 */

import type { ForemanSessionMeta } from '../api/types';

/** 当前班次进地址（决策 217①：对讲台班次 → URL `?session=` + localStorage 兜底）。 */
export const TALK_SESSION_KEY = 'agentpipeline.talk_session';

/** 「哪一条我看过」的时刻表（决策 217⑤ 的键名族）。 */
export const TALK_SEEN_KEY = 'agentpipeline.talk_seen';

/** 看过时刻表：`{ [sessionId]: 那一班的 last_active_at }`。 */
export type SeenAt = Record<string, string>;

/**
 * 落库的不是「本机的当下」而是**那一班当时的 `last_active_at`**：`last_active_at` 由服务端
 * 与消息插入同事务更新（`storage/foreman.rs:335`），故两边比对的是**同一座钟**——拿本机的
 * `Date.now()` 去比服务端的时间戳，机器一慢一快就会读出假标记。
 */
function localStore(): Storage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    // 隐私模式 / 被策略禁用时读 `localStorage` 属性本身就会抛
    return null;
  }
}

/** 读一份 JSON 记录；库里的值坏了（被别的版本写过、手改过）一律当空——不把坏值当数据。 */
function readRecord<T extends object>(key: string): T | null {
  const store = localStore();
  if (!store) return null;
  const raw = store.getItem(key);
  if (!raw) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return null;
    return parsed as T;
  } catch {
    return null;
  }
}

function writeRecord(key: string, value: object | null): void {
  const store = localStore();
  if (!store) return;
  try {
    if (value === null) store.removeItem(key);
    else store.setItem(key, JSON.stringify(value));
  } catch {
    // 存储写不进去（配额 / 隐私模式）不影响这一屏的用法，只是下次不再记得
  }
}

/** 读过时刻表（没有就是空表）。 */
export function loadSeen(): SeenAt {
  return readRecord<SeenAt>(TALK_SEEN_KEY) ?? {};
}

/** 写回时刻表。 */
export function saveSeen(seen: SeenAt): void {
  writeRecord(TALK_SEEN_KEY, seen);
}

/** 兜底用的当前班次 id。 */
export function loadSessionId(): string | null {
  const store = localStore();
  if (!store) return null;
  const raw = store.getItem(TALK_SESSION_KEY);
  return raw && raw.length > 0 ? raw : null;
}

/** 记下当前班次（`null` = 一个班次都没有，顺手删键）。 */
export function saveSessionId(id: string | null): void {
  const store = localStore();
  if (!store) return;
  try {
    if (id) store.setItem(TALK_SESSION_KEY, id);
    else store.removeItem(TALK_SESSION_KEY);
  } catch {
    // 同上：写不进去不影响这一屏
  }
}

/**
 * 本机**第一次**看到这份班次列表时的基线：把当时的状态当作「都看过了」。
 *
 * 不做这一步的话，第一屏会把每一条都标成「有新动静」——而它们只是刚被列出来。
 */
export function seedSeen(list: readonly ForemanSessionMeta[]): SeenAt {
  const seen: SeenAt = {};
  for (const s of list) seen[s.id] = s.last_active_at;
  return seen;
}

/**
 * 只在**本机一条记录都没有**时立基线（决策 220③ 的第一屏）。
 *
 * 判据是「这份时刻表是不是空的」，**不是**「这一屏是不是刚装的」：本仓的实现在这里踩过一次
 * ——按「每次装载都拿当下的状态立基线」写，等于把**关机期间别处发生的动静**一并记成「看过
 * 了」，而那正是这枚标记最该说话的场合（手机在别的设备上开了新班次、说了话，回到这台电脑
 * 打开对讲台，菜单里那条**该亮着**）。空表才立基线：装过、用过的机器上，表里没有的班次就是
 * 「本机没见过」，照 ③ 的规则算「有新动静」。
 */
export function seedBaselineIfFirstRun(
  seen: SeenAt,
  list: readonly ForemanSessionMeta[],
): SeenAt {
  return Object.keys(seen).length === 0 ? seedSeen(list) : seen;
}

/** 把某一班记成看过了（打开它、或每次读到新台账都算「看了」这一步）。 */
export function markSeen(seen: SeenAt, sessionId: string, lastActiveAt: string): SeenAt {
  if (seen[sessionId] === lastActiveAt) return seen;
  return { ...seen, [sessionId]: lastActiveAt };
}

/** 服务端已经不存在的班次从表里清掉（决策 217⑤ 的清理纪律，与 `?session=` 的删键同一手）。 */
export function pruneSeen(seen: SeenAt, live: readonly string[]): SeenAt {
  const keep = new Set(live);
  const out: SeenAt = {};
  let dropped = false;
  for (const [id, at] of Object.entries(seen)) {
    if (keep.has(id)) out[id] = at;
    else dropped = true;
  }
  return dropped ? out : seen;
}

/** 这一班「有新动静」吗：它的 `last_active_at` 晚于本机记的那一个。 */
export function isFresh(meta: ForemanSessionMeta, seen: SeenAt): boolean {
  const at = seen[meta.id];
  if (!at) return true; // 本机没见过它 —— 它在别处动过
  // 时间戳坏掉时 `Date.parse` 给 `NaN`，而任何与 `NaN` 的比较都是假 —— 不亮标记而不是
  // 抛出去：读不懂它，就别说它「有新动静」
  return Date.parse(meta.last_active_at) > Date.parse(at);
}

/** 一条班次最多挂一枚标记；没有就是 `null`。 */
export type SessionMark = 'replying' | 'fresh' | null;

/**
 * 两枚标记的合成判据（决策 220③）——**优先关系也在这里**，组件只负责把结果画出来
 * （与 `lib/talkStops.ts` 同一姿态：判据不在模板里，模板里就一条 `{#if}`）。
 *
 * - **「正在回话」优先**：它是「此刻」，另一条是「累计」——同时成立时只说得出一件事，
 *   而「它这一秒在说话」比「它上次说的话你还没看」更该被说出来；
 * - **当前打开的那一条永远不带「有新动静」**：打开即清零，同一帧内不该闪一下；
 * - 当前打开的那一条**可以**带「正在回话」——本机刚发出去还没落地时，它就在你眼前说话。
 */
export function sessionMark(
  meta: ForemanSessionMeta,
  ctx: {
    currentId: string | null;
    /** 本机刚发出去、还没落地的那一班（`sending` 期间的那个 id）。 */
    sendingSid: string | null;
    /** SSE 里带**别的** `session_id` 的增量（`realtime/foreman.ts` 的映射判据）。 */
    foreignReplying: boolean;
    seen: SeenAt;
  },
): SessionMark {
  if (meta.id === ctx.sendingSid || ctx.foreignReplying) return 'replying';
  if (meta.id === ctx.currentId) return null;
  return isFresh(meta, ctx.seen) ? 'fresh' : null;
}
