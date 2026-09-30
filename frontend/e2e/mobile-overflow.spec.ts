/**
 * 前端 E2E：窄版面**不许横向溢出**（决策 341）。
 *
 * 钉的是 2026-09-30 的那条用户报障——「很多页面未对文字做换行导致整体被缩小」。
 * 「缩小」不是修辞：iOS 在 `width=device-width` 下遇到比视口宽的版面会把**布局视口**撑开
 * 再整体缩放显示，现场量到的是 `screen.width=390` 而 `window.innerWidth=1560`（对讲台，
 * 竖屏）与 `1930`（横屏）。所以这条用例断言两件事，缺一不可：
 *   ① 文档不横向溢出（`scrollWidth ≤ clientWidth`）——版面本身没被顶宽；
 *   ② 布局视口不比设备宽（`innerWidth ≤ screen.width`）——**没有发生收缩**。
 * 只断言①会漏掉「已经缩过、内容反而塞下了」那种退化。
 *
 * **内容必须是真的长 token**，否则这条用例恒绿（假绿比没有更坏）：值班长的回话里带着
 * 一段没有空格的 JSON，它就是报障现场那一段的等价物。用例先断言这段真的渲染出来了，
 * 再量几何。
 *
 * 断言口径沿用本仓既有约定：只测外部行为（视口几何），不测 CSS 源码措辞、不测 class 名。
 * 只 Chromium（决策 144）；后端与产物走 `harness`（回环绑定，故配对闸门不参与——闸门那半
 * 由 `crates/app/tests/integration/api_contract.rs` 钉）。
 */

import { expect, test, type Page } from '@playwright/test';
import { expectBundleHealthy, settleBundle, startApp, watchBundle, type App } from './harness';
import { foremanScript, text } from './scripts';

/** 报障现场那一段的等价物：**一个没有空格的长词**，markdown 正文里最常见的一类。 */
const LONG_TOKEN = `{"archived_at":null,"branch_name":null,"note":"${'A'.repeat(700)}"}`;
/** 断言「它真的渲染出来了」用的标记（缺了它，几何断言可能只是因为页面空着而绿）。 */
const MARK = '溢出闸门标记';

const REPLY = [MARK, '回执如下：', '', LONG_TOKEN].join('\n');

/** 一档视口下的四个读数。`layout` 就是「收缩有没有发生」的那个数。 */
async function viewport(page: Page) {
  return page.evaluate(() => ({
    screen: window.screen.width,
    layout: window.innerWidth,
    doc: document.documentElement.scrollWidth,
    client: document.documentElement.clientWidth,
  }));
}

test.describe('窄版面横向溢出（决策 341）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({
      script: foremanScript([[text(REPLY)]]),
      providerOnly: true,
      title: '窄版面溢出',
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** 两档都量：竖屏是报障那台机器的常态，横屏是 `@media (max-width: 479px)` 那批兜底规则**失效**的那一档。 */
  for (const [width, height] of [
    [390, 844],
    [844, 390],
  ] as const) {
    test(`对讲台回话里的长 token 不顶穿版面（${width}×${height}）`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      const bundle = watchBundle(page);
      await page.goto(`${app.webBase}/#/talk`);
      await settleBundle(page, bundle);

      // 让值班长真的回一段话（脚本的第一轮）
      const input = page.locator('.typer textarea');
      await expect(input).toBeVisible();
      await input.fill('回一段带长 token 的回执');
      await input.press('Enter');

      const reply = page.locator('.timeline .turn.fm', { hasText: MARK }).first();
      await expect(reply, '回话没进时间线，后面的几何断言会因为页面空着而假绿').toBeVisible({
        timeout: 30_000,
      });
      // 长 token 真的落进了 markdown 正文（不是被截断掉、也不是进了 `pre` 那种有意的横滚容器）。
      // 断言挂在**回话那一轮**上：时间线上还有别的 `.md`（「脚本已结束」那条也走 MarkdownView），
      // 拿整条时间线去 `toContainText` 会撞 strict mode。
      const md = reply.locator('.md').first();
      await expect(md).toContainText(MARK);
      await expect(md).toContainText('A'.repeat(50));
      await page.waitForTimeout(500);

      const m = await viewport(page);
      expect(m.doc, `版面被顶宽了（${m.doc} > ${m.client}）`).toBeLessThanOrEqual(m.client + 1);
      expect(
        m.layout,
        `布局视口被撑到 ${m.layout}（设备只有 ${m.screen}）——iOS 会据此把整页缩小`,
      ).toBeLessThanOrEqual(m.screen + 1);

      expectBundleHealthy(bundle);
    });
  }

  /**
   * 全路由扫一遍：把这条闸门铺到**没被报障的那些页**上，将来谁往版面里塞一个长 token
   * 都会被拦下来。路由取自 `router` 的全表（任务详情走空态那条，本 fixture 没有任务）。
   */
  const ROUTES = [
    '#/',
    '#/metrics',
    '#/settings',
    '#/settings/foreman',
    '#/settings/market',
    '#/settings/notify',
    '#/settings/projects',
    '#/settings/providers',
    '#/settings/stages',
    '#/settings/tools',
    '#/share',
    '#/talk',
    '#/talk/watch',
    '#/task/nope',
  ];

  test('每条路由在 390px 下都不横向溢出、不触发收缩', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const offenders: string[] = [];
    for (const route of ROUTES) {
      const bundle = watchBundle(page);
      await page.goto(`${app.webBase}/${route}`);
      await settleBundle(page, bundle);
      await page.waitForTimeout(800);
      const m = await viewport(page);
      if (m.doc > m.client + 1 || m.layout > m.screen + 1) {
        offenders.push(`${route}: doc=${m.doc} client=${m.client} layout=${m.layout} screen=${m.screen}`);
      }
      expectBundleHealthy(bundle);
    }
    expect(offenders, `这些路由在 390px 下版面超宽：\n${offenders.join('\n')}`).toEqual([]);
  });
});
