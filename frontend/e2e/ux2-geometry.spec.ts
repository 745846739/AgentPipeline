/**
 * UX 第二轮 · 票 05 / 08 / 09（R2-02 / R2-10 / R2-11）：三处几何——动作坞与状态行、
 * 状态行自己的宽度档位、档案盒的 sticky 让位。
 *
 * 这一组**断言数字与矩形**，不靠截图肉眼：叠放关系用矩形的交集、可达性用 `toBeInViewport`
 * 加一次真按（深浅切换按得到、主题真的变了）、让位用铭牌与顶栏的交集。
 * 宽度扫描的采样点按决策 215 的档位边界取（1440 / 1024 / 900 / 820 / 768 / 700 / 600 /
 * 560 / 520 / 500 / 480），**中段必须有**——上一轮就是只取了两个端点才漏掉这一整段。
 *
 * 真 axum 后端 + mock LLM + 临时 home；只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  expectBundleHealthy,
  pendingTypeOf,
  settleBundle,
  startApp,
  waitForTask,
  watchBundle,
  type App,
} from './harness';
import { fullPassScript } from './scripts';

async function themeOf(page: import('@playwright/test').Page): Promise<string> {
  return page.evaluate(() => document.documentElement.dataset.theme ?? '');
}

test.describe('UX2 ⑥ 几何：坞 / 状态行 / 档案盒（票 05 / 08 / 09）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('UX2G'), title: 'UX2 几何' });
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval', 180_000);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('手机上动作坞给状态行让位：矩形交集为 0，深浅切换按得到', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 932 });
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);
    await page.waitForTimeout(400);

    const boxes = await page.evaluate(() => {
      const dock = document.querySelector('.dock') as HTMLElement | null;
      const bar = document.querySelector('footer.statusline') as HTMLElement | null;
      if (!dock || !bar) return null;
      const d = dock.getBoundingClientRect();
      const b = bar.getBoundingClientRect();
      return {
        dock: { top: Math.round(d.top), bottom: Math.round(d.bottom) },
        status: { top: Math.round(b.top), bottom: Math.round(b.bottom) },
        overlap: Math.round(Math.max(0, Math.min(d.bottom, b.bottom) - Math.max(d.top, b.top))),
        statusText: (bar.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40),
      };
    });
    expect(boxes, '这一态应当同时有动作坞与状态行').not.toBeNull();
    expect(boxes!.overlap, '动作坞不应压住状态行（此前实测压掉 42px = 100%）').toBe(0);
    expect(boxes!.status.top).toBeGreaterThanOrEqual(boxes!.dock.bottom - 1);

    // 状态行真的可见（不是被盖住的那种「在 DOM 里」）
    const status = page.locator('footer.statusline');
    await expect(status).toBeInViewport();

    // 深浅切换：按得到，且主题真的变了
    const toggle = page.getByRole('button', { name: /切换到(浅色|深色)主题/ });
    await expect(toggle).toBeInViewport();
    const before = await themeOf(page);
    await toggle.click();
    await expect
      .poll(async () => themeOf(page), { message: '按下深浅切换后主题应当真的变' })
      .not.toBe(before);

    // 坞上的按钮仍然按得到（没有被新的让位顶出屏幕）
    const dockBtn = page.locator('.dock button').first();
    await expect(dockBtn).toBeInViewport();
    expectBundleHealthy(bundle);
  });

  test('状态行在 1440→480 逐档不越界，且在 500 / 600 按得到深浅切换', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);

    for (const w of [1440, 1024, 900, 820, 768, 700, 600, 560, 520, 500, 480]) {
      await page.setViewportSize({ width: w, height: 900 });
      await page.waitForTimeout(150);
      const m = await page.evaluate(() => {
        const bar = document.querySelector('footer.statusline') as HTMLElement | null;
        if (!bar) return null;
        const clock = bar.querySelector('.clock') as HTMLElement | null;
        const tog = bar.querySelector('.theme-tog') as HTMLElement | null;
        return {
          overflow: bar.scrollWidth - bar.clientWidth,
          clockVisible: clock ? clock.getBoundingClientRect().width > 0 : null,
          togRight: tog ? Math.round(tog.getBoundingClientRect().right) : null,
          viewport: window.innerWidth,
        };
      });
      expect(m, `w=${w}：状态行应当在`).not.toBeNull();
      // 无静默裁切：内容不越出容器（舍格优先、可滚兜底）
      expect(m!.overflow, `w=${w}：状态行内容越出容器（静默裁切）`).toBeLessThanOrEqual(0);
      // 主题钮整块在视口里
      expect(m!.togRight, `w=${w}：深浅切换被顶出视口`).toBeLessThanOrEqual(m!.viewport);
      // **时钟不许静默消失**（R2-10 的原症状：`clockRight=748` 恒定 > 视口，整块被裁掉）。
      // 只断言 overflow 是不够的——把内容 `display:none` 掉也满足「不越界」，而那时钟就
      // 正好是评审说的那种「把症状藏起来」。480 及以上每一档都保留时钟（<480 才是既有
      // 移动款：那里由手机状态栏报时）。
      expect(m!.clockVisible, `w=${w}：时钟不该在这一档消失`).toBe(true);
    }

    for (const w of [600, 500]) {
      await page.setViewportSize({ width: w, height: 900 });
      await page.waitForTimeout(150);
      const toggle = page.getByRole('button', { name: /切换到(浅色|深色)主题/ });
      await expect(toggle, `w=${w}：深浅切换必须可见可点`).toBeInViewport();
      const before = await themeOf(page);
      await toggle.click();
      await expect.poll(async () => themeOf(page)).not.toBe(before);
    }
    // 时钟的可见性在每一档都断言过了（见循环里那条 `clockVisible`）
    expectBundleHealthy(bundle);
  });

  test('档案盒吸顶时「等你拍板」铭牌不被顶栏盖住', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);
    await expect(page.locator('aside.dossier')).toBeVisible({ timeout: 60_000 });

    await page.evaluate(() => window.scrollTo(0, 500));
    await page.waitForTimeout(400);

    const m = await page.evaluate(() => {
      const header = document.querySelector('header') as HTMLElement | null;
      const tag = document.querySelector('.dossier .dtag') as HTMLElement | null;
      const dossier = document.querySelector('.dossier') as HTMLElement | null;
      if (!header || !tag || !dossier) return null;
      const h = header.getBoundingClientRect();
      const t = tag.getBoundingClientRect();
      const overlapY = Math.max(0, Math.min(h.bottom, t.bottom) - Math.max(h.top, t.top));
      const overlapX = Math.max(0, Math.min(h.right, t.right) - Math.max(h.left, t.left));
      return {
        headerBottom: Math.round(h.bottom),
        tagTop: Math.round(t.top),
        tagBottom: Math.round(t.bottom),
        overlap: Math.round(Math.min(overlapX, overlapY)),
        stickyTop: getComputedStyle(dossier).top,
        inViewport: t.top >= 0 && t.bottom <= window.innerHeight,
      };
    });
    expect(m, '待拍板任务应当有档案盒与铭牌').not.toBeNull();
    expect(m!.overlap, `铭牌与顶栏相交（tagTop=${m!.tagTop} headerBottom=${m!.headerBottom}）`).toBe(
      0,
    );
    expect(m!.tagTop, '铭牌必须在视口内（不被顶栏吃掉）').toBeGreaterThanOrEqual(
      m!.headerBottom - 2,
    );
    expect(m!.inViewport, '铭牌整块都在视口里').toBe(true);
    expectBundleHealthy(bundle);
  });
});
