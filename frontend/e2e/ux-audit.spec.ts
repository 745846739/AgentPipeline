/**
 * UX 审计截图（证据，非闸门）。
 *
 * 与 `screenshots.spec.ts` 的分工：那条只出「每个路由一张」的基线；这一条出**交互态**
 * ——空库、过滤、下拉、对话框、pending 档案盒、对讲台多急停、市场列表、手机访问两种
 * 绑定态。产物落 `.scratch/ux-audit/`，供人工审 UI/UX。
 *
 * 默认 skip：审计不是闸门，不该拖慢 `make check-e2e`。用法（仓库根）：
 *   cd frontend && UX_AUDIT=1 AGENTPIPELINE_E2E_BIN=target/release/agent-pipeline \
 *     npx playwright test --project=chromium e2e/ux-audit.spec.ts
 */

import { mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, type Page } from '@playwright/test';
import { startApp, waitForTaskById, watchBundle, type App } from './harness';
import { startGitRepo, type GitRepoFixture } from './gitRepo';
import {
  NODE,
  foremanScript,
  fullPassScript,
  readTask,
  siblingDesignRounds,
  siblingPassScript,
  text,
  tool,
} from './scripts';

test.skip(process.env.UX_AUDIT !== '1', '审计截图是证据不是门；设 UX_AUDIT=1 才跑');

const here = dirname(fileURLToPath(import.meta.url));
const outDir = resolve(here, '..', '..', '.scratch', 'ux-audit');

/** 任意合法形状的 id：只用来让值班长翻一次台账，回执行照样渲染。 */
const SOME_ID = '01JZZZZZZZZZZZZZZZZZZZZZZZ';

/** 一个**停在 develop** 的任务：`sleep` 让它在截图期间一直是 running。 */
const stalledScript = {
  ...siblingDesignRounds(),
  [NODE.developEx]: [[tool('run_command', { command: 'sleep 900' })]],
};

/** 值班长的回话：先翻两处台账（出「工位回执」），再给结论。 */
const foremanRounds = foremanScript([
  [readTask(SOME_ID), tool('read_conversation', { task_id: SOME_ID, run_id: null }), text('当前态势')],
  [text('三件货箱卡在合入前等拍板，两个工位还在跑。')],
  [text('合计 8 个工位：1 个在跑、3 个急停、其余空闲。')],
]);

async function shot(page: Page, name: string): Promise<void> {
  await page.screenshot({ path: resolve(outDir, `${name}.png`), fullPage: true });
}

/** 换主题后重渲染（与 screenshots.spec.ts 同姿态：直接改 dataset 更稳）。 */
async function setTheme(page: Page, theme: 'dark' | 'light'): Promise<void> {
  await page.evaluate((t) => {
    document.documentElement.dataset.theme = t;
    localStorage.setItem('agentpipeline.theme', t);
  }, theme);
  await page.waitForTimeout(350);
}

/** 进页面并等产物执行完（不能用 networkidle：SSE 常驻）。 */
async function open(page: Page, app: App, hash: string): Promise<void> {
  await page.goto(`${app.webBase}/${hash}`);
  await page.waitForLoadState('load');
  await page.waitForTimeout(700);
}

