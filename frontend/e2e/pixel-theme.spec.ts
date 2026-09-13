/**
 * E2E-⑨ 像素主题（主题六「像素机房 · 夜班流水线」，决策 169）。
 *
 * 让「前端确实变成了像素主题」这件事在**真应用**上有自动断言，而不是靠人眼：
 * 页面加载编译期内嵌的真实 bundle（决策 155），断言浏览器**实际计算出的样式**——
 * token 取值、圆角、描边宽度、硬投影。这是视觉改造的回归门。
 *
 * 分阶段落成（票 02 起始 → 票 12 全量）：本文件先覆盖 token 层与基元层已能生效的部分。
 * 断言口径（规格 Testing Decisions）：只测外部行为——计算样式与可见图元，
 * 不测 class 名、不测 CSS 源码措辞。
 *
 * 只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';
import { startApp, watchBundle, settleBundle, expectBundleHealthy, type App } from './harness';
import { fullPassScript } from './scripts';

/** 读根元素上的计算样式 token（`:root` 与 `html[data-theme]` 都落在此）。
 *  浏览器返回的值会小写化（#1B1D2C → #1b1d2c），故统一小写比较。 */
async function rootToken(page: import('@playwright/test').Page, name: string): Promise<string> {
  return page.evaluate(
    (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim().toLowerCase(),
    name,
  );
}

/** 把 `#RGB` / `#RRGGBB` 归一成 `rgb(r, g, b)`，便于与计算值比较。 */
function hexToRgb(hex: string): string {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const n = Number.parseInt(full, 16);
  return `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
}

const PIXEL_DARK = {
  bg: '#1B1D2C',
  pending: '#FFB545',
  textHi: '#F1ECDC',
};
const PIXEL_LIGHT = {
  bg: '#E8E6DC',
  pending: '#8F5B00',
  textHi: '#14151F',
};

test.describe('前端 E2E ⑨：像素主题（决策 169）', () => {
  let app: App;
  const title = 'E2E pixel theme';

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('E2E'), title });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('深色与浅色两套像素 token 生效，且切换真的换 token', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);

    const card = page.locator('article.card', { hasText: title });
    await expect(card).toBeVisible({ timeout: 60_000 });

    // ── 深色（默认）：像素 token 的计算值 ──
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
    expect(await rootToken(page, '--bg')).toBe(PIXEL_DARK.bg.toLowerCase());
    expect(await rootToken(page, '--pending')).toBe(PIXEL_DARK.pending.toLowerCase());
    expect(await rootToken(page, '--text-hi')).toBe(PIXEL_DARK.textHi.toLowerCase());

    // 页面底色真的用了这个 token。
    const bodyBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(bodyBg).toBe(hexToRgb(PIXEL_DARK.bg));

    // ── 像素纪律：圆角恒 0、描边只有 2px 一档 ──
    // 用像素基元 `.btn`（票 02 已改造；货箱的 2px 描边在票 05）——它出现在顶栏，
    // 任何路由都可见，且是「真实应用上算出来的样式」而非源码措辞。
    const btn = page.locator('button.btn').first();
    await expect(btn).toBeVisible();
    await expect(btn).toHaveCSS('border-radius', '0px');
    await expect(btn).toHaveCSS('border-top-width', '2px');
    await expect(btn).toHaveCSS('border-top-style', 'solid');

    // 面板基元同为 2px 描边 + 圆角 0（设置页的台账盒使用它）。
    const panel = page.locator('section.panel').first();
    if ((await panel.count()) > 0) {
      await expect(panel).toHaveCSS('border-radius', '0px');
      await expect(panel).toHaveCSS('border-top-width', '2px');
    }

    // ── 切到浅色：同一组 token 换成浅色值（证明 data-theme 真的换了材质） ──
    await page.evaluate(() => {
      document.documentElement.dataset.theme = 'light';
      localStorage.setItem('agentpipeline.theme', 'light');
    });
    expect(await rootToken(page, '--bg')).toBe(PIXEL_LIGHT.bg.toLowerCase());
    expect(await rootToken(page, '--pending')).toBe(PIXEL_LIGHT.pending.toLowerCase());
    expect(await rootToken(page, '--text-hi')).toBe(PIXEL_LIGHT.textHi.toLowerCase());
    const bodyBgLight = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(bodyBgLight).toBe(hexToRgb(PIXEL_LIGHT.bg));

    // 收尾：把主题偏好还原成深色，避免影响同一 worker 内的后续用例。
    await page.evaluate(() => {
      document.documentElement.dataset.theme = 'dark';
      localStorage.setItem('agentpipeline.theme', 'dark');
    });

    // ── 无未捕获页面错误（延续主流程票 01 的口径） ──
    expectBundleHealthy(bundle);
  });

  test('缝合像素字体自托管生效：本地加载、非回退到系统 monospace', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);

    // 字体 CSS 来自同源内嵌产物（决策 155），不是外部 CDN。
    const fontLinks = await page.evaluate(() =>
      [...document.querySelectorAll('link[rel=stylesheet]')].map((l) => l.getAttribute('href')),
    );
    expect(fontLinks.some((h) => h?.includes('/fonts/fusion-pixel-12px/'))).toBe(true);
    expect(fontLinks.some((h) => h?.includes('fonts.googleapis.com'))).toBe(false);

    // body 的计算字体族首选缝合像素（真加载与否由 web font API 复核）。
    const family = await page.evaluate(() => getComputedStyle(document.body).fontFamily);
    expect(family).toContain('Fusion Pixel 12px Monospaced');

    const loaded = await page.evaluate(async () => {
      await document.fonts.ready;
      return [...document.fonts].filter((f) => f.family.includes('Fusion Pixel')).length;
    });
    expect(loaded).toBeGreaterThan(0);
    expectBundleHealthy(bundle);
  });
});
