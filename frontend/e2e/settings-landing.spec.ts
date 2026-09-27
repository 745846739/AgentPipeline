/**
 * 前端 E2E：设置的信息架构（票 21 / 决策 198）与 404 的出口（票 03）。
 *
 * 口径与其它用例一致：只测**外部行为**——真应用（内嵌产物 + 真后端）、真浏览器、
 * 按用户动作断言；断言落在可访问性契约（导航区的名字、heading 的层级与名字、链接的可读名）
 * 与计算样式上，不落 class 名，也不做说明性段落的精确文案匹配。
 *
 * 覆盖五条：
 *   ① 顶栏**页面导航行恰四项**（对讲台 / 看板 / 指标 / 设置，决策 240 修订决策 198）；
 *   ② 设置落地页两组分类、每一项可点且落到各自的路由；
 *   ③ 非本机来源下「手机访问」项**不渲染**；
 *   ④ `#/settings/stages` 有自己的页面、阶段配置在那里（小节标题不与页面标题同级）；
 *   ⑤ 404 说清状态与下一步，出口是顶栏那一行页签（决策 240：此处不再自带「回看板」）。
 *
 * ③ 的做法：`onHostMachine()`（决策 190）的判据是**来源是否回环**——`api base` 非空时看它，
 * 否则看当前地址（`lib/localPage.ts`）。playwright 造不出一个非回环的浏览器来源，但可以按
 * 桌面壳**文档化的注入口**（`window.__AGENTPIPELINE_API_BASE__`，决策 153④）把 base 指到
 * 一个非回环地址——这正是「壳被指向局域网地址」那一种来源，判据走的就是这条分支。
 * （同一个规则的单元层接线测试在 `src/routes/SettingsLanding.test.ts`。）
 */

import { expect, test } from '@playwright/test';
import { startApp, settleBundle, watchBundle, expectBundleHealthy, type App } from './harness';
import { foremanScript } from './scripts';

/** 非回环的 base（TEST-NET-1，RFC 5737 的文档用网段）：判据只看主机名，故请求打不到谁。 */
const OFF_HOST_BASE = 'http://192.0.2.10:8788';

/** 落地页每一项 → 它该落地的那一页的标题（`h1` 的可见文字）。 */
const ITEMS: Array<{ label: string; hash: string; title: string }> = [
  { label: '项目', hash: '#/settings/projects', title: '设置 · 项目' },
  { label: '值守轮', hash: '#/settings/foreman', title: '设置 · 值守轮' },
  { label: '命令执行', hash: '#/settings/tools', title: '设置 · 命令执行' },
  { label: '模型与密钥', hash: '#/settings/providers', title: '设置 · 模型与密钥' },
  { label: '阶段配置', hash: '#/settings/stages', title: '设置 · 阶段配置' },
  { label: '技能市场', hash: '#/settings/market', title: '设置 · 技能市场' },
  { label: '手机访问', hash: '#/share', title: '手机访问' },
];

