/**
 * 前端 E2E：指标页（UX 审计票 06 / 12 / 13 / 23 / 27）。
 *
 * 断言口径（整备规格 Testing Decisions）：只测**用户看得见的东西**——
 *   · 带 `?task=<id>` 打开指标页时那个任务的指标**自动载入并高亮**（用户没动手）；
 *   · 第一段说的是人话：四个量各自是什么 + 怎么算，没有原始字段名、没有内部编号；
 *   · 分母为 0 的比值给的是**为什么没有意义**，而不是一条没有解释的横线；
 *   · 一个任务都没有时的空态有「状态 → 下一步」，提到的另一个页面可点；
 *   · 同一页里琥珀（`--pending`）不再挂在纯刻度的重试率图元上（票 12）。
 *
 * 文案不做精确匹配（只有 `transmission` 这类契约性字符串例外——这里是隐喻首现的定稿译文与
 * 数据驱动的阶段名）；说明性段落只做「不含某类术语」的**否定**断言。
 * 真 axum 后端 + 内嵌产物 + mock LLM；只 Chromium（决策 144）。
 */

import { expect, test, type Locator, type Page } from '@playwright/test';

import {
  expectBundleHealthy,
  settleBundle,
  startApp,
  waitForTask,
  watchBundle,
  type App,
} from './harness';
import { NODE, designRounds, fullPassScript, tool } from './scripts';

/* ── 计算样式（照 pixel-theme / settings-empty-and-copy 的口径：读浏览器**算出**的值） ── */

