/**
 * 浏览器推送的报文与落点（spec `.scratch/pwa-webpush/` 票 03）——service worker 的
 * 两个处理器对半切开后那一层**纯判据**。
 *
 * 为什么单独住一个文件：service worker 跑在另一条线程上、没有页面 store、也没法
 * 在 vitest 里当页面组件挂起来；而「payload → 通知形状」与「payload → 导航目标」
 * 这两件恰恰是最容易写错、也最该被钉住的两件（票面点名要纯函数 + vitest 覆盖）。
 * `src/sw.ts` 只是把这两个函数接到浏览器事件上。
 */

/** 通知里那份 payload（服务端 `NotifyFormat::WebPush` 拼的那三个字段）。 */
export interface PushNotice {
  title: string;
  body: string;
  /** 深链：服务端拼好的 hash 路由（`#/task/t1` 这类），见 core 的 `attention_deep_link`。 */
  url: string;
}

/** 看板首页——深链不可用时**唯一的**降级落点（票面：降级到看板首页而不是白屏）。 */
export const BOARD_HOME_HASH = '#/';

/** 没有标题时的兜底（`showNotification` 的 title 不能为空串，否则浏览器直接拒收）。 */
const FALLBACK_TITLE = '[AgentPipeline]';

/**
 * 收下 push 事件的数据，归一成一条通知。
 *
 * `raw` 是**已经解析过**的载荷（对象 / 字符串 / 别的什么），解析本身留在调用方
 * ——service worker 那边是 `event.data.json()`（可能抛），这样这个函数就纯了。
 *
 * 形状不对时**不丢**：推送到了却不显示通知，浏览器会把它当成「这条推送没人处理」
 * 而报错（`userVisibleOnly` 的约束）。所以缺什么补什么，最差也给一句兜底文案。
 */
export function notificationFrom(raw: unknown): PushNotice {
  if (typeof raw === 'string') {
    const text = raw.trim();
    if (!text) return { title: FALLBACK_TITLE, body: '', url: BOARD_HOME_HASH };
    try {
      return notificationFrom(JSON.parse(text));
    } catch {
      // 非 JSON 的纯文本：正文照给（服务端不发这种，但谁知道将来是谁在推）。
      return { title: FALLBACK_TITLE, body: text, url: BOARD_HOME_HASH };
    }
  }
  if (raw && typeof raw === 'object') {
    const obj = raw as Record<string, unknown>;
    const title = typeof obj.title === 'string' ? obj.title.trim() : '';
    const body = typeof obj.body === 'string' ? obj.body : '';
    const url = typeof obj.url === 'string' ? obj.url : '';
    return { title: title || FALLBACK_TITLE, body, url };
  }
  return { title: FALLBACK_TITLE, body: '', url: BOARD_HOME_HASH };
}

/**
 * 点通知去哪儿（票面「不可达时降级看板首页」）。
 *
 * **只认自己那几条 hash 路由**：`#/…` 开头的相对深链。这是刻意的收窄——payload 由
 * 服务端拼、经推送服务转发，把它当成可信输入直接 `openWindow` 就是给了「让通知把
 * 用户带去任意站点」的能力。故：绝对地址（含 `//` 或 `scheme:`）、空值、别的形状
 * 一律降级到看板首页。
 *
 * `origin` 由调用方给（service worker 里是 `self.location.origin`），故这个函数是纯的。
 */
export function notificationTarget(url: string | null | undefined, origin: string): string {
  const home = `${origin}/#/`;
  if (typeof url !== 'string') return home;
  const value = url.trim();
  if (!value.startsWith('#/')) return home;
  // `#//host` 这种「hash 里再塞一条**协议相对**地址」的形状拒掉：本应用的路由不认它，
  // 而它在别的解释器里可能被当成另一个 origin 的起点。降级到看板首页是安全的答案。
  if (value.startsWith('#//')) return home;
  return `${origin}/${value}`;
}
