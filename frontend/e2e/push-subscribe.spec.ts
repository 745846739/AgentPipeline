/**
 * 前端 E2E：浏览器推送的订阅链（spec `.scratch/pwa-webpush/` 票 03/04；决策 323）。
 *
 * 只测**外部行为**，口径与其它用例一致：真应用（内嵌产物 + 真后端）、真浏览器、按用户
 * 动作断言。这一票在 e2e 层要拿的三件事，恰好都是单测够不着的：
 *
 * ① **`/sw.js` 真的是能跑起来的经典脚本**。`sw.js` 与页面产物同出一炉，而 service worker
 *    的注册**不带** `{type:'module'}`——那是刻意的：iOS Safari 至今不支持 module worker，
 *    而 iPhone 正是这一票的目标设备之一。故「产物里没有 ESM 语法」是运行期才暴露的性质，
 *    这里用 `navigator.serviceWorker.ready` 兑现（Vite 若把共享代码提成公共 chunk，sw.js
 *    会带上 `import`，注册当场失败）。作用域断言在根（`/`），推送点开的深链才落得进页面。
 * ② **上报真的到了服务端**：订阅成功后独立读一次 `GET /notify/push/subscriptions`
 *    ——只看界面的话，「清单里有一行」与「服务端真的存了」是两件事。同时钉住完整 endpoint
 *    **不出现在任何一处**（清单只给摘要，那是能力 URL 的一半）。
 * ③ **权限被拒 / iOS 非主屏这两态摆的是引导而不是钮**（票面 12 号故事的四态之二）。
 *    这两态在真浏览器里都要改环境才造得出（系统权限是浏览器外面的事、iOS 是 UA），
 *    故用 `addInitScript` 注入——注入的是**浏览器那一侧的环境**，页面代码一行不改。
 *
 * **两处环境注入**（`browserPushStub`）都不是省事的抄近路，各自有取证：
 *
 * - **推送服务**：headless Chromium 连不上任何推送服务，`pushManager.subscribe()` 必然抛。
 *   替身只换这一环——service worker 注册、上报服务端、清单读写全真跑（与 harness 只换
 *   LLM 流同一替换边界）。
 * - **通知权限**：`context.grantPermissions(['notifications'])` 在这里**不够**——同一刻
 *   实测 `navigator.permissions.query({name:'notifications'})` 已报 `granted`，而
 *   `Notification.permission` 仍报 `denied`（平台那一层没有通知服务，Chromium 的
 *   `Notification.permission` 看的是它，不是权限库）。而这一页的判据读的正是后者，故
 *   权限也在注入里给。真机上的权限链由票 04 的人工验收兜（那是浏览器外面的事）。
 */

import { expect, test, type Page } from '@playwright/test';
import { expectBundleHealthy, settleBundle, startApp, watchBundle, type App } from './harness';
import { foremanScript } from './scripts';

/** 那台「设备」的推送地址（摘要与完整值都从这里推出来，断言用同一份）。 */
const ENDPOINT = 'https://push.example.net/e2e-device-0001';
/** 摘要：取路径最后一段（≤16 字符时原样加前缀 `…`），见 `storage/push.rs::endpoint_hint`。 */
const ENDPOINT_HINT = '…e2e-device-0001';

/**
 * 浏览器那一侧环境的替身（`addInitScript` 注入页面上下文，理由见文件头）。
 *
 * `getSubscription` 先回 `null`、`subscribe` 之后才回那条活订阅——**顺序**是这一票的
 * 关键：页面据此决定摆「订阅此设备」还是「退订此设备」，而第一次进页面时本机什么都没有。
 * `unsubscribe` 把状态置回 `null`，好让「撤销那一行之后订阅钮回来」也验得到。
 */
function browserPushStub(permission: 'granted' | 'denied'): void {
  const endpoint = 'https://push.example.net/e2e-device-0001';
  // p256dh / auth 用 RFC 8291 §5 的两件真值（`webpush.rs` 的 KAT 用的是同一对）——
  // 服务端在落库前校验形状（65 字节未压缩点 + 16 字节 auth），随手编的字符串会被 400 挡掉。
  const keys = {
    p256dh: 'BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4',
    auth: 'BTBZMqHH6r4Tts7J_aSIgg',
  };
  let live: unknown = null;
  const make = () => ({
    endpoint,
    expirationTime: null,
    options: { userVisibleOnly: true },
    getKey: () => new Uint8Array([0x04, 0x01, 0x02, 0x03]),
    toJSON: () => ({ endpoint, keys }),
    unsubscribe: async () => {
      live = null;
      return true;
    },
  });
  Object.defineProperty(PushManager.prototype, 'subscribe', {
    configurable: true,
    writable: true,
    value: async () => (live = make()),
  });
  Object.defineProperty(PushManager.prototype, 'getSubscription', {
    configurable: true,
    writable: true,
    value: async () => live,
  });
  Object.defineProperty(Notification, 'permission', {
    configurable: true,
    get: () => permission,
  });
  Object.defineProperty(Notification, 'requestPermission', {
    configurable: true,
    writable: true,
    value: async () => permission,
  });
}

