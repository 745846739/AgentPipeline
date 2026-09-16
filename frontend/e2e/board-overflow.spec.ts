/**
 * E2E：看板溢出（票 17 / 决策 196）、脊线数字的口径标注（票 19 / 决策 197）、
 * 并行站名标签的内边距（票 10）。
 *
 * 断言口径（规格 Testing Decisions）：只测**用户看得见的东西**——元素在视口里的几何、
 * 计算样式、右缘指示的存在与否、屏上读到的字。不测 class 名的语义、不测 CSS 源码措辞、
 * 不做说明性文案的精确匹配（契约性字符串如「累计」「还有 N 列」除外，它们是本届拍板的定稿）。
 *
 * 只 Chromium（决策 144），与 `pixel-theme.spec.ts` 同一写法：真后端 + 内嵌真产物。
 */

import { expect, test, type Page } from '@playwright/test';
import { expectBundleHealthy, settleBundle, startApp, watchBundle, type App } from './harness';
import { fullPassScript } from './scripts';

test.describe('前端 E2E：看板溢出与脊线口径（票 10 / 17 / 19）', () => {
  let app: App;
  const title = 'E2E board overflow';

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('E2E'), title });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** 打开看板（八列恒定存在，空列也有列头），先定视口再进页面（初始滚动按当时宽度算）。 */
  async function openBoard(page: Page, width: number) {
    await page.setViewportSize({ width, height: 900 });
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    await expect(page.locator('section.col').first()).toBeVisible({ timeout: 60_000 });
    return bundle;
  }

  /** 看板本体 = 那条横滚容器（`overflow-x: auto` 的盒子）。 */
  const board = (page: Page) => page.locator('.board');

  /** 列 = `BoardColumn` 的 `<section id="s-<key>">`（移动款跳段也用这个 id）。 */
  const col = (page: Page, key: string) => page.locator(`#s-${key}`);

  /** 横滚容器的当前滚动量与可视宽度。 */
  async function scrollState(page: Page) {
    return board(page).evaluate((el) => ({
      left: el.scrollLeft,
      max: Math.max(0, el.scrollWidth - el.clientWidth),
      viewLeft: el.getBoundingClientRect().left,
      viewRight: el.getBoundingClientRect().right,
      viewWidth: el.clientWidth,
    }));
  }

  /** 右缘之外**完整不可见**的列数：钉右档的边界是钉缝（`merge` 的左缘），窄档是横滚区右缘。 */
  async function hiddenColumns(page: Page) {
    return page.evaluate(() => {
      const view = document.querySelector('.board') as HTMLElement;
      const merge = document.getElementById('s-merge') as HTMLElement;
      const sticky = getComputedStyle(merge).position === 'sticky';
      const boundary = sticky
        ? merge.getBoundingClientRect().left
        : view.getBoundingClientRect().right;
      const keys = [
        'init',
        'architect-design',
        'design',
        'develop',
        'review',
        'test',
        'merge',
        'done',
      ];
      return keys.filter((k) => {
        if (sticky && (k === 'merge' || k === 'done')) return false; // 钉住的两列压在右缘上
        const el = document.getElementById(`s-${k}`) as HTMLElement;
        return el.getBoundingClientRect().left >= boundary - 0.5;
      }).length;
    });
  }

  /** 根元素上的计算 token 值（浏览器返回小写，统一小写比较）。 */
  async function rootToken(page: Page, name: string) {
    return page.evaluate(
      (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim().toLowerCase(),
      name,
    );
  }

  /** `#RGB` / `#RRGGBB` → `rgb(r, g, b)`（与计算值比较用）。 */
  function hexToRgb(hex: string) {
    const h = hex.replace('#', '');
    const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
    const n = Number.parseInt(full, 16);
    return `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
  }

  /** 元素在视口里完整可见。 */
  function fullyInside(box: { x: number; y: number; width: number; height: number } | null, width: number) {
    if (!box) return false;
    return box.x >= -1 && box.x + box.width <= width + 1;
  }

  test('宽屏：merge / done 钉在右侧不参与横滚，钉缝是 2px --pane 竖线', async ({ page }) => {
    const bundle = await openBoard(page, 1440);
    await expect(board(page)).toHaveCSS('overflow-x', 'auto');

    const merge = col(page, 'merge');
    const done = col(page, 'done');
    await expect(merge).toBeVisible();
    await expect(done).toBeVisible();

    // 打开时两列就在视口里（1440 屏上「谁在等我拍板」不必先拖六列）
    const boxStart = await merge.boundingBox();
    const doneStart = await done.boundingBox();
    expect(fullyInside(boxStart, 1440)).toBe(true);
    expect(fullyInside(doneStart, 1440)).toBe(true);

    // 钉住区宽 = 两列（列宽同宽；钉缝是 merge 自己的左框线，已算在它盒子里）
    expect(doneStart?.width).toBeCloseTo(boxStart?.width ?? 0, 0);
    expect(boxStart?.width ?? 0).toBeGreaterThan(0);
    expect((boxStart?.width ?? 0) + (doneStart?.width ?? 0)).toBeCloseTo(
      (boxStart?.width ?? 0) * 2,
      0,
    );
    // 阵列没被压窄 / 撑开：列与列仍逐列相接（冻结原型是一排 264 边框盒）
    const initBox = await col(page, 'init').boundingBox();
    const nextBox = await col(page, 'architect-design').boundingBox();
    expect(Math.abs((initBox?.x ?? 0) + (initBox?.width ?? 0) - (nextBox?.x ?? 0))).toBeLessThanOrEqual(1);
    expect(Math.abs((boxStart?.x ?? 0) + (boxStart?.width ?? 0) - (doneStart?.x ?? 0))).toBeLessThanOrEqual(1);
    await expect(merge).toHaveCSS('position', 'sticky');
    await expect(done).toHaveCSS('position', 'sticky');

    // 钉缝 = 钉住区左缘的 2px `--pane` 竖线（merge 列的左框线，就在钉住区的左缘上）
    const pane = await rootToken(page, '--pane');
    await expect(merge).toHaveCSS('border-left-width', '2px');
    await expect(merge).toHaveCSS('border-left-color', hexToRgb(pane));

    // 横滚一段（仍在钉住区间内）：中间六列被带走，钉住的两列纹丝不动（这才叫「不参与横滚」）
    const initStart = await col(page, 'init').boundingBox();
    const before = await scrollState(page);
    const step = Math.min(400, before.max);
    await board(page).evaluate((el, delta) => {
      el.scrollLeft = delta;
    }, step);
    await expect.poll(async () => (await scrollState(page)).left).toBeGreaterThan(before.left);
    const initEnd = await col(page, 'init').boundingBox();
    const mergeEnd = await merge.boundingBox();
    expect(Math.abs((initEnd?.x ?? 0) - (initStart?.x ?? 0))).toBeGreaterThan(100);
    expect(Math.abs((mergeEnd?.x ?? 0) - (boxStart?.x ?? 0))).toBeLessThanOrEqual(1);

    expectBundleHealthy(bundle);
  });

  test('窄档：打开时的滚动位置落在 merge（贴横滚区右缘），done 留在右缘之外', async ({ page }) => {
    const bundle = await openBoard(page, 1024);

    // 期望值由真实几何算出（不重抄坐标）：merge 右缘 − 横滚区宽，钳制在 [0, max]
    const view = await scrollState(page);
    const mergeRight = await page.evaluate(() => {
      const el = document.querySelector('.board') as HTMLElement;
      const merge = document.getElementById('s-merge') as HTMLElement;
      return (
        merge.getBoundingClientRect().right - el.getBoundingClientRect().left + el.scrollLeft
      );
    });
    const expected = Math.min(Math.max(mergeRight - view.viewWidth, 0), view.max);
    expect(Math.abs(view.left - expected)).toBeLessThanOrEqual(1);

    // merge 贴住横滚区右缘（「落在 merge」的可实现读法），done 仍在右缘之外
    const mergeBox = await col(page, 'merge').boundingBox();
    const doneBox = await col(page, 'done').boundingBox();
    expect(Math.abs((mergeBox?.x ?? 0) + (mergeBox?.width ?? 0) - view.viewRight)).toBeLessThanOrEqual(1);
    expect(doneBox?.x ?? 0).toBeGreaterThanOrEqual(view.viewRight - 1);

    expectBundleHealthy(bundle);
  });

  test('右缘「还有 N 列」指示：在右缘、不吃指针事件，n 归零后整条不渲染', async ({ page }) => {
    const bundle = await openBoard(page, 1440);

    const more = page.getByText(/还有\s*\d+\s*列/);
    await expect(more).toBeVisible();
    // 数字 = 右缘之外完整不可见的列数（独立量一遍，不写死）
    const n = await hiddenColumns(page);
    expect(n).toBeGreaterThan(0);
    await expect(more).toHaveText(new RegExp(`还有\\s*${n}\\s*列`));

    // 形态：2px --pane 描边 + wash 底 + 12px 字；它是指示不是控件
    await expect(more).toHaveCSS('border-top-width', '2px');
    await expect(more).toHaveCSS('border-radius', '0px');
    await expect(more).toHaveCSS('font-size', '12px');
    await expect(more).toHaveCSS('color', hexToRgb(await rootToken(page, '--text-3')));
    await expect(more).toHaveCSS('background-color', hexToRgb(await rootToken(page, '--wash')));
    await expect(more).toHaveCSS('pointer-events', 'none');
    // 它是指示不是控件：对读屏隐藏（整枚指示都在 aria-hidden 的锚点子树里）
    await expect(page.locator('.more-anchor')).toHaveAttribute('aria-hidden', 'true');

    // 纵向压在脊线带底部空带与阵列顶框之间：不覆盖任何列头 / 计数
    const moreBox = (await more.boundingBox())!;
    const headBox = (await page.locator('.col-head').first().boundingBox())!;
    expect(moreBox.y + moreBox.height).toBeLessThanOrEqual(headBox.y + 1);

    // 钉右档：指示紧贴钉缝左侧（钉住区不放它进去；正文与钉缝之间留 2px，别叠成粗线）
    const seamX = (await col(page, 'merge').boundingBox())!.x;
    expect(moreBox.x + moreBox.width).toBeLessThanOrEqual(seamX);
    expect(moreBox.x + moreBox.width).toBeGreaterThanOrEqual(seamX - 4);

    // 滚到最右就没有「还有列」这回事了：整条不渲染
    await board(page).evaluate((el) => {
      el.scrollLeft = el.scrollWidth;
    });
    await expect(more).toHaveCount(0);

    // 窄档同一枚：边界挪到横滚区右缘，`done` 被数进去
    await page.setViewportSize({ width: 1024, height: 900 });
    await board(page).evaluate((el) => {
      el.scrollLeft = 0;
    });
    await expect(more).toBeVisible();
    const nNarrow = await hiddenColumns(page);
    expect(nNarrow).toBeGreaterThan(0);
    await expect(more).toHaveText(new RegExp(`还有\\s*${nNarrow}\\s*列`));

    expectBundleHealthy(bundle);
  });

  test('脊线数字读作「累计 N」；列头数字不带口径词，并行侧站没有数字', async ({ page }) => {
    const bundle = await openBoard(page, 1440);

    const ct = page.locator('.rail.spine .stn .ct').first();
    await expect(ct).toBeVisible();
    // 框内 = 口径词 + 数字（常显的标注词是主手段，title 只是辅助）
    await expect(ct).toHaveText(/^累计\s*\d+$/);
    await expect(ct).toHaveAttribute('title', /累计/);
    // 形态不变：2px --pane 描边、圆角 0、内边距 0 4px、12px 字
    await expect(ct).toHaveCSS('border-top-width', '2px');
    await expect(ct).toHaveCSS('border-radius', '0px');
    await expect(ct).toHaveCSS('padding-left', '4px');
    await expect(ct).toHaveCSS('padding-right', '4px');
    await expect(ct).toHaveCSS('font-size', '12px');

    // 并行两个侧站没有数字（整块不渲染，不把「没有」画成 0）
    await expect(page.locator('.rail.spine .stn.side .ct')).toHaveCount(0);
    await expect(page.locator('.rail.spine .stn.side .lb')).toHaveCount(2);

    // 列头数字不带词（带词两处又长得一样了，且列头行没有富余宽度）
    const colN = page.locator('.col-head .col-n').first();
    await expect(colN).toBeVisible();
    await expect(colN).toHaveText(/^\d+$/);

    expectBundleHealthy(bundle);
  });

  test('票 10：并行两个侧站名标签带背景遮罩与 4px 横向内边距', async ({ page }) => {
    const bundle = await openBoard(page, 1440);

    const labels = page.locator('.rail.spine .stn.side .lb');
    await expect(labels).toHaveCount(2);
    for (const label of await labels.all()) {
      await expect(label).toBeVisible();
      // 原型 `.stn.side .lb{padding:0 4px}`：实现漏抄的正是这两条边距（少 8px、链节从字两端透出）
      await expect(label).toHaveCSS('padding-left', '4px');
      await expect(label).toHaveCSS('padding-right', '4px');
      // 遮罩是不透明的 --bg（否则链节透出来）
      const bg = await label.evaluate((el) => getComputedStyle(el).backgroundColor);
      expect(bg).toBe(hexToRgb(await rootToken(page, '--bg')));
      // 盒宽确实比文字宽 8px（量文字自身，不靠内边距声明自证）
      const widths = await label.evaluate((el) => {
        const range = document.createRange();
        range.selectNodeContents(el);
        return {
          box: el.getBoundingClientRect().width,
          text: range.getBoundingClientRect().width,
        };
      });
      expect(widths.box - widths.text).toBeGreaterThanOrEqual(7);
    }

    expectBundleHealthy(bundle);
  });

  test('移动款：纵向站点带不受影响（横向脊线隐藏、阵列纵向堆叠）', async ({ page }) => {
    const bundle = await openBoard(page, 430);

    // 横向脊线在移动款里是隐藏的（那里是纵向站点带）。钉右档把脊线切成两段，
    // 故 `.rail.spine` 不止一个元素——逐条核，别用会撞严格模式的单数定位。
    const spines = page.locator('.rail.spine');
    const spineCount = await spines.count();
    expect(spineCount, '横向脊线至少一条').toBeGreaterThan(0);
    for (let i = 0; i < spineCount; i += 1) {
      await expect(spines.nth(i)).toBeHidden();
    }
    // 阵列纵向排列：两列同起点、等宽、后一列在下一列下方
    const first = (await page.locator('section.col').first().boundingBox())!;
    const second = (await page.locator('section.col').nth(1).boundingBox())!;
    expect(second.y).toBeGreaterThan(first.y);
    expect(Math.abs(first.x - second.x)).toBeLessThanOrEqual(1);
    expect(Math.abs(first.width - second.width)).toBeLessThanOrEqual(1);
    // 移动款自己的那条纵向链节脊线还在
    await expect(page.locator('.spine-rule').first()).toBeVisible();

    expectBundleHealthy(bundle);
  });
});
