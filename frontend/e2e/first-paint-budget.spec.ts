/**
 * 首屏载荷闸门（决策 361，票 04）。
 *
 * 本仓此前零 bench 设施，而本批要防的回归面**窄到一条断言就够**：「有没有人把大载荷
 * 重新塞回任务详情的第一屏」。commands 是全站最大的一条（106 实测 `GET
 * /tasks/{id}/commands` 1,330,415 字节、公网链路上客户端 8.5–10.9 秒），而它挡住的正是
 * 时间线要的 `transitions`（来自 7 KB 的 `/flow`）——因为 `load()` 曾经把它们放进同一个
 * `Promise.all`。
 *
 * **可回归的指标是字节数与请求时机，不是秒数**：秒数在 CI 上必然抖动，只配当人工读数
 * （106 上那一组写在 `.scratch/scene-read-path-perf/spec.md` 的排查结论里）。故两条腿：
 *
 * ① **挂住 `/commands` 不返回**，断言时间线仍然渲染出来——票 02 的判据由它承载。
 *    把票 02 的改动回退（`getCommands` 挪回 `Promise.all`），这一条必红：那条挂住的
 *    请求永远不会 resolve，`transitions` 就永远不会赋值。
 * ② 首屏**数据面**响应的总字节数在预算之内（预算来由见 {@link FIRST_PAINT_BYTE_BUDGET}）。
 *
 * 真 axum 后端 + 内嵌 bundle + 临时 home（与其余 e2e 同一套装置，`harness.ts`）。
 */

import { expect, test, type Page } from '@playwright/test';

import { pendingTypeOf, startApp, waitForTask, type App } from './harness';
import { fullPassScript } from './scripts';

/**
 * 首屏数据面的字节预算。
 *
 * **来由**：`E2E_FIRST_PAINT_MEASURED` 是这条用例在实施时实测到的读数，预算取它的
 * 两倍出头——留够 fixture 微调（多一个文件、多两轮会话）的余量，又远小于任何一条
 * 「把 commands 塞回来」的量级（那条是 1.33 MB 起）。这个数字**不是调出来的**：
 * 越过它就说明首屏多了一类载荷，人该来看一眼。
 */
const E2E_FIRST_PAINT_MEASURED = 11_163; // 实施时实测：2,564（详情）+ 1,045（看板）+ 505（diff）+ 4,753（flow）+ 2,296（会话摘要）
const FIRST_PAINT_BYTE_BUDGET = 32_000;

/** 同一 origin 的「数据面」响应才算载荷：静态资源（bundle / 字体）不是本闸门的对象。 */
function isDataResponse(page: Page, url: URL): boolean {
  if (url.origin !== new URL(page.url() || 'http://127.0.0.1').origin) return false;
  // 事件流永不完结，读它的 body 会挂住；它也不是载荷。
  if (url.pathname.endsWith('/stream')) return false;
  return url.pathname.startsWith('/tasks');
}

interface Watched {
  total: () => number;
  paths: () => Record<string, number>;
  settle: () => Promise<void>;
}

/** 采集首屏期间数据面响应的字节数（`res.body()` 拿到的是**解码后**的正文）。 */
function watchDataBytes(page: Page): Watched {
  let total = 0;
  const paths: Record<string, number> = {};
  const inflight: Array<Promise<void>> = [];
  page.on('response', (res) => {
    const url = new URL(res.url());
    if (!isDataResponse(page, url)) return;
    inflight.push(
      res
        .body()
        .then((body) => {
          total += body.byteLength;
          paths[url.pathname] = (paths[url.pathname] ?? 0) + body.byteLength;
        })
        // 304 / 被中止的响应没有 body：不计入，也不让用例失败（那不是载荷）。
        .catch(() => undefined),
    );
  });
  return {
    total: () => total,
    paths: () => ({ ...paths }),
    settle: async () => {
      await Promise.all(inflight);
    },
  };
}

test.describe('前端 E2E：任务详情首屏载荷闸门（决策 361，票 04）', () => {
  let app: App;
  const title = 'E2E 首屏载荷';

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('E2E'), title });
    // 等流水线停下来：跑着的任务有 in-flight 命令与流式增量，那会让「首屏」不再是一次
    // 干净的装载。merge_approval 是这套脚本的天然停点（happy-path 用的也是它）。
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('首屏不等 /commands：时间线先渲染，载荷在字节预算内', async ({ page }) => {
    const watched = watchDataBytes(page);

    // ① 把 commands 挂住：**永不 resolve**。首屏若等它，时间线就永远出不来。
    let commandsHits = 0;
    await page.route('**/tasks/*/commands', (route) => {
      commandsHits += 1;
      // 故意不 fulfill / 不 continue：这条请求就此悬着。
    });

    await page.goto(`${app.webBase}/#/task/${app.taskId}`);

    // 时间线（默认页签）渲染出流转行 = 首屏真的完成了，且**没有等**那条挂住的请求。
    await expect(page.locator('.tline .trow').first()).toBeVisible({ timeout: 30_000 });

    // 后台补拉确实发生了（不是「压根没请求」蒙混过关）——票 02 要的是移出关键路径，
    // 不是取消它。
    expect(commandsHits, 'commands 应在后台被补拉（挂住的那一条）').toBeGreaterThan(0);

    await watched.settle();
    const total = watched.total();
    const paths = watched.paths();
    // 读数留在失败信息里：超预算时不必回头重跑一遍才知道多出来的是哪一条。
    expect(
      total,
      `首屏数据面载荷 ${total} 字节超预算 ${FIRST_PAINT_BYTE_BUDGET}（实测基线约 ${E2E_FIRST_PAINT_MEASURED}）：${JSON.stringify(paths)}`,
    ).toBeLessThan(FIRST_PAINT_BYTE_BUDGET);

    // 反向证据：/commands 与它的输出端点都**不在**首屏的载荷里
    expect(
      Object.keys(paths).filter((p) => p.endsWith('/commands')),
      `commands 不该进首屏：${JSON.stringify(paths)}`,
    ).toEqual([]);
  });

  test('现场页签：挂住批量会话，轮名牌先于正文出现', async ({ page }) => {
    // 批量那一跳（票 03 的 `?include_messages=true`）挂住不返回；摘要那一跳（无参数）
    // 照常放行——「有名牌、没正文」正是渐进填充要的那个中间态。
    let batchHits = 0;
    await page.route(
      (url) =>
        url.pathname.endsWith('/conversations') &&
        url.searchParams.get('include_messages') === 'true',
      (route) => {
        batchHits += 1;
        // 同前：挂着，不 fulfill。
      },
    );

    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await page.locator('.tabs button.tab', { hasText: '现场' }).click();

    // 名牌（来自先到的摘要）已经渲染，正文占位还在
    await expect(page.locator('article.turn .dname').first()).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('.quiet').first()).toContainText('正在读取会话');

    expect(batchHits, '批量会话应在后台被请求（挂住的那一条）').toBeGreaterThan(0);
  });
});
