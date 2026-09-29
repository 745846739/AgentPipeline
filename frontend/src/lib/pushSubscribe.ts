/**
 * 「订阅此设备」的前端判据（spec `.scratch/pwa-webpush/` 票 03）。
 *
 * 三层，各自可测：
 * 1. **权限四态**（[`pushFace`]）——页面按它决定摆按钮还是摆引导；判据只读环境
 *    （安全上下文 / service worker / PushManager / Notification.permission / iOS 主屏），
 *    故是纯函数。
 * 2. **应用服务器公钥的字节形**（[`urlBase64ToUint8Array`]）——`applicationServerKey`
 *    要的是 `Uint8Array`（Safari 上尤其不能只给字符串），而服务端给的是 base64url。
 * 3. **权限弹窗只在点击手势里弹**（票面 12 号故事）：这条不是函数能保证的，而是
 *    `SettingsNotify.svelte` 里那颗按钮的形状——进页面不弹、点了才弹。
 */

/** iOS Safari 非主屏时的引导：iOS 的 Web Push 只在**添加到主屏幕**的 PWA 里可用。 */
export const IOS_INSTALL_HINT = '请先添加到主屏幕：点分享 → 添加到主屏幕，再从主屏图标打开这一页。';

/** 权限被拒时的指引：浏览器把这条决定交给了系统设置。 */
export const DENIED_HINT =
  '通知权限已被拒绝。到系统设置的「浏览器 / 通知」里把本站在允许列表里放开，再回来点一次。';

/** 浏览器根本不支持推送时的说明（含「非安全上下文」这一种：http 的局域网地址）。 */
export const UNSUPPORTED_HINT =
  '这个浏览器 / 这个地址不支持浏览器推送：推送只在 HTTPS（或本机 localhost）下可用。';

export type PushFaceKind =
  /** iOS 且不在主屏：摆「添加到主屏幕」的引导。 */
  | 'ios-needs-install'
  /** 环境不支持（老浏览器 / 非安全上下文）。 */
  | 'unsupported'
  /** 权限被拒：摆指引，不摆按钮（点了也不会弹）。 */
  | 'denied'
  /** 已授权：可以订阅（还没订）或已订阅（可以退订）。 */
  | 'granted'
  /** 还没问过：摆可点的按钮。 */
  | 'prompt';

export interface PushFace {
  kind: PushFaceKind;
  /** 这一态要不要摆「订阅此设备」那颗钮（只有 prompt / granted 摆）。 */
  canSubscribe: boolean;
}

/**
 * 环境读数的形状（页面传 `window` 侧的那几个字段进来；测试传构造值）。
 *
 * `permission` 用 `NotificationPermission | 'unsupported'`：浏览器没有 `Notification`
 * 时页面给 `'unsupported'`，免得这个纯函数去碰全局。
 */
export interface PushEnv {
  isSecureContext: boolean;
  hasServiceWorker: boolean;
  hasPushManager: boolean;
  permission: 'default' | 'granted' | 'denied' | 'unsupported';
  isIos: boolean;
  /** iOS 主屏 PWA（`navigator.standalone === true`）；非 iOS 上恒 false。 */
  standalone: boolean;
}

/** 判据顺序：**先环境、后权限**——环境不满足时权限是什么都没有意义。 */
export function pushFace(env: PushEnv): PushFace {
  if (env.isIos && !env.standalone) return { kind: 'ios-needs-install', canSubscribe: false };
  if (!env.isSecureContext || !env.hasServiceWorker || !env.hasPushManager || env.permission === 'unsupported') {
    return { kind: 'unsupported', canSubscribe: false };
  }
  if (env.permission === 'denied') return { kind: 'denied', canSubscribe: false };
  if (env.permission === 'granted') return { kind: 'granted', canSubscribe: true };
  return { kind: 'prompt', canSubscribe: true };
}

/** 这一态在页头那枚标牌上的短名。 */
export function pushFaceLabel(face: PushFaceKind): string {
  switch (face) {
    case 'ios-needs-install':
      return '需要添加到主屏幕';
    case 'unsupported':
      return '环境不支持';
    case 'denied':
      return '权限被拒绝';
    case 'granted':
      return '已授权';
    case 'prompt':
      return '未申请权限';
  }
}

/** 这一态的说明文案（页面只做展示，措辞住在这里以便被断言）。 */
export function pushFaceHint(face: PushFaceKind): string {
  switch (face) {
    case 'ios-needs-install':
      return IOS_INSTALL_HINT;
    case 'unsupported':
      return UNSUPPORTED_HINT;
    case 'denied':
      return DENIED_HINT;
    case 'granted':
      return '已授权：这台设备可以订阅；订阅之后就能在锁屏上收到流水线的通知。';
    case 'prompt':
      return '点下面的钮完成订阅——权限弹窗只会在你点它的那一刻出现。';
  }
}