test.describe('前端 E2E：设置的信息架构（票 21 / 决策 198）', () => {
  let app: App;

  test.beforeAll(async () => {
    // providerOnly：有可用模型、没有项目与任务——落地页与 404 都不依赖业务数据
    app = await startApp({ script: foremanScript([[]]), providerOnly: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('顶栏页面导航行恰四项：对讲台 / 看板 / 指标 / 设置（决策 240）', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);

    const nav = page.getByRole('navigation', { name: '页面导航' });
    await expect(nav.getByRole('link')).toHaveCount(4);
    await expect(nav.getByRole('link').nth(0)).toHaveText(/对讲台/);
    await expect(nav.getByRole('link').nth(1)).toHaveText(/看板/);
    await expect(nav.getByRole('link').nth(2)).toHaveText(/指标/);
    await expect(nav.getByRole('link').nth(3)).toHaveText(/设置/);

    // 看板落在根路由，且只在根路由上点亮
    await expect(nav.getByRole('link', { name: '看板' })).toHaveAttribute('href', '#/');
    await expect(nav.getByRole('link', { name: '看板' })).toHaveAttribute('aria-current', 'page');

    // 收缩只针对这一行：「新建任务」仍在（它不是导航项）；wordmark 是铭牌，不再是看板入口
    await expect(page.getByRole('button', { name: '新建任务' })).toBeVisible();
    await expect(page.getByText('AGENTPIPELINE').first()).toBeVisible();
    await expect(page.getByRole('link', { name: 'AGENTPIPELINE' })).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test('落地页：两组分类；每一项可点且落到各自的路由', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings`);
    await settleBundle(page, bundle);

    // 标题与分类（分类按用途两分）
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('设置');
    await expect(page.getByRole('heading', { name: '谁能进来' })).toBeVisible();
    await expect(page.getByRole('heading', { name: '怎么跑' })).toBeVisible();

    // 每一项都可点，点了落在那一页（各自路由不变，落地页只是入口）
    for (const item of ITEMS) {
      await page.goto(`${app.webBase}/#/settings`);
      await settleBundle(page, bundle);
      await page.getByRole('link', { name: new RegExp(item.label) }).click();
      await expect(page).toHaveURL(new RegExp(`${item.hash}$`));
      await expect(page.getByRole('heading', { level: 1 })).toContainText(item.title);
    }

    expectBundleHealthy(bundle);
  });

  test('顶栏「设置」在设置类各页面都高亮（每一个设置类路由）', async ({ page }) => {
    const bundle = watchBundle(page);
    const nav = page.getByRole('navigation', { name: '页面导航' });
    const bg = (label: string) =>
      nav
        .getByRole('link', { name: label })
        .evaluate((el) => getComputedStyle(el).backgroundColor);

    for (const hash of [
      '#/settings',
      '#/settings/projects',
      '#/settings/foreman',
      '#/settings/tools',
      '#/settings/notify',
      '#/settings/providers',
      '#/settings/stages',
      '#/settings/market',
      '#/share',
    ]) {
      await page.goto(`${app.webBase}/${hash}`);
      await settleBundle(page, bundle);
      // 当前项 = wash 底（与未选中的透明底不同）；判据是 `route.name` 的集合
      expect(await bg('设置'), hash).not.toBe('rgba(0, 0, 0, 0)');
      expect(await bg('对讲台'), hash).toBe('rgba(0, 0, 0, 0)');
    }

    expectBundleHealthy(bundle);
  });

  test('非本机来源：落地页不渲染「手机访问」项（行为与决策 190 逐字一致）', async ({ page }) => {
    const bundle = watchBundle(page);
    // 桌面壳的文档化注入口：把 base 指到一个非回环地址 → onHostMachine() 为假
    await page.addInitScript((base) => {
      (window as unknown as { __AGENTPIPELINE_API_BASE__?: string }).__AGENTPIPELINE_API_BASE__ =
        base;
    }, OFF_HOST_BASE);

    await page.goto(`${app.webBase}/#/settings`);
    await settleBundle(page, bundle);

    await expect(page.getByRole('heading', { level: 1 })).toHaveText('设置');
    // 不渲染这一项（不是禁用、不是留个空位）
    await expect(page.getByRole('link', { name: /手机访问/ })).toHaveCount(0);
    await expect(page.getByText(/手机访问/)).toHaveCount(0);
    // 其余各项一个不少
    for (const label of ['项目', '值守轮', '命令执行', '模型与密钥', '阶段配置', '技能市场']) {
      await expect(page.getByRole('link', { name: new RegExp(label) }), label).toBeVisible();
    }

    expectBundleHealthy(bundle);
  });

  test('阶段配置有自己的页：小节标题不与页面标题同级，配置就在这一页', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/stages`);
    await settleBundle(page, bundle);

    const title = page.getByRole('heading', { level: 1 });
    await expect(title).toHaveText('设置 · 阶段配置');
    // 小节标题（票 09）：落在 12px 字距档，明确小于页面标题的 24px
    const section = page.getByRole('heading', { level: 2, name: '阶段配置' });
    await expect(section).toBeVisible();
    await expect(section).toHaveCSS('font-size', '12px');
    await expect(title).toHaveCSS('font-size', '24px');
    // 阶段配置的编辑器与清单都在这一页（新增入口 + 台账或空态）
    await expect(page.getByRole('button', { name: /新增阶段配置/ })).toBeVisible();

    // 同一口径也覆盖设置落地页（两个设置页）
    await page.goto(`${app.webBase}/#/settings`);
    await settleBundle(page, bundle);
    await expect(page.getByRole('heading', { level: 1 })).toHaveCSS('font-size', '24px');
    await expect(page.getByRole('heading', { name: '谁能进来' })).toHaveCSS('font-size', '12px');

    expectBundleHealthy(bundle);
  });

  test('404 说清状态与下一步，出口是顶栏的看板页签（决策 240）', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/nope`);
    await settleBundle(page, bundle);

    // 状态一行说清是什么 + 干净的路由名（hash 路由里恒有的 `#` 不进文案）
    const state = page.locator('p', { hasText: '页面不存在' }).first();
    await expect(state).toBeVisible();
    await expect(state).toContainText('/nope');
    expect(await state.textContent()).not.toContain('#');

    // 不再自带一条「回看板」：看板入口**只有**顶栏那一枚页签（同一个目的地两套进法）
    await expect(page.getByRole('link', { name: '回看板' })).toHaveCount(0);

    // 点那枚页签，落在看板（八列工位阵列在场）
    await page.getByRole('navigation', { name: '页面导航' }).getByRole('link', { name: '看板' }).click();
    await expect(page).toHaveURL(/#\/$/);
    await expect(page.locator('p', { hasText: '页面不存在' })).toHaveCount(0);
    await expect(page.locator('section.col').first()).toBeVisible({ timeout: 60_000 });

    expectBundleHealthy(bundle);
  });
});