/** 根元素上的 token 取值。 */
async function rootToken(page: Page, name: string): Promise<string> {
  return page.evaluate(
    (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim().toLowerCase(),
    name,
  );
}

/** `#rgb` / `#rrggbb` / `rgb()` / `rgba()` → `[r, g, b]`；其它形态直接报错，不猜。 */
function toRgb(value: string): [number, number, number] {
  const v = value.trim().toLowerCase();
  if (v.startsWith('#')) {
    const h = v.slice(1);
    const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h.slice(0, 6);
    const n = Number.parseInt(full, 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  const m = /rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)/.exec(v);
  if (!m) throw new Error(`读不出颜色：${value}`);
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

/** 与浏览器归一化后的计算值同形（`rgb(r, g, b)`），便于直接比较。 */
function rgbString(value: string): string {
  const [r, g, b] = toRgb(value);
  return `rgb(${r}, ${g}, ${b})`;
}

/** 元素的计算文字色。 */
function colorOf(locator: Locator): Promise<string> {
  return locator.evaluate((el) => getComputedStyle(el).color);
}

/**
 * 打开指标页并等它真的画出来。
 *
 * 全是同文档的片段导航（hash 路由），故 `load` 事件不会再来一次——用页面标题当锚，
 * 免得断言落在上一页还没换掉的 DOM 上。
 */
async function openMetrics(page: Page, app: App, hash: string): Promise<void> {
  const bundle = watchBundle(page);
  await page.goto(`${app.webBase}/${hash}`);
  await settleBundle(page, bundle);
  await expect(page.locator('.page h1.p-title')).toContainText('全局指标');
  expectBundleHealthy(bundle);
}

/* ─────────── ① 全流程在跑：深链自动载入并高亮 + 第一段说人话 ─────────── */

test.describe('前端 E2E：指标页的深链入口与说人话的第一段（票 06 / 23 / 25 / 27）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('E2E'), title: '指标页 E2E 任务' });
    // 等任务离开队列（有运行行之后，任务级与全局两类指标都有内容可看）
    await waitForTask(app, (t) => t.status !== 'queued', 'started', 60_000).catch(() => undefined);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('带 ?task=<id> 打开：那个任务的指标自己就位并高亮，不需要用户动手（票 06）', async ({
    page,
  }) => {
    await openMetrics(page, app, `#/metrics?task=${app.taskId}`);

    const taskBox = page.locator('section.task');
    await expect(taskBox).toBeVisible({ timeout: 30_000 });
    // 指标自己就位了——用户没有填过 ID、也没有点过「载入」
    await expect(taskBox).toContainText(/个 token/);
    // 页头点出这是哪个任务的指标，输入框也据链接填好（手输只是兜底，不是唯一的路）
    await expect(taskBox).toContainText(app.taskId);
    await expect(taskBox.locator('input')).toHaveValue(app.taskId);
    // 没有报错、也不是「请填写任务 ID」那种原地踏步
    await expect(taskBox.locator('.blank.error')).toHaveCount(0);

    // 高亮是**算出来的**：这一份带 wash 底（同一页里别的图不带）
    const box = await taskBox.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(box).toBe(rgbString(await rootToken(page, '--wash')));
    const other = await page
      .locator('section.chart.panel:not(.task)')
      .first()
      .evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(other).not.toBe(box);
  });

  test('第一段说人话：四个量都解释、没有原始字段名与内部编号（票 27 / 23）', async ({ page }) => {
    await openMetrics(page, app, '#/metrics');

    const lead = page.locator('.hintline').first();
    await expect(lead).toBeVisible();
    const text = await lead.innerText();
    for (const term of ['成功率', '重试率', '逃逸事件', '首过率']) {
      expect(text, `第一段没有解释「${term}」`).toContain(term);
    }
    for (const jargon of [
      'total_tokens',
      'total_calls',
      'calls',
      'done ÷',
      'validate_output',
      'attempt',
      'kickback',
      'core metrics',
      '决策',
    ]) {
      expect(text, `第一段里还有实现术语 / 内部编号：${jargon}`).not.toContain(jargon);
    }

    // 灰字档位：次级必读（--text-3），不是纯装饰的 --text-4；深浅两套都不漏
    for (const theme of ['dark', 'light'] as const) {
      await page.evaluate((t) => {
        document.documentElement.dataset.theme = t;
        localStorage.setItem('agentpipeline.theme', t);
      }, theme);
      await page.waitForTimeout(300);
      const color = await colorOf(lead);
      expect(color, `${theme} 款下第一段用的是装饰档`).toBe(
        rgbString(await rootToken(page, '--text-3')),
      );
      expect(color).not.toBe(rgbString(await rootToken(page, '--text-4')));
    }
  });

  test('整页正文里没有内部编号；页脚的排除说明说人话并带隐喻首现译文（票 23 / 25 / 27）', async ({
    page,
  }) => {
    await openMetrics(page, app, '#/metrics');

    // 页脚那句：sync-check 不在轨道阶段里，跑起来之后会被列出来
    const excl = page.locator('.excl');
    await expect(excl).toBeVisible({ timeout: 60_000 });
    await expect(excl).toContainText('sync-check');
    // 隐喻首现的定稿说法（决策 200 词表）就在这一句里，逐字
    await expect(excl).toContainText('传送带（这条流水线的顺序）');
    await expect(excl).not.toContainText('决策');

    // 面向用户的正文里不再出现「决策 N」（兜底机器门在 src/lib/copy-discipline.test.ts，
    // 这里在真应用上再核一遍屏幕上真的读得到什么）
    const pageText = await page.locator('.page').innerText();
    expect(pageText).not.toMatch(/决策\s*[0-9０-９]/);
    expect(pageText).not.toContain('（决策');
  });
});

/* ─────────── ② 分母为 0：比值给的是「为什么没有意义」 ─────────── */

test.describe('前端 E2E：指标页的未知比值与琥珀收敛（票 12 / 27）', () => {
  let app: App;

  test.beforeAll(async () => {
    // 一个**跑不完**的任务：停在 develop 的 sleep 上，永远不进终态。
    // 于是一个任务都没有跑完 → 成功率的分母是 0（正是审计截图里那条没有解释的横线）。
    app = await startApp({
      script: { ...designRounds(), [NODE.developEx]: [[tool('run_command', { command: 'sleep 120' })]] },
      title: '跑不完的任务',
    });
    // 等它真的走到 develop（设计三段与 sync-check 都跑过了，各阶段图上才有内容）
    await waitForTask(app, (t) => t.current_stage === 'develop', 'started', 60_000)
      .catch(() => undefined);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('成功率的分母是 0：给的是一句解释，不是一条光秃秃的横线（票 27）', async ({ page }) => {
    await openMetrics(page, app, '#/metrics');

    // 这一块里没有任何一条「跑完了」的任务，所以成功率算不出来
    const success = page.locator('.chart').filter({ hasText: '成功率' }).first();
    await expect(success).toBeVisible();
    const text = (await success.innerText()).trim();
    expect(text, `成功率那一块只剩一条没有解释的横线：${text}`).not.toContain('—');
    expect(text.length, `成功率那一块没有解释：${text}`).toBeGreaterThan(20);

    // 有值的比值照常显示数值与单位（各阶段重试率这一块有记录）
    const retry = page.locator('.chart').filter({ hasText: '各阶段重试率' }).first();
    await expect(retry).toBeVisible();
    await expect(retry).toContainText(/%/);
  });

  test('琥珀在重试率图上随值取：0 值的中性档不带琥珀（票 12 / 决策 203）', async ({ page }) => {
    await openMetrics(page, app, '#/metrics');

    const retry = page.locator('.chart').filter({ hasText: '各阶段重试率' }).first();
    await expect(retry).toBeVisible();
    const pending = rgbString(await rootToken(page, '--pending'));

    // 逐列把**显示出来的值**与这一列的条 / 灯的计算配色配成对：值 0.0% 的列不得有琥珀
    //（决策 203 裁决 ②：0 值走中性材质档，有值才取 `caution`——「整图恒取 caution」正是
    // 这次要修掉的那个读法）。灯与条同色是硬要求，两处一起核。
    const cols = await retry.locator('.col').evaluateAll((els) =>
      els.map((el) => {
        const paint = (sel: string) => {
          const node = el.querySelector(sel);
          if (!node) return '';
          const s = getComputedStyle(node);
          return `${s.backgroundColor}|${s.borderColor}`;
        };
        return {
          display: el.querySelector('.val')?.textContent?.trim() ?? '',
          tone: `${el.querySelector('.mdot')?.className ?? ''}`,
          paints: [paint('.mdot'), paint('.fill')],
        };
      }),
    );
    expect(cols.length).toBeGreaterThan(0);
    expect(cols.every((c) => c.display.length > 0), '每列都该带数值').toBe(true);

    const zeroCols = cols.filter((c) => /^0(\.0+)?%$/.test(c.display));
    expect(zeroCols.length, `这张图里应当有 0.0% 的列：${JSON.stringify(cols)}`).toBeGreaterThan(0);
    for (const col of zeroCols) {
      for (const paint of col.paints) {
        expect(paint, `0 值的那一列还带着琥珀（${col.display}）：${paint}`).not.toContain(pending);
      }
      expect(col.tone, `0 值的那一列还带着 caution 档：${col.tone}`).not.toContain('caution');
    }
    // 反过来：真有重试的列**才**配得上这个颜色（有值才取）
    for (const col of cols.filter((c) => c.display !== '0.0%' && !/^0(\.0+)?%$/.test(c.display))) {
      expect(col.tone, `有值的列没取告警档（${col.display}）：${col.tone}`).toContain('caution');
    }
  });
});

/* ─────────── ③ 一个任务都没有：空态（票 13） ─────────── */

test.describe('前端 E2E：指标页的空态（票 13）', () => {
  let app: App;

  test.beforeAll(async () => {
    // 首启（空 home）：连项目都没有，指标页没有任何可统计的东西
    app = await startApp({ script: fullPassScript('E2E'), seedless: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('空态说清「现在是空的 + 下一步做什么」，入口可点、点了能到（票 13）', async ({ page }) => {
    await openMetrics(page, app, '#/metrics');

    // 空态的形状来自共享组件 components/ui/EmptyState.svelte。这里**只按用户看得见的文字**
    // 定位，不按它的 class（spec 的 Testing Decisions：断言不落 class 名与组件内部形状）。
    await expect(page.getByText('现在还没有任务，所以没有可统计的东西。')).toBeVisible();
    await expect(page.getByText(/先去看板新建一个任务/)).toBeVisible();

    // 下一步提到的另一个页面**必须可点**
    const link = page.getByRole('link', { name: '去看板新建任务' }).first();
    await expect(link).toBeVisible();
    await link.click();
    await expect(page.locator('.board')).toBeVisible();
    await expect(page).toHaveURL(/#\/$/);
  });

  test('空态文字用的是可读档（--text-3）而不是装饰档，深浅两套都不漏（票 13 / §三.1）', async ({
    page,
  }) => {
    await openMetrics(page, app, '#/metrics');

    for (const theme of ['dark', 'light'] as const) {
      await page.evaluate((t) => {
        document.documentElement.dataset.theme = t;
        localStorage.setItem('agentpipeline.theme', t);
      }, theme);
      await page.waitForTimeout(300);

      const next = page.getByText(/先去看板新建一个任务/).first();
      await expect(next).toBeVisible();
      const color = await colorOf(next);
      expect(color, `${theme} 款下空态用的是装饰档`).toBe(
        rgbString(await rootToken(page, '--text-3')),
      );
      expect(color).not.toBe(rgbString(await rootToken(page, '--text-4')));
    }
  });
});
