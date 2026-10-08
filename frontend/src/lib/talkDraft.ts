/**
 * 对讲台输入草稿的 localStorage 形状（决策 217①⑤）。
 *
 * 草稿**只进本地、不进地址**：半句话塞进 `?draft=`，分享出去的是一个别人看不懂的 URL，
 * 而且打字时地址栏会被反复改写（决策 217① 的原话）。这一格是「我没写完的那句话」，
 * 不是「我在哪一格」，所以它连参数都不配有一个。
 *
 * 键 `agentpipeline.talk_draft`，值是 JSON `{sessionId, text, at}`：
 * - `sessionId`：草稿属于哪一班——切班次不搬别人的稿，也不删别人的稿（单输入框，同一时刻
 *   只有一个「正在写」的格子，后来者覆盖前者）；
 * - `at`：最后一次写下的时刻。装载时早于 7 天整条清掉（决策 217⑤「不让它无限增长」）。
 *
 * 形状不对 / JSON 坏掉一律**就地删键并回 `null`**（决策 217④「取值非法回落缺省并顺手删键」）
 * ——脏键留在那里，下一回装载只会再读一次同样的脏东西。
 *
 * 存储不可用（隐私模式、配额满）一律静默：丢一条草稿好过把打字这条路走成抛错。
 */

/** 决策 217⑤ 写死的键名。 */
export const TALK_DRAFT_KEY = 'agentpipeline.talk_draft';

/** 决策 217⑤：草稿最长活 7 天。 */
export const TALK_DRAFT_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

export interface TalkDraft {
  /** 这半句话是在哪一班打的（对讲台的班次 id）。 */
  sessionId: string;
  /** 原文——含中间态的空格与换行，回填进输入框要一字不差。 */
  text: string;
  /** 最后写下的时刻（epoch ms）。 */
  at: number;
}

/**
 * 读草稿。
 *
 * - 键不在 → `null`（**不删任何东西**：没有与「坏」是两回事）；
 * - JSON 坏 / 形状不对 / 超过 7 天 → 删键并回 `null`；
 * - `now` 可注入：过期判据要能被单测钉住（默认 `Date.now()`）。
 */
export function loadTalkDraft(now: number = Date.now()): TalkDraft | null {
  let raw: string | null;
  try {
    raw = localStorage.getItem(TALK_DRAFT_KEY);
  } catch {
    return null;
  }
  if (raw === null) return null;

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw) as unknown;
  } catch {
    clearTalkDraft();
    return null;
  }

  if (parsed === null || typeof parsed !== 'object') {
    clearTalkDraft();
    return null;
  }
  const d = parsed as Partial<TalkDraft>;
  if (
    typeof d.sessionId !== 'string' ||
    d.sessionId === '' ||
    typeof d.text !== 'string' ||
    typeof d.at !== 'number' ||
    !Number.isFinite(d.at)
  ) {
    clearTalkDraft();
    return null;
  }
  if (now - d.at > TALK_DRAFT_MAX_AGE_MS) {
    clearTalkDraft();
    return null;
  }
  return { sessionId: d.sessionId, text: d.text, at: d.at };
}

/** 写草稿（覆盖同键）。 */
export function writeTalkDraft(draft: TalkDraft): void {
  try {
    localStorage.setItem(TALK_DRAFT_KEY, JSON.stringify(draft));
  } catch {
    /* 存储不可用：丢草稿好过抛错 */
  }
}

/** 删草稿（发送成功后走这一条，决策 217⑤「立刻清零」）。 */
export function clearTalkDraft(): void {
  try {
    localStorage.removeItem(TALK_DRAFT_KEY);
  } catch {
    /* 同上 */
  }
}

/**
 * 读「这一班」的草稿正文（归属比对留在这里——页面只问「这班有没有稿」）。
 *
 * 页面里不许再出现手写的 `sessionId` 比对：那是票 02 退场的「在途比对」拼法，
 * `delegation-scan.test.ts` 有一条正则扫着 `routes/Talk.svelte`，比对长在 lib 里，
 * 页面上只剩「取这一班的稿」这一件事。
 *
 * 别的班那半句既不搬也不删（等切回去再说——单输入框，不覆盖别人的稿）。
 */
export function loadTalkDraftText(sessionId: string, now: number = Date.now()): string | null {
  const d = loadTalkDraft(now);
  return d !== null && d.sessionId === sessionId ? d.text : null;
}