test.describe('① 有数据的库：看板 / 详情 / 对讲台 / 台账', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: { ...fullPassScript('UX-MAIN'), ...foremanRounds },
      title: '实现用户登录接口',
      additionalTasks: [
        { title: '把登录失败原因写进审计日志', script: siblingPassScript('UX-EXTRA1') },
        { title: '给导出命令加一个 --since 参数', script: stalledScript },
      ],
    });
    // 让看板先有「跑到一半」的样子：三件事分散在不同列
    await new Promise((r) => setTimeout(r, 7000));
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('看板：列 / 卡片 / 过滤 / 下拉 / 新建对话框', async ({ page }) => {
    const guard = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    await shot(page, 'board-dark');

    await setTheme(page, 'light');
    await shot(page, 'board-light');
    await setTheme(page, 'dark');

    // 过滤：已完成（第 6 格；顺序见 TopBar 的 FILTERS）
    await page.locator('.slots .slot').nth(5).click();
    await page.waitForTimeout(300);
    await shot(page, 'board-filter-done');
    await page.locator('.slots .slot').nth(0).click();
    await page.waitForTimeout(300);

    // 待处理下拉
    await page.locator('.pending-count').click();
    await page.waitForTimeout(300);
    await shot(page, 'board-pending-dropdown');
    await page.locator('.pending-count').click();
    await page.waitForTimeout(300);

    // 新建任务对话框
    await page.locator('.btn-new').click();
    await page.waitForTimeout(400);
    await shot(page, 'board-newtask-dialog');
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);

    // 窄屏
    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    await shot(page, 'mobile-board');
    void guard;
  });

  test('详情：pending（档案盒）与五个页签', async ({ page }) => {
    const guard = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    // 等主任务挂到 merge_approval —— 那才是档案盒最全的样子
    await waitForTaskById(
      app.taskIds[0],
      app,
      (t) => {
        const reason = t.pending_reason as { type?: string } | null | undefined;
        return reason?.type === 'merge_approval';
      },
      'merge_approval',
      180_000,
    ).catch(() => undefined);

    await open(page, app, `#/task/${app.taskIds[0]}`);
    await setTheme(page, 'dark');
    await shot(page, 'detail-pending-dossier');
    await setTheme(page, 'light');
    await shot(page, 'detail-pending-dossier-light');
    await setTheme(page, 'dark');

    for (const [slug, label] of [
      ['timeline', '时间线'],
      ['conversation', '会话'],
      ['commands', '命令与输出'],
      ['files', '产出文件'],
      ['diff', 'Diff'],
    ] as const) {
      const tab = page.getByRole('button', { name: new RegExp(label) });
      if (await tab.count()) {
        await tab.first().click();
        await page.waitForTimeout(1000);
        await shot(page, `detail-tab-${slug}`);
      }
    }
    void guard;
  });

  test('详情：在跑的任务（桌面 + 窄屏）', async ({ page }) => {
    const guard = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, `#/task/${app.taskIds[2]}`);
    await setTheme(page, 'dark');
    await shot(page, 'detail-running');

    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, `#/task/${app.taskIds[0]}`);
    await setTheme(page, 'dark');
    await shot(page, 'mobile-detail-pending');

    await open(page, app, `#/task/${app.taskIds[2]}`);
    await shot(page, 'mobile-detail-running');
    void guard;
  });

  test('对讲台：多急停 + 一轮对话', async ({ page }) => {
    const guard = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/talk');
    await setTheme(page, 'dark');
    await shot(page, 'talk-stops');

    // 展开第一张急停（多张时默认全折叠）
    const expander = page.getByRole('button', { name: /展开恢复动作/ });
    if (await expander.count()) {
      await expander.first().click();
      await page.waitForTimeout(600);
      await shot(page, 'talk-stop-expanded');
    }

    // 发一句话，看时间线里的几种「轮」
    await page.locator('textarea.input').fill('现在卡在哪几个工位？');
    await page.getByRole('button', { name: /发送/ }).click();
    await page.waitForTimeout(3000);
    await shot(page, 'talk-conversation');

    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, '#/talk');
    await setTheme(page, 'dark');
    await shot(page, 'mobile-talk');
    void guard;
  });

  test('指标：有数据', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/metrics');
    await setTheme(page, 'dark');
    await shot(page, 'metrics');
    await setTheme(page, 'light');
    await shot(page, 'metrics-light');
    await setTheme(page, 'dark');

    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, '#/metrics');
    await shot(page, 'mobile-metrics');
  });

  test('项目 / 模型与密钥：列表与表单', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });

    await open(page, app, '#/settings/providers');
    await setTheme(page, 'dark');
    await shot(page, 'providers-list');
    await page.getByRole('button', { name: /新增 provider/ }).click();
    await page.waitForTimeout(400);
    await shot(page, 'providers-form');
    await page.getByRole('button', { name: /取消/ }).first().click();
    await page.waitForTimeout(300);
    await page.getByRole('button', { name: /新增阶段配置/ }).click();
    await page.waitForTimeout(400);
    await shot(page, 'providers-stage-form');
    await page.getByRole('button', { name: /取消/ }).first().click();
    await page.waitForTimeout(300);
    // 项目页放最后：三个任务都在飞，删除钮是**禁用**的（决策 101），
    // 这一张截的就是「按钮禁掉 + 旁边写出原因」的样子。
    await open(page, app, '#/settings/projects');
    await setTheme(page, 'dark');
    await shot(page, 'projects-list');
    await page.getByRole('button', { name: /新建项目/ }).click();
    await page.waitForTimeout(400);
    await shot(page, 'projects-form');

    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, '#/settings/projects');
    await shot(page, 'mobile-projects');
    await open(page, app, '#/settings/providers');
    await shot(page, 'mobile-providers');
  });

  test('手机访问：回环态 → 改绑后的二维码', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/share');
    await setTheme(page, 'dark');
    await shot(page, 'share-loopback');

    // 真按那颗钮：改绑全网卡 → 二维码态
    const toggle = page.getByRole('button', { name: /绑定全网卡/ });
    if (await toggle.count()) {
      await toggle.click();
      await page.waitForTimeout(5000);
      await shot(page, 'share-lan-qr');
      await setTheme(page, 'light');
      await shot(page, 'share-lan-qr-light');
      await setTheme(page, 'dark');

      await page.setViewportSize({ width: 430, height: 932 });
      await open(page, app, '#/share');
      await shot(page, 'mobile-share-lan');
    }
  });

  test('404', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/nope');
    await shot(page, 'notfound');
  });
});

