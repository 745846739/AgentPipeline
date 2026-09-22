/**
 * UX 第二轮 · 票 04 / 06（R2-19 / R2-20 / R2-04）：地标、标题层级、页签、当前项、
 * 可访问名里的装饰、每页标题——以及「待处理」下拉那份播报是不是真的。
 *
 * 这一组是**逐路由的 DOM 事实枚举**：`<main>`、`<h1>`、标题层级、`tablist`、
 * `aria-current`、`document.title`。断言落在结构与可访问性契约上，不落 class 名。
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

/** 逐路由的结构事实。`nav` = 这一条路由上顶栏该点亮哪一项（没有就是 null）。 */
const ROUTES: Array<{ hash: string; name: string; nav: string | null }> = [
  { hash: '#/', name: '看板', nav: '看板' },
  { hash: '#/talk', name: '对讲台', nav: '对讲台' },
  { hash: '#/metrics', name: '指标', nav: '指标' },
  { hash: '#/settings', name: '设置落地页', nav: '设置' },
  { hash: '#/settings/projects', name: '设置 · 项目', nav: '设置' },
  { hash: '#/settings/providers', name: '设置 · 模型与密钥', nav: '设置' },
  { hash: '#/settings/stages', name: '设置 · 阶段配置', nav: '设置' },
  { hash: '#/settings/market', name: '设置 · 技能市场', nav: '设置' },
  { hash: '#/share', name: '手机访问', nav: '设置' },
  { hash: '#/nope', name: '404', nav: null },
];

test.describe('UX2 ⑤ 全站语义与待处理下拉（票 04 / 06）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('UX2S'), title: 'UX2 语义' });
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval', 180_000);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('逐路由：main / h1 / 当前项 / 每页标题各不相同', async ({ page }) => {
    const bundle = watchBundle(page);
    const seenTitles: string[] = [];

    for (const route of [...ROUTES, { hash: `#/task/${app.taskId}`, name: '任务详情', nav: null }]) {
      await page.goto(`${app.webBase}/${route.hash}`);
      await settleBundle(page, bundle);
      await page.waitForTimeout(250);

      const facts = await page.evaluate(() => {
        const headings = Array.from(document.querySelectorAll('h1,h2,h3,h4,h5,h6')).map((h) =>
          Number(h.tagName[1]),
        );
        // 层级逐级递进：下一条不能比上一条深超过 1 级（h1 → h4 就是断级）
        const skips: string[] = [];
        for (let i = 1; i < headings.length; i += 1) {
          if (headings[i] > headings[i - 1] + 1) skips.push(`h${headings[i - 1]} → h${headings[i]}`);
        }
        return {
          main: document.querySelectorAll('main').length,
          h1: document.querySelectorAll('h1').length,
          h1Text: (document.querySelector('h1')?.textContent ?? '').trim(),
          skips,
          navCurrent: Array.from(document.querySelectorAll('nav[aria-label="页面导航"] a')).map((a) =>
            a.getAttribute('aria-current') === 'page' ? (a.textContent ?? '').trim() : null,
          ),
          title: document.title,
        };
      });

      seenTitles.push(facts.title);
      expect(facts.main, `${route.name}：应当恰有一个 <main>`).toBe(1);
      expect(facts.h1, `${route.name}：应当恰有一个 <h1>`).toBe(1);
      expect(facts.h1Text.length, `${route.name}：<h1> 不能是空的`).toBeGreaterThan(0);
      expect(facts.skips, `${route.name}：标题层级断级`).toEqual([]);

      const current = facts.navCurrent.filter((x): x is string => x !== null);
      expect(current, `${route.name}：顶栏当前项的判定`).toEqual(route.nav ? [route.nav] : []);
    }

    // 每页一个标题：11 条路由不许再共用一个
    expect(new Set(seenTitles).size, `标题应当逐页不同，实际：${seenTitles.join(' | ')}`).toBe(
      seenTitles.length,
    );
    expectBundleHealthy(bundle);
  });

  test('任务详情：页签是真 tablist，方向键能切，Diff 的文件块不再跳级', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);

    const tablist = page.getByRole('tablist', { name: '任务详情页签' });
    await expect(tablist).toBeVisible();
    const timeline = page.getByRole('tab', { name: '时间线' });
    await expect(timeline).toHaveAttribute('aria-selected', 'true');
    await expect(timeline).toHaveAttribute('aria-controls', 'detail-pane');
    await expect(page.getByRole('tabpanel')).toHaveCount(1);

    // 方向键：焦点跟着选中的页签走
    await timeline.focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('tab', { name: '会话' })).toHaveAttribute('aria-selected', 'true');
    await page.keyboard.press('End');
    await expect(page.getByRole('tab', { name: /^Diff$/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );

    // 标题层级：h1（任务名）→ h2（面板）→ h3（diff 的每个文件）
    const levels = await page.evaluate(() =>
      Array.from(document.querySelectorAll('h1,h2,h3,h4,h5,h6')).map((h) => Number(h.tagName[1])),
    );
    expect(levels[0]).toBe(1);
    expect(levels).toContain(2);
    expect(levels).not.toContain(4);
    await expect(page.getByRole('heading', { level: 3 }).first()).toBeVisible({ timeout: 30_000 });
    expectBundleHealthy(bundle);
  });

  test('拍板按钮的可访问名里没有装饰的 ▶', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);

    // 精确匹配能命中 = 名字就是「合入」两个字（读屏念的与眼睛看的不是一回事）
    await expect(page.getByRole('button', { name: '合入', exact: true })).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.getByRole('button', { name: '返回修改', exact: true })).toBeVisible();
    expectBundleHealthy(bundle);
  });

  test('待处理下拉：Escape / 点外关得掉，方向键进得去，项是链接', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);

    const chip = page.getByRole('button', { name: /^待处理 \d+$/ });
    const panel = page.locator('#pending-dropdown');
    await expect(chip).toHaveAttribute('aria-controls', 'pending-dropdown');
    await expect(chip).toHaveAttribute('aria-expanded', 'false');
    await expect(panel).toBeHidden();

    // 打开 → Escape 关得掉（审计里这一条是「仍然开着」）
    await chip.click();
    await expect(chip).toHaveAttribute('aria-expanded', 'true');
    await expect(panel).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(chip).toHaveAttribute('aria-expanded', 'false');
    await expect(panel).toBeHidden();

    // 关掉之后焦点回到触发钮
    await expect(chip).toBeFocused();

    // 再打开 → 点面板外面关得掉
    await chip.click();
    await expect(panel).toBeVisible();
    await page.mouse.click(20, 400);
    await expect(panel).toBeHidden();

    // 再打开 → ArrowDown 把焦点送进第一项（每一项是跳任务详情的链接）
    await chip.click();
    await chip.focus();
    await page.keyboard.press('ArrowDown');
    const firstItem = panel.getByRole('link').first();
    await expect(firstItem).toBeFocused();
    await expect(firstItem).toHaveAttribute('href', new RegExp(`^#/task/`));

    // 点进去真的到任务详情，且面板随之关闭
    const href = await firstItem.getAttribute('href');
    await firstItem.click();
    await expect(page).toHaveURL(new RegExp(`${href?.replace('#', '\\#')}$`));
    await expect(panel).toBeHidden();
    expectBundleHealthy(bundle);
  });
});
