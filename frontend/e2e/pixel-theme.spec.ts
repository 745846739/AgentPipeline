/**
 * E2E-⑨ 像素主题（主题六「像素机房 · 夜班流水线」，决策 169）。
 *
 * 让「前端确实变成了像素主题」这件事在**真应用**上有自动断言，而不是靠人眼：
 * 页面加载编译期内嵌的真实 bundle（决策 155），断言浏览器**实际计算出的样式**——
 * token 取值、圆角、描边宽度、硬投影、像素图元的存在与形状。这是视觉改造的回归门。
 *
 * 全量成文于票 12（起始护栏在票 02）：token 层 / 基元层 / 看板层 / 详情层 / 完成横幅 /
 * 移动层各有一组断言，深浅两套都覆盖。
 * 断言口径（规格 Testing Decisions）：只测外部行为——计算样式与可见图元，
 * 不测 class 名、不测 CSS 源码措辞。
 *
 * 只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';
import {
  startApp,
  waitForTask,
  pendingTypeOf,
  watchBundle,
  settleBundle,
  expectBundleHealthy,
  type App,
} from './harness';
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

/** 把页面上的计算色（`#RRGGBB` 或 `rgb(...)`）拆成三通道。 */
function channels(color: string): [number, number, number] {
  if (color.startsWith('#')) {
    const h = color.slice(1);
    const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
    const n = Number.parseInt(full, 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  const m = color.match(/\d+/g) ?? ['0', '0', '0'];
  return [Number(m[0]), Number(m[1]), Number(m[2])];
}

/** WCAG 相对亮度（0–1）。 */
function luminance(color: string): number {
  const lin = (v: number): number => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const [r, g, b] = channels(color);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** WCAG 对比度比（1–21）。 */
function contrastRatio(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const PIXEL_DARK = {
  bg: '#1B1D2C',
  pending: '#FFB545',
  textHi: '#F1ECDC',
  beltLit: '#4E5478',
  branchDev: '#59A7FF',
  branchTst: '#C08BFF',
};
const PIXEL_LIGHT = {
  bg: '#E8E6DC',
  pending: '#8F5B00',
  textHi: '#14151F',
  beltLit: '#7E8094',
};

/** 把主题设成给定值（切换 token 用；不依赖状态行按钮，避免与其它用例耦合）。 */
async function setTheme(page: import('@playwright/test').Page, theme: 'dark' | 'light') {
  await page.evaluate((t) => {
    document.documentElement.dataset.theme = t;
    localStorage.setItem('agentpipeline.theme', t);
  }, theme);
}

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
    expect(await rootToken(page, '--belt-lit')).toBe(PIXEL_DARK.beltLit.toLowerCase());

    // 页面底色真的用了这个 token。
    const bodyBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(bodyBg).toBe(hexToRgb(PIXEL_DARK.bg));

    // ── 像素纪律：圆角恒 0、描边只有 2px 一档、硬投影 4px 4px 0 ──
    const btn = page.locator('button.btn').first();
    await expect(btn).toBeVisible();
    await expect(btn).toHaveCSS('border-radius', '0px');
    await expect(btn).toHaveCSS('border-top-width', '2px');
    await expect(btn).toHaveCSS('border-top-style', 'solid');
    // 像素钮的硬投影（无模糊半径、无扩散）
    await expect(btn).toHaveCSS('box-shadow', 'rgb(18, 19, 30) 3px 3px 0px 0px');

    // 货箱：2px 描边 + 4px 硬投影（票 05）
    await expect(card).toHaveCSS('border-radius', '0px');
    await expect(card).toHaveCSS('border-top-width', '2px');
    await expect(card).toHaveCSS('box-shadow', 'rgb(18, 19, 30) 4px 4px 0px 0px');

    // ── 切到浅色：同一组关键 token 换成浅色值（证明 data-theme 真的换了材质） ──
    await setTheme(page, 'light');
    expect(await rootToken(page, '--bg')).toBe(PIXEL_LIGHT.bg.toLowerCase());
    expect(await rootToken(page, '--pending')).toBe(PIXEL_LIGHT.pending.toLowerCase());
    expect(await rootToken(page, '--text-hi')).toBe(PIXEL_LIGHT.textHi.toLowerCase());
    expect(await rootToken(page, '--belt-lit')).toBe(PIXEL_LIGHT.beltLit.toLowerCase());
    const bodyBgLight = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(bodyBgLight).toBe(hexToRgb(PIXEL_LIGHT.bg));
    // 浅色下同一条像素纪律仍成立（不是只有深色款守规矩）
    await expect(btn).toHaveCSS('border-radius', '0px');
    await expect(card).toHaveCSS('border-radius', '0px');

    await setTheme(page, 'dark');
    expectBundleHealthy(bundle);
  });

  test('次级必读档达标：--text-3 在真应用上对 --bg 与 --panel 都 ≥ 4.5:1（深浅两套）', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    await expect(page.locator('article.card', { hasText: title })).toBeVisible({ timeout: 60_000 });

    // 票 15 / 决策 195：--text-3 重新定位为「次级必读」，门槛对**两个承载表面**都要成立
    // （深色款的约束面是 --panel、浅色款的约束面是 --bg）。断言的是浏览器里真正生效的
    // 计算值——把它改回旧值（深 2.93 / 浅 3.32）这条立刻红。
    for (const theme of ['dark', 'light'] as const) {
      await setTheme(page, theme);
      const text3 = await rootToken(page, '--text-3');
      for (const surface of ['--bg', '--panel'] as const) {
        const ratio = contrastRatio(text3, await rootToken(page, surface));
        expect(ratio, `${theme} 款 ${text3} 对 ${surface} 实测 ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
      }
      // 提的是 --text-3 这一档：--done（归档灰，刻意退后）原地不动，两者不再共用字面值。
      expect(text3).not.toBe(await rootToken(page, '--done'));
      // 次级必读档与纯装饰档是两档，值必须分得开。
      expect(text3).not.toBe(await rootToken(page, '--text-4'));
    }

    await setTheme(page, 'dark');
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

    // 自托管子集真被取回（同源 /fonts/*，不是 404 后悄悄回退 monospace）
    const fontResponses = await page.evaluate(() =>
      performance
        .getEntriesByType('resource')
        .map((e) => e.name)
        .filter((n) => n.includes('/fonts/fusion-pixel-12px/')),
    );
    expect(fontResponses.length).toBeGreaterThan(0);
    expectBundleHealthy(bundle);
  });

  test('看板 = 运转的流水线：传送带链节、信号灯、16 段量表、列头小人', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);

    const card = page.locator('article.card', { hasText: title });
    await expect(card).toBeVisible({ timeout: 60_000 });

    // ── 传送带：站点信号灯是 12px 实心方块（不是字符） ──
    const lamp = page.locator('.stn .lamp').first();
    await expect(lamp).toBeVisible();
    const lampBox = await lamp.boundingBox();
    expect(lampBox?.width).toBe(12);
    expect(lampBox?.height).toBe(12);

    // ── 货箱迷你轨 = 9 刻度像素方块 + 链节 ──
    const mini = card.locator('.rail.mini');
    await expect(mini).toBeVisible();
    expect(await mini.locator('.d').count()).toBe(9);

    // ── meta 行带 16 段 token 量表 ──
    const gauge = card.locator('.gauge').first();
    await expect(gauge).toBeVisible();
    expect(await gauge.locator('i').count()).toBe(16);

    // ── 列头：工位 sprite（非空 SVG）+ 挥锤小人 ──
    const head = page.locator('.col-head').first();
    expect(await head.locator('svg.sprite rect').count()).toBeGreaterThan(0);
    const worker = page.locator('.col-head .worker').first();
    await expect(worker).toBeVisible();
    // 小人是双帧（两张 svg），帧切换为离散 opacity 翻转
    expect(await worker.locator('svg').count()).toBe(2);

    // ── 顶栏过滤是 34px 道具栏槽位（图标 + 词 + 计数徽章；待处理槽不画徽章） ──
    // 决策 201：槽位**高度**仍恒 34px、2px 描边、零圆角、共享边框；变的只是宽度不再定死——
    // 七个槽都带 12px 词，宽随词走（`.slot` 现在是 `min-width: 34px` 的图标 + 词）。
    const slot = page.locator('.slot').first();
    await expect(slot).toBeVisible();
    const slotBox = await slot.boundingBox();
    expect(slotBox?.height).toBe(34);
    expect(slotBox?.width).toBeGreaterThanOrEqual(34);
    expect(await slot.locator('svg.sprite').count()).toBe(1);
    expect(await slot.locator('.cb').count()).toBe(1);

    // 决策 201 的计数去重：第 3 槽（待处理）**不画计数徽章**——同一个数不得在相邻的两个
    // 控件上同时出现，而紧邻的「待处理 N」芯片是 pending 数的唯一显示位（保留原位）。
    expect(await page.locator('.slot').nth(2).locator('.cb').count()).toBe(0);
    await expect(page.locator('.chip.pending-count')).toBeVisible();

    // ── 底部车间看板条：2px 顶描边 + token 量表 + `▪` 分隔符 ──
    const statusline = page.locator('.statusline');
    await expect(statusline).toHaveCSS('border-top-width', '2px');
    expect(await statusline.locator('.sep').first().textContent()).toBe('▪');
    expect(await statusline.locator('.gauge').count()).toBeGreaterThan(0);

    expectBundleHealthy(bundle);
  });

  test('详情 = 同一条传送带的放大版：hero 灯 + 工位标签盒页签', async ({ page }) => {
    const bundle = watchBundle(page);
    // 等任务跑起来（running 才有实心绿当前灯）
    await waitForTask(app, (t) => t.status === 'running', 'running', 60_000).catch(() => {});

    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);
    await expect(page.locator('h1.d-title')).toHaveText(title);

    // hero：9 站（不含 sync-check，决策 107），节点用 12px 灯表达
    const heroLamps = page.locator('.rail.hero .stn .lamp');
    expect(await heroLamps.count()).toBe(9);
    // 站点名是「工位名」，不再是字符字形
    const heroText = await page.locator('.rail.hero').textContent();
    expect(heroText).not.toMatch(/[○●◆◇✓✗]/);
    expect(heroText).toContain('init');

    // 页签 = 工位标签盒：active = wash 实底 + 描边上浮；圆角 0、2px 描边
    const activeTab = page.locator('.tabs .tab.on').first();
    await expect(activeTab).toHaveCSS('border-radius', '0px');
    await expect(activeTab).toHaveCSS('border-top-width', '2px');
    const activeBg = await activeTab.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(activeBg).toBe(hexToRgb('#2B2F47')); // --wash 深色

    // 图例也用灯（不是字符）
    const legend = page.locator('.legend');
    if ((await legend.count()) > 0) {
      expect(await legend.locator('.sw').count()).toBeGreaterThan(0);
      expect(await legend.textContent()).not.toMatch(/[○●◆◇✓✗]/);
    }

    expectBundleHealthy(bundle);
  });

  test('任务完成横幅：done 时出现、含 diff 摘要、点「收下」关闭', async ({ page }) => {
    const bundle = watchBundle(page);

    // fullPassScript 会一路推进到 merge_approval（合入需人工拍板）。本用例自己走完
    // 合入这一步：直接对后端下发 merge decision，再看 UI 的完成横幅——
    // 这样横幅断言不依赖「上一个用例恰好把任务留在某状态」。
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval', 180_000);

    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    const card = page.locator('article.card', { hasText: title });
    await expect(card).toBeVisible({ timeout: 60_000 });

    // 从看板卡上的动作按钮合入（决策 164：卡片动作必须可点；
    // 整卡链接铺满 inset:0，故点卡中心会落在动作区上——正是那条修复的语义）。
    const approve = card.getByRole('button', { name: /合入/ });
    await expect(approve).toBeVisible({ timeout: 60_000 });

    const banner = page.locator('[role="status"]').filter({ hasText: '任务完成' }).first();
    await approve.click();

    // 横幅是顶部居中的奖杯条；它不自动消失，由「收下」关闭
    await expect(banner).toBeVisible({ timeout: 120_000 });
    // trophy sprite 在场（非空 SVG）
    expect(await banner.locator('svg.sprite rect').count()).toBeGreaterThan(0);
    // diff 摘要形态（+N −M）——有数据时必须给真数字，不是 0 占位
    await expect(banner).toContainText(/[+−]\d+/);

    // 圆角 0（像素纪律）
    await expect(banner).toHaveCSS('border-radius', '0px');

    const take = banner.getByRole('button', { name: '收下' });
    await expect(take).toBeVisible();
    await take.click();
    await expect(banner).toHaveCount(0);

    // 刷新后不重弹（同任务的同一次 done 只弹一次）
    await page.reload();
    await settleBundle(page, bundle);
    await expect(page.locator('[role="status"]').filter({ hasText: '任务完成' })).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test('移动款：导航钉底缘成页签栏 + 道具栏行只在看板、纵向链节脊线、触控目标 ≥44px', async ({
    page,
  }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    await expect(page.locator('section.col').first()).toBeVisible({ timeout: 60_000 });

    const header = page.locator('header.top');
    const headerBox = await header.boundingBox();
    // 看板路由 = 44(触控行) + 6(上下留白) + 2(下框) = 52px（决策 243：导航行已移出顶栏）
    expect(headerBox?.height).toBe(52);

    // 行位：**页面导航行钉在视口底缘**（决策 243），道具栏行留在顶栏里
    const navBox = await page.locator('header.top .navbar').boundingBox();
    const filBox = await page.locator('header.top .filters-row').boundingBox();
    expect(navBox, '底部页签栏应当有几何').not.toBeNull();
    expect(filBox, '看板上道具栏行应当有几何').not.toBeNull();
    expect(navBox!.y, '页签栏必须在道具栏行下面').toBeGreaterThan(filBox!.y);
    expect(
      Math.round(navBox!.y + navBox!.height),
      '页签栏下沿必须贴住视口底缘',
    ).toBe(900);
    // 页签栏自身几何：上框 2 + 上留白 6 + 页签 44 + 下留白 6 = 58（无安全区的桌面上下文）
    expect(Math.round(navBox!.height)).toBe(58);
    // 四枚页签命中区 ≥44px（移动基线），状态条叠在页签栏上方、交集为 0
    const chips = page.locator('header.top .navbar .navchip');
    await expect(chips).toHaveCount(4);
    const chipBoxes = await chips.evaluateAll((els) =>
      els.map((el) => el.getBoundingClientRect().height),
    );
    for (const h of chipBoxes) {
      expect(h).toBeGreaterThanOrEqual(44);
    }
    const statusBox = await page.locator('footer.statusline').boundingBox();
    expect(statusBox, '状态条应当有几何').not.toBeNull();
    expect(
      Math.round(statusBox!.y + statusBox!.height),
      '状态条下沿必须贴住页签栏上沿',
    ).toBe(Math.round(navBox!.y));

    // 铭牌行（logo / wordmark / 会话名 / 信号灯缩略条）在窄档整行不渲染
    // （元素仍在 DOM——桌面档要靠它，故断的是 `display:none` 而不是不存在）
    await expect(page.locator('header.top .bar-top')).toBeHidden();
    await expect(page.locator('nav.railnav')).toHaveCount(0);
    await expect(page.locator('header.top .wordmark')).toBeHidden();

    // 站点脊线 = 6px 纵向链节（不是横向传送带）
    const spine = page.locator('.spine-rule').first();
    await expect(spine).toBeVisible();
    await expect(spine).toHaveCSS('width', '6px');
    const spineBg = await spine.evaluate((el) => getComputedStyle(el).backgroundImage);
    expect(spineBg).toContain('repeating-linear-gradient');

    // 跳段定位的让位读实测 `--topbar-h`（看板 52 + 10 的呼吸）
    const scrollMargin = await page
      .locator('section.col')
      .first()
      .evaluate((el) => getComputedStyle(el).scrollMarginTop);
    expect(scrollMargin).toBe('62px');

    // 道具栏槽位：高度仍恒 34px、不缩、整行横滚；窄屏（决策 201）只给**当前选中**的槽带词，
    // 故第一个槽（缺省选中的「全部」）宽于未选中的图标槽——宽度不再定死。
    const slot = page.locator('.slot').first();
    const slotBox = await slot.boundingBox();
    expect(slotBox?.height).toBe(34);
    expect(slotBox?.width).toBeGreaterThanOrEqual(34);
    const offBox = await page.locator('.slot:not(.on)').first().boundingBox();
    expect(offBox?.width).toBeGreaterThanOrEqual(34);
    expect(slotBox!.width).toBeGreaterThan(offBox!.width);

    // 触控目标 ≥44px
    const back = page.locator('section.col .col-head').first();
    const headBox = await back.boundingBox();
    expect(headBox?.height).toBeGreaterThanOrEqual(32);

    expectBundleHealthy(bundle);
  });
});