test.describe('② 空库：首启空态', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({ script: foremanScript([[]]), seedless: true });
    await new Promise((r) => setTimeout(r, 1500));
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('对话框的键盘可达性：Escape / 焦点', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/');
    await page.locator('.btn-new').click();
    await page.waitForTimeout(400);
    const before = await page.locator('.dialog').count();
    const focusedAfterOpen = await page.evaluate(
      () => `${document.activeElement?.tagName}.${document.activeElement?.className ?? ''}`,
    );
    await page.keyboard.press('Escape');
    await page.waitForTimeout(400);
    const afterEsc = await page.locator('.dialog').count();
    // 焦点在弹窗内时 Escape 才有人接
    await page.locator('.dialog input').first().focus();
    await page.keyboard.press('Escape');
    await page.waitForTimeout(400);
    const afterEscFocused = await page.locator('.dialog').count();
    console.log(
      `[audit] dialog before=${before} afterEsc=${afterEsc} afterEscFocused=${afterEscFocused} focus=${focusedAfterOpen}`,
    );
    await shot(page, 'dialog-escape-check');
  });

  test('空看板 / 空对讲台 / 空指标 / 空台账', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    for (const [slug, hash] of [
      ['empty-board', '#/'],
      ['empty-talk', '#/talk'],
      ['empty-metrics', '#/metrics'],
      ['empty-projects', '#/settings/projects'],
      ['empty-providers', '#/settings/providers'],
      ['empty-market', '#/settings/market'],
      ['empty-share', '#/share'],
    ] as const) {
      await open(page, app, hash);
      await setTheme(page, 'dark');
      await shot(page, slug);
    }
  });
});

test.describe('③ 技能市场：带离线仓的列表', () => {
  let app: App;
  let remote: GitRepoFixture;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    remote = await startGitRepo({
      owner: 'acme',
      repo: 'skills',
      skills: [
        { dir: 'skills/grilling', description: '拷问设计树', body: ['拷问规则：事实自己查。'] },
        { dir: 'skills/tdd', description: '测试先行', body: ['红绿重构。'] },
      ],
    });
    app = await startApp({
      script: foremanScript([[text('空班。')]]),
      providerOnly: true,
      market: { owner: 'acme', repo: 'skills', gitBase: remote.base },
    });
  });

  test.afterAll(async () => {
    await app?.stop();
    await remote?.close();
  });

  test('仓名单 + 技能列表', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/settings/market');
    await setTheme(page, 'dark');
    await shot(page, 'market-repo-list');

    const view = page.getByRole('button', { name: /查看技能/ });
    if (await view.count()) {
      await view.first().click();
      await page.waitForTimeout(4000);
      await shot(page, 'market-listing');
      await setTheme(page, 'light');
      await shot(page, 'market-listing-light');
    }
  });
});
