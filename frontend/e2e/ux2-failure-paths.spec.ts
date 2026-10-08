/**
 * UX 第二轮 · 票 01 / 02：失败有出口、错误说得出口（R2-01 / 06 / 07 / 08）。
 *
 * 这三组用例都是「被推到失败」的界面：读不到任务、读不到仓、动作提交失败。
 * 断言落在**用户看得见 / 听得见**的东西上：屏上有没有上一个任务的残留、有没有动作按钮、
 * 有没有一颗能按的出路、`role=alert` 里说了什么——不落 class 名、不落组件内部状态。
 *
 * 关于 `role=alert`：读屏只认这一条通道，红颜色只说给看得见的人。所以「错误可见」的判据
 * 是 `[role=alert]` 里出现了那句话，而不是「页面上有红字」。
 *
 * 真 axum 后端 + mock LLM + 临时 home；只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  expectBundleHealthy,
  pendingTypeOf,
  settleBundle,
  startApp,
  waitForTaskById,
  watchBundle,
  type App,
} from './harness';
import { clickConfirmed } from './confirm';
import { startGitRepo, type GitRepoFixture } from './gitRepo';
import { foremanScript, fullPassScript, siblingPassScript, text } from './scripts';

/* ───────── ① 详情：读不到任务不留残留 + 终态动作失败不再静默 ───────── */