/** 选第四通道并保存（保存那一刻服务端把 VAPID 密钥对生成出来，订阅钮才可点）。 */
async function enablePushChannel(page: Page): Promise<void> {
  await page.getByRole('radio', { name: '浏览器推送' }).click();
  await page.getByRole('button', { name: '保存通道' }).click();
  await expect(page.getByRole('status')).toContainText('VAPID 密钥对已在服务端生成');
}

/** 服务端清单的原样读数（独立取证：界面之外的那一份）。 */
async function serverList(page: Page, app: App): Promise<Record<string, unknown>[]> {
  const res = await page.request.get(`${app.apiBase}/notify/push/subscriptions`);
  expect(res.ok()).toBe(true);
  return ((await res.json()) as { subscriptions: Record<string, unknown>[] }).subscriptions;
}

test.describe('前端 E2E：浏览器推送的订阅（票 03）', () => {
  let app: App;

  test.beforeAll(async () => {
    // providerOnly：设置页不依赖项目与任务，且不必跑流水线
    app = await startApp({ script: foremanScript([[]]), providerOnly: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('service worker 注册并激活在根作用域（/sw.js 是可执行的经典脚本）', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/notify`);
    await settleBundle(page, bundle);

    const reg = await page.evaluate(async () => {
      const registration = await navigator.serviceWorker.ready;
      return { url: registration.active?.scriptURL ?? '', scope: registration.scope };
    });
    expect(reg.url).toContain('/sw.js');
    // 作用域是根：推送深链（`#/task/…`）落在任何一条路由上都在它的领地内
    expect(new URL(reg.scope).pathname).toBe('/');

    expectBundleHealthy(bundle);
  });

  test('订阅此设备：权限 → 订阅 → 上报；服务端真的多一行，撤销后那行没了', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.addInitScript(browserPushStub, 'granted');
    await page.goto(`${app.webBase}/#/settings/notify`);
    await settleBundle(page, bundle);

    // 起点：通道还没配、也没有设备订阅
    expect(await serverList(page, app)).toHaveLength(0);
    await expect(page.getByText(/还没有设备订阅/)).toHaveCount(0);

    await enablePushChannel(page);

    // 「订阅」一节摆出来了（通道 = 浏览器推送才摆），本机未订阅 → 摆的是订阅钮
    const subscribe = page.getByRole('button', { name: '订阅此设备' });
    await expect(subscribe).toBeEnabled();

    await subscribe.click();
    await expect(page.getByRole('status')).toContainText('这台设备已订阅');

    // 清单里多了一行：只给摘要，完整 endpoint 一处都不出现
    await expect(page.getByText(ENDPOINT_HINT)).toBeVisible();
    await expect(page.getByText(ENDPOINT)).toHaveCount(0);

    // 服务端真的存了那一行（独立读一次，不经界面）
    const saved = await serverList(page, app);
    expect(saved).toHaveLength(1);
    expect(saved[0].endpoint_hint).toBe(ENDPOINT_HINT);
    expect(saved[0].user_agent).toBeTruthy();
    expect(JSON.stringify(saved)).not.toContain('https://push.example.net');

    // 撤销 → 服务端那一行没了，订阅钮回来（本机那份订阅也一并退掉）
    await page.getByRole('button', { name: '撤销' }).click();
    await expect(page.getByRole('status')).toContainText('那一台设备已撤销');
    expect(await serverList(page, app)).toHaveLength(0);
    await expect(page.getByRole('button', { name: '订阅此设备' })).toBeVisible();
    await expect(page.getByText(/还没有设备订阅/)).toBeVisible();

    expectBundleHealthy(bundle);
  });

  test('权限被拒：不摆订阅钮，摆去系统设置的指引（点了也不会弹）', async ({ page }) => {
    const bundle = watchBundle(page);
    // 浏览器那一侧的环境：权限已是 denied（真实世界里由用户在地址栏里做过这个决定）
    await page.addInitScript(browserPushStub, 'denied');
    await page.goto(`${app.webBase}/#/settings/notify`);
    await settleBundle(page, bundle);

    await enablePushChannel(page);

    await expect(page.getByText(/通知权限已被拒绝/)).toBeVisible();
    await expect(page.getByRole('button', { name: '订阅此设备' })).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test.describe('iPhone（Safari）非主屏', () => {
    test.use({
      userAgent:
        'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1',
    });

    test('摆「添加到主屏幕」的引导而不是订阅钮（iOS 的 Web Push 只在主屏 PWA 里可用）', async ({
      page,
    }) => {
      const bundle = watchBundle(page);
      await page.addInitScript(browserPushStub, 'granted');
      await page.goto(`${app.webBase}/#/settings/notify`);
      await settleBundle(page, bundle);

      await enablePushChannel(page);

      await expect(page.getByText('需要添加到主屏幕')).toBeVisible();
      await expect(page.getByText(/请先添加到主屏幕/).first()).toBeVisible();
      await expect(page.getByRole('button', { name: '订阅此设备' })).toHaveCount(0);

      expectBundleHealthy(bundle);
    });
  });
});
