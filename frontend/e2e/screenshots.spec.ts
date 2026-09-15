/**
 * 真应用截图（票 12 / 决策 169）——**证据，不是闸门**。
 *
 * 在既有 harness（真后端 + 同源内嵌产物 + 临时 home，决策 151/155/166）上驱动真应用，
 * 覆盖 6 个路由 × 深浅 + 移动款关键视图，产出可重新生成的截图供评审对照原型
 * （原型那边是 7 个视图——它在 `detail` 上拆了 run / approve 两屏；真应用 `detail` 只有一个
 * 路由，两种外观由任务状态决定，故路由数是 6）
 * （design/prototype-pixel*.html）。
 *
 * **刻意不做字节级 golden 回放**：像素字体跨机渲染存在差异，那会引入 flaky 门。
 * 截图是给人看的证据，断言留给 `pixel-theme.spec.ts`。
 *
 * 用法（仓库根）：
 *   cd frontend && AGENTPIPELINE_SHOTS=1 npx playwright test --project=chromium e2e/screenshots.spec.ts
 * 产物：`.scratch/shots/app/*.png`（该目录在 .gitignore 里）。
 *
 * **默认 skip**：截图是证据不是闸门，不该拖慢 `make check-e2e`；设 `AGENTPIPELINE_SHOTS=1`
 * 才跑（决策 169「截图是证据不是门」）。
 *
 * 只 Chromium（决策 144）。
 */

import { mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from '@playwright/test';
import { startApp, settleBundle, waitForTask, watchBundle, type App } from './harness';
import { fullPassScript } from './scripts';

// 证据而非闸门：未显式开启时整组跳过（见文件头说明）。
test.skip(process.env.AGENTPIPELINE_SHOTS !== '1', '截图是证据不是门；设 AGENTPIPELINE_SHOTS=1 才跑');

const here = dirname(fileURLToPath(import.meta.url));
const outDir = resolve(here, '..', '..', '.scratch', 'shots', 'app');

/** 七个路由（`frontend-design.md` §4 + 决策 167 的 `#/share` + 决策 174 的 `#/talk`）。 */
const ROUTES: Array<{ slug: string; hash: string }> = [
  { slug: 'board', hash: '#/' },
  { slug: 'talk', hash: '#/talk' },
  { slug: 'detail', hash: '#/task/__TASK__' },
  { slug: 'metrics', hash: '#/metrics' },
  { slug: 'projects', hash: '#/settings/projects' },
  { slug: 'providers', hash: '#/settings/providers' },
  { slug: 'share', hash: '#/share' },
];

test.describe('真应用截图（证据，非闸门）', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({ script: fullPassScript('E2E'), title: '截图任务' });
    // 等任务离开 init，让看板/详情有内容可截
    await waitForTask(app, (t) => t.status !== 'queued' && t.status !== 'waiting', 'started', 60_000)
      .catch(() => undefined);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('桌面 7 路由 × 深浅', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });

    for (const theme of ['dark', 'light'] as const) {
      for (const route of ROUTES) {
        await page.goto(`${app.webBase}/${route.hash.replace('__TASK__', app.taskId)}`);
        await settleBundle(page, bundle);
        // 换主题后重渲染（截图脚本不走状态行按钮，直接改 dataset 更稳）
        await page.evaluate((t) => {
          document.documentElement.dataset.theme = t;
          localStorage.setItem('agentpipeline.theme', t);
        }, theme);
        await page.waitForTimeout(600);
        await page.screenshot({
          path: resolve(outDir, `${theme}-${route.slug}.png`),
          fullPage: true,
        });
      }
    }

    await page.evaluate(() => {
      document.documentElement.dataset.theme = 'dark';
      localStorage.setItem('agentpipeline.theme', 'dark');
    });
  });

  test('移动款关键视图 × 深浅', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 932 });

    for (const theme of ['dark', 'light'] as const) {
      for (const route of [
        { slug: 'board', hash: '#/' },
        { slug: 'detail', hash: `#/task/${app.taskId}` },
        { slug: 'metrics', hash: '#/metrics' },
      ]) {
        await page.goto(`${app.webBase}/${route.hash}`);
        await settleBundle(page, bundle);
        await page.evaluate((t) => {
          document.documentElement.dataset.theme = t;
          localStorage.setItem('agentpipeline.theme', t);
        }, theme);
        await page.waitForTimeout(600);
        await page.screenshot({
          path: resolve(outDir, `mobile-${theme}-${route.slug}.png`),
          fullPage: true,
        });
      }
    }

    await page.evaluate(() => {
      document.documentElement.dataset.theme = 'dark';
      localStorage.setItem('agentpipeline.theme', 'dark');
    });
  });
});
