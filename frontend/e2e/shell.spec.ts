/**
 * 前端 E2E：浏览器壳（决策 282 ④，票 talk-mobile-space-2/02）。
 *
 * 「壳层让位」的可见部分都在静态产物里：manifest、apple meta、图标、viewport meta。
 * 这里钉住的是**可机核的那一半**——meta 内容、manifest 可取且声明 standalone、图标资源
 * 真的可取（png 字节）；真机上的收益（iPhone standalone 少地址栏底栏、安卓键盘压缩版面）
 * 只能人手验收，票面记录。
 */

import { expect, test } from '@playwright/test';
import { expectBundleHealthy, settleBundle, startApp, watchBundle, type App } from './harness';
import { fullPassScript } from './scripts';

test.describe('浏览器壳（决策 282 ④）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('E2E'), seedless: true, title: '壳层' });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('viewport meta 带键盘收缩键，manifest / apple meta / 图标齐全且可取', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/`);
    await settleBundle(page, bundle);

    // ① 键盘收缩（安卓 Chrome/WebView 生效；iPhone 由 WebKit 自理）
    const viewport = await page.evaluate(() =>
      document.querySelector('meta[name="viewport"]')?.getAttribute('content'),
    );
    expect(viewport, 'viewport meta 缺 viewport-fit=cover').toContain('viewport-fit=cover');
    expect(viewport, 'viewport meta 缺 interactive-widget=resizes-content').toContain(
      'interactive-widget=resizes-content',
    );

    // ② manifest：链接在页面上、资源可取、声明 standalone、四枚图标逐枚可取
    const href = await page.evaluate(() =>
      document.querySelector('link[rel="manifest"]')?.getAttribute('href'),
    );
    expect(href, 'manifest 链接不在页面上').toBeTruthy();
    const res = await page.request.get(`${app.webBase}${href}`);
    expect(res.status(), 'manifest 取不到').toBe(200);
    const mf = await res.json();
    expect(mf.display, 'manifest 没声明 standalone').toBe('standalone');
    expect(mf.name, 'manifest 缺应用名').toBeTruthy();
    expect(mf.icons?.length ?? 0, 'manifest 没有图标').toBeGreaterThan(0);
    for (const icon of mf.icons) {
      const ir = await page.request.get(`${app.webBase}${icon.src}`);
      expect(ir.status(), `图标取不到：${icon.src}`).toBe(200);
      expect(
        ir.headers()['content-type'] ?? '',
        `图标不是 png：${icon.src}`,
      ).toContain('image/png');
      expect((await ir.body()).length, `图标是空文件：${icon.src}`).toBeGreaterThan(0);
    }

    // ③ iOS 主屏那组 meta 与 apple-touch-icon（图形与桌面壳同一份母版的满幅版）
    expect(
      await page.evaluate(
        () => document.querySelector('meta[name="apple-mobile-web-app-capable"]')?.content,
      ),
    ).toBe('yes');
    expect(
      await page.evaluate(
        () => document.querySelector('meta[name="apple-mobile-web-app-status-bar-style"]')?.content,
      ),
    ).toBe('black');
    const touch = await page.evaluate(() =>
      document.querySelector('link[rel="apple-touch-icon"]')?.getAttribute('href'),
    );
    expect(touch, 'apple-touch-icon 链接不在页面上').toBeTruthy();
    const tr = await page.request.get(`${app.webBase}${touch}`);
    expect(tr.status(), 'apple-touch-icon 取不到').toBe(200);
    expect((await tr.body()).length).toBeGreaterThan(0);

    expectBundleHealthy(bundle);
  });
});