test.describe('UX2 ① 详情失败态与终态动作（票 01 / 02）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({
      script: fullPassScript('UX2F'),
      title: 'UX2 详情失败态',
      additionalTasks: [{ title: 'UX2 归档失败', script: siblingPassScript('UX2F2') }],
    });
    await waitForTaskById(
      app.taskIds[0],
      app,
      (t) => pendingTypeOf(t) === 'merge_approval',
      'merge_approval',
      180_000,
    );
    await waitForTaskById(
      app.taskIds[1],
      app,
      (t) => pendingTypeOf(t) === 'merge_approval',
      '第二个任务到 merge_approval',
      180_000,
    );
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('切到一个不存在的 id：上一个任务连同它的动作按钮一起收走，并给「重新加载」', async ({
    page,
  }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/task/${app.taskIds[0]}`);
    await settleBundle(page, bundle);
    const title = (await page.locator('.d-title').first().innerText()).trim();
    expect(title.length, '前置：有效任务应当有标题').toBeGreaterThan(0);

    // 同页切 hash（不重载）：这正是审计里「红字底下仍是上一个任务」的那一刻
    await page.goto(`${app.webBase}/#/task/01JZZZZZZZZZZZZZZZZZZZZZZZ`);
    await page.waitForTimeout(1200);

    // 上一个任务的一切都不在**详情页正文**里：标题、页签、六颗拍板按钮（它们会提交到那个坏 id）。
    //
    // 判据不用 `body`：顶栏的「待处理」下拉**常驻 DOM**（票 04：靠 `hidden` 开合，`aria-controls`
    // 指过去的目标必须真的在），它会列出 pending 任务的**标题**——那是顶栏的合法内容，不是残留。
    // Playwright 的 `toContainText` 走 DOM 文本（不看 CSS 可见性），拿 `body` 当判据就会把
    // 这份合法内容误判成残留（本用例 2026-09-18 就是这么红过一次）。
    const detail = page.locator('main.detail');
    await expect(detail).not.toContainText(title);
    await expect(page.locator('.d-title')).toHaveCount(0);
    await expect(page.getByRole('tab', { name: '时间线' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: '合入' })).toHaveCount(0);
    // 空态可达，且带一条出路（横幅与空态各说一遍同一句，取第一处即可）
    await expect(page.getByRole('alert')).toContainText('任务不存在');
    await expect(page.getByText(/任务不存在/).first()).toBeVisible();
    await expect(page.getByRole('button', { name: '重新加载', exact: true })).toBeVisible();
    // 出路是**那颗「重新加载」**（决策 240 摘掉了本页的「回看板」链）：回看板走顶栏那枚页签，
    // 死路上不再摆第二个入口。顶栏那一行恒在，故这一页并不因此没了去处。
    await expect(page.getByRole('link', { name: '回看板' })).toHaveCount(0);
    await expect(
      page.getByRole('navigation', { name: '页面导航' }).getByRole('link', { name: '看板' }),
    ).toBeVisible();
    expectBundleHealthy(bundle);
  });

  test('首次装载 500：给可读的错误 + 「重新加载」，恢复后点它内容回来', async ({ page }) => {
    const bundle = watchBundle(page);
    let failNext = true;
    await page.route(`**/tasks/${app.taskIds[1]}`, async (route) => {
      if (route.request().method() !== 'GET' || !failNext) return route.continue();
      failNext = false;
      await route.fulfill({ status: 500, contentType: 'text/plain', body: 'injected failure' });
    });

    await page.goto(`${app.webBase}/#/task/${app.taskIds[1]}`);
    await settleBundle(page, bundle);

    // 失败要说出口（票 02）：原因那一行进 live region 且非空（不是一个占位句）
    const alert = page.locator('.main [role=alert]');
    await expect(alert).toBeVisible();
    expect((await alert.innerText()).trim().length, 'live region 要说出一条真原因').toBeGreaterThan(0);
    await expect(page.getByRole('button', { name: '重新加载', exact: true })).toBeVisible();
    // 「没读到」不是「这个 id 没有」：两句话不混用
    await expect(page.locator('.main')).toContainText('任务没能打开');
    await expect(page.locator('.main')).not.toContainText('任务不存在');

    // 后端恢复后点「重新加载」：内容真的回来（首次加载失败不再是永久死页）
    await page.getByRole('button', { name: '重新加载', exact: true }).click();
    await expect(page.locator('.d-title')).toContainText('UX2 归档失败', { timeout: 30_000 });
    await expect(page.locator('.main [role=alert]')).toHaveCount(0);
    expectBundleHealthy(bundle);
  });

  test('终态任务点「归档」而请求失败：横幅与 live region 都说出来（不再静默复活）', async ({
    page,
  }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/task/${app.taskIds[1]}`);
    await settleBundle(page, bundle);

    // 合入 → done（终态），旁路行（归档）才会出现。
    // `合入` 是 destructive 档 → 内联两步确认（决策 216②）；`归档` 无确认步，不走这个助手。
    await clickConfirmed(page.locator('aside.dossier').getByRole('button', { name: '合入' }));
    await waitForTaskById(app.taskIds[1], app, (t) => t.status === 'done', 'done', 90_000);

    // 完成横幅本身是一个可点的主体（可访问名是「<标题> · done」），子串匹配会同时命中它
    // ——收敛到旁路行里的那一颗。
    const archive = page.locator('.main .bypass').getByRole('button', { name: '归档', exact: true });
    await expect(archive).toBeVisible({ timeout: 30_000 });

    await page.route(`**/tasks/${app.taskIds[1]}/archive`, (route) =>
      route.fulfill({ status: 500, contentType: 'text/plain', body: 'injected failure' }),
    );
    await archive.click();

    // 修复前：按钮静静复活，`bannerErrors` 为空。修复后：动作错误位说出失败。
    await expect(page.locator('.main [role=alert]')).toContainText('动作提交失败', {
      timeout: 30_000,
    });
    expectBundleHealthy(bundle);
  });
});

/* ───────── ② 技能市场：断网刷新不再把列表与控制一起弄没 ───────── */

test.describe('UX2 ② 市场断网刷新（票 02 / R2-07a）', () => {
  let app: App;
  let remote: GitRepoFixture;

  test.beforeAll(async () => {
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

  test('断网刷新：已列出的技能还在、还能再试一次', async ({ page, context }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    await page.getByRole('button', { name: '查看技能' }).first().click();
    await expect(page.locator('.hit-name', { hasText: 'grilling' })).toBeVisible({
      timeout: 60_000,
    });
    const refresh = page.getByRole('button', { name: '刷新', exact: true });
    await expect(refresh).toBeVisible();

    // 断网 → 刷新：错误要出现，但**列表与控制不能消失**
    await context.setOffline(true);
    await refresh.click();

    const alert = page.locator('[role=alert]').first();
    await expect(alert).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('.hit-name', { hasText: 'grilling' })).toBeVisible();
    await expect(refresh).toBeVisible();
    await expect(page.getByRole('button', { name: '重试', exact: true })).toBeVisible();

    // 网络回来 → 重试：错误消失，内容照旧
    await context.setOffline(false);
    await page.getByRole('button', { name: '重试', exact: true }).click();
    await expect(page.locator('[role=alert]')).toHaveCount(0, { timeout: 60_000 });
    await expect(page.locator('.hit-name', { hasText: 'grilling' })).toBeVisible();
    expectBundleHealthy(bundle);
  });
});

/* ───────── ③ 设置页读不到：横幅进 live region 且自带重试 ───────── */

test.describe('UX2 ③ 设置页读不到时的出路（票 02 / R2-07c）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: foremanScript([[text('空班。')]]), providerOnly: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('读 provider 列表失败：role=alert + 一颗能按的「重试」，点了列表回来', async ({ page }) => {
    const bundle = watchBundle(page);
    let failNext = true;
    await page.route('**/providers', async (route) => {
      if (route.request().method() !== 'GET' || !failNext) return route.continue();
      failNext = false;
      await route.fulfill({ status: 500, contentType: 'text/plain', body: 'injected failure' });
    });

    await page.goto(`${app.webBase}/#/settings/providers`);
    await settleBundle(page, bundle);

    await expect(page.locator('.banner.error[role=alert]')).toBeVisible();
    const retry = page.getByRole('button', { name: '重试', exact: true });
    await expect(retry).toBeVisible();

    await retry.click();
    await expect(page.locator('.banner.error[role=alert]')).toHaveCount(0, { timeout: 30_000 });
    await expect(page.locator('.reg-row').first()).toBeVisible({ timeout: 30_000 });
    expectBundleHealthy(bundle);
  });
});