/**
 * 读当前环境（页面用；`navigator` 缺失时按「不支持」处理——服务端渲染 / 测试环境）。
 *
 * iOS 的判据照 iOS 13+ 的 iPadOS：`MacIntel` + 触摸点也算 iPad（它的 UA 装成 Mac）。
 */
export function readPushEnv(
  nav: Pick<Navigator, 'userAgent' | 'maxTouchPoints'> & { standalone?: boolean } = navigator,
  notification: Pick<typeof Notification, 'permission'> | undefined = typeof Notification !== 'undefined'
    ? Notification
    : undefined,
): PushEnv {
  const ua = nav.userAgent ?? '';
  const isIos =
    /iPad|iPhone|iPod/.test(ua) ||
    (ua.includes('Macintosh') && (nav.maxTouchPoints ?? 0) > 1);
  return {
    isSecureContext: typeof window !== 'undefined' ? window.isSecureContext : false,
    hasServiceWorker: typeof navigator !== 'undefined' && 'serviceWorker' in navigator,
    hasPushManager: typeof window !== 'undefined' && 'PushManager' in window,
    permission: notification?.permission ?? 'unsupported',
    isIos,
    standalone: Boolean((nav as { standalone?: boolean }).standalone),
  };
}

/**
 * base64url（无填充）→ `Uint8Array`：`applicationServerKey` 的那一件。
 *
 * 服务端回显的是 base64url（RFC 4648 §5：`-`/`_` 代替 `+`/`/`、无 `=` 填充），
 * 而 `atob` 只认标准 base64 **且要求长度是 4 的倍数**——故先补齐再换字符表。
 * 传错形状（非法字符 / 空）时返回 `null`：调用方据此说话，而不是拿一串垃圾去订阅。
 */
export function urlBase64ToUint8Array(base64Url: string): Uint8Array<ArrayBuffer> | null {
  const trimmed = base64Url.trim();
  if (!trimmed) return null;
  const standard = trimmed.replace(/-/g, '+').replace(/_/g, '/');
  const padded = standard + '='.repeat((4 - (standard.length % 4)) % 4);
  let binary: string;
  try {
    binary = atob(padded);
  } catch {
    return null;
  }
  // 显式开一块 `ArrayBuffer` 而不是 `new Uint8Array(len)`：后者的 buffer 类型是
  // `ArrayBufferLike`（含 SharedArrayBuffer），而 `applicationServerKey` 要的是
  // `BufferSource`（`ArrayBufferView<ArrayBuffer>`）——TS 的 lib 类型在这一处不让步。
  const bytes = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/** 本机那条订阅的行 id 存在哪（退订时要按 id 删服务端那一行）。 */
export const LOCAL_SUBSCRIPTION_KEY = 'agentpipeline.push.subscriptionId';

/**
 * 本机那条订阅的行 id（票 03 的「可退订」）：订阅成功时把服务端回的 id 记下来，
 * 退订时按它删服务端那一行。
 *
 * 为什么不是「拿本机 endpoint 去清单里找」：清单只给 endpoint 的**摘要**（那是刻意的
 * ——完整 endpoint 是能力 URL）。三条出路里这是最诚实的一条：记 id（读不到就退化成
 * 「本地退订 + 服务端下次收到 410 时自清」），而不是把能力 URL 再发给前端一遍。
 *
 * 存储面照 `api/config.ts` 的 localStorage 纪律：读不到 / 写不进（隐私模式）时**不抛**，
 * 代价只是退订要等下一次 410（功能照旧可用）。
 */
export type LocalStore = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

function store(fallback?: LocalStore): LocalStore | null {
  if (fallback) return fallback;
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

export function rememberLocalSubscriptionId(id: number, fallback?: LocalStore): void {
  try {
    store(fallback)?.setItem(LOCAL_SUBSCRIPTION_KEY, String(id));
  } catch {
    // 写不进去就算了（退订仍能本地生效）。
  }
}

export function readLocalSubscriptionId(fallback?: LocalStore): number | null {
  try {
    const raw = store(fallback)?.getItem(LOCAL_SUBSCRIPTION_KEY);
    if (!raw) return null;
    const id = Number.parseInt(raw, 10);
    return Number.isFinite(id) ? id : null;
  } catch {
    return null;
  }
}

export function forgetLocalSubscriptionId(fallback?: LocalStore): void {
  try {
    store(fallback)?.removeItem(LOCAL_SUBSCRIPTION_KEY);
  } catch {
    // 同上。
  }
}
