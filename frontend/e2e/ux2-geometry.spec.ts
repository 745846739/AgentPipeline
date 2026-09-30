/**
 * UX 第二轮 · 票 05 / 08 / 09（R2-02 / R2-10 / R2-11）：三处几何——窄档动作坞与底部
 * 页签栏、状态行自己的宽度档位（仅 ≥480）、档案盒的 sticky 让位。
 *
 * 这一组**断言数字与矩形**，不靠截图肉眼：叠放关系用矩形的交集、可达性用 `toBeInViewport`
 * 加一次真按（深浅切换按得到、主题真的变了）、让位用铭牌与顶栏的交集。
 * 宽度扫描的采样点按决策 215 的档位边界取（1440 / 1024 / 900 / 820 / 768 / 700 / 600 /
 * 560 / 520 / 500 / 480），**中段必须有**——上一轮就是只取了两个端点才漏掉这一整段。
 * **窄档（≤479）的状态条已整条退场**（决策 300，修订决策 243 的 ②④）：那一条的让位
 * 判据改盯页签栏，深浅切换的可达性改在设置落地页页头上验（手机端唯一入口）。
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

  test('手机上动作坞贴页签栏上沿（交集为 0），换配色在设置落地页按得到', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 932 });
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);
    await page.waitForTimeout(400);

    // 状态条在窄档整条退场（决策 300）：底部从此只有页签栏一层，`--sbar-h` = `--nav-h`
    await expect(page.locator('footer.statusline')).toBeHidden();

    const boxes = await page.evaluate(() => {
      const dock = document.querySelector('.dock') as HTMLElement | null;
      const nav = document.querySelector('header.top .navbar') as HTMLElement | null;
      if (!dock || !nav) return null;
      const d = dock.getBoundingClientRect();
      const n = nav.getBoundingClientRect();
      return {
        dock: { top: Math.round(d.top), bottom: Math.round(d.bottom) },
        nav: { top: Math.round(n.top), bottom: Math.round(n.bottom) },
        overlap: Math.round(Math.max(0, Math.min(d.bottom, n.bottom) - Math.max(d.top, n.top))),
      };
    });
    expect(boxes, '这一态应当同时有动作坞与底部页签栏').not.toBeNull();
    expect(boxes!.overlap, '动作坞不应压住页签栏').toBe(0);
    expect(boxes!.nav.top, '坞的下沿不该越过页签栏的上沿').toBeGreaterThanOrEqual(
      boxes!.dock.bottom - 1,
    );

    // 坞上的按钮仍然按得到（没有被新的让位顶出屏幕）
    const dockBtn = page.locator('.dock button').first();
    await expect(dockBtn).toBeInViewport();

    // 深浅切换：窄档唯一入口在**设置落地页页头**（决策 300）——按得到，且主题真的变了。
    // 用 `.p-head` 限定作用域：桌面档状态行那枚还在 DOM 里，虽然窄档 `display:none`
    // 不参与 role 查询，但作用域写死在源头，将来谁把状态条放回来也不会撞 strict mode。
    await page.goto(`${app.webBase}/#/settings`);
    await settleBundle(page, bundle);
    const toggle = page.locator('.p-head .theme-tog');
    await expect(toggle).toBeInViewport();
    const before = await themeOf(page);
    await toggle.click();
    await expect
      .poll(async () => themeOf(page), { message: '按下深浅切换后主题应当真的变' })
      .not.toBe(before);

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

  /**
   * ③ 档案盒吸顶：铭牌不被顶栏盖住（票 09 / R2-11）。
   *
   * **取样要取在「吸顶」那一段**（2026-09-30，决策 337）：这一页总共只能滚 156px，原先写的
   * `scrollTo(0, 500)` 被夹到 156——那是**行程末端**。档案盒的 sticky 行程受它包含块
   * （网格行）的下沿所限，滚到头之后它随页往上走：实测 dossierTop 从钉位的 94 掉到 86，
   * 铭牌跟着上移 8px，于是与顶栏相交 6px。那不是让位算错，是它在**离场**——顶栏那条
   * `top: calc(var(--topbar-h) + 16px)` 一个字都没错（探针曲线：滚 22–140 恒为 94/80）。
   *
   * 所以这里把位置算在**钉住之后、离场之前**，并且先断言「确实吸顶了」：用例的名字叫
   * 「吸顶时……」，此前却没有一条断言真的验证它吸顶（只查了不相交）。
   */
  test('档案盒吸顶时「等你拍板」铭牌不被顶栏盖住', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);
    await expect(page.locator('aside.dossier')).toBeVisible({ timeout: 60_000 });

    // 钉位读**实测**的 CSS（`--topbar-h` 一档一变，写死就是又一个对不上的魔数）；
    // +90px 是钉住之后的余量——离场那一段在页尾（这一页实测：滚 140px 仍钉住、156px 已离场，
    // 而 500 会被夹到 156）。
    await page.evaluate(() => {
      const dossier = document.querySelector('.dossier') as HTMLElement;
      const pin = parseFloat(getComputedStyle(dossier).top);
      const natural = dossier.getBoundingClientRect().top + window.scrollY;
      window.scrollTo(0, natural - pin + 90);
    });
    await page.waitForTimeout(400);

    const m = await page.evaluate(() => {
      const header = document.querySelector('header') as HTMLElement | null;
      const tag = document.querySelector('.dossier .dtag') as HTMLElement | null;
      const dossier = document.querySelector('.dossier') as HTMLElement | null;
      if (!header || !tag || !dossier) return null;
      const h = header.getBoundingClientRect();
      const t = tag.getBoundingClientRect();
      const d = dossier.getBoundingClientRect();
      const pin = parseFloat(getComputedStyle(dossier).top);
      const overlapY = Math.max(0, Math.min(h.bottom, t.bottom) - Math.max(h.top, t.top));
      const overlapX = Math.max(0, Math.min(h.right, t.right) - Math.max(h.left, t.left));
      return {
        headerBottom: Math.round(h.bottom),
        tagTop: Math.round(t.top),
        tagBottom: Math.round(t.bottom),
        dossierTop: Math.round(d.top),
        pin: Math.round(pin),
        overlap: Math.round(Math.min(overlapX, overlapY)),
        inViewport: t.top >= 0 && t.bottom <= window.innerHeight,
      };
    });
    expect(m, '待拍板任务应当有档案盒与铭牌').not.toBeNull();
    expect(
      Math.abs(m!.dossierTop - m!.pin),
      `这一档应当已经吸顶：dossierTop=${m!.dossierTop} 钉位=${m!.pin}（页不够长就够不着钉位，那是取样点的问题）`,
    ).toBeLessThanOrEqual(1);
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
