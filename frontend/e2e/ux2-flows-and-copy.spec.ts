/**
 * UX 第二轮 · 票 10 / 11 / 15：新建任务的跳转、provider 的表单校验、文案与格式。
 *
 * 三条都落在**真用户路径**上：走 UI 铺项目、走 UI 填表单、走 UI 建任务。
 *
 * 真 axum 后端 + mock LLM + 临时 home；只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  expectBundleHealthy,
  settleBundle,
  startApp,
  watchBundle,
  type App,
} from './harness';
import { fullPassScript } from './scripts';

test.describe('UX2 ⑧ 新建任务 / 表单校验 / 文案（票 10 / 11 / 15）', () => {
  let app: App;
  let projectB = '';
  let taskInB = '';

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('UX2C'), title: 'UX2 甲任务', seedless: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('① 铺三件套：provider + 项目甲 + 项目乙（都走 UI）', async ({ page }) => {
    const bundle = watchBundle(page);

    await page.goto(`${app.webBase}/#/settings/providers`);
    await settleBundle(page, bundle);
    await page.getByRole('button', { name: '新增 provider' }).click();
    const provForm = page.locator('form.prov-form');
    await provForm.locator('.field', { hasText: '厂商 vendor' }).locator('input').fill('openai');
    await provForm.locator('.field', { hasText: '模型 model' }).locator('input').fill('mock');
    await provForm.locator('.field', { hasText: 'base_url' }).locator('input').fill(app.mockUrl);
    await provForm.locator('.field', { hasText: 'api_key' }).locator('input').fill('sk-ux2-secret');
    await provForm.getByRole('button', { name: '创建' }).click();
    await expect(page.locator('li.row', { hasText: 'mock' })).toBeVisible();

    await page.goto(`${app.webBase}/#/settings/projects`);
    await settleBundle(page, bundle);
    for (const name of ['项目甲', '项目乙']) {
      await page.getByRole('button', { name: '新建项目' }).click();
      const form = page.locator('form.proj-form');
      await form.locator('.field', { hasText: '名称' }).locator('input').fill(name);
      await form.locator('.field', { hasText: 'local_path' }).locator('input').fill(app.repoDir);
      await form.getByRole('button', { name: '创建' }).click();
      await expect(page.locator('li.row', { hasText: name })).toBeVisible();
    }

    const projects = (await (await fetch(`${app.apiBase}/projects`)).json()) as {
      projects: Array<{ id: string; name: string }>;
    };
    projectB = projects.projects.find((p) => p.name === '项目乙')?.id ?? '';
    expect(projectB, '项目乙应当建出来了').not.toBe('');
    expectBundleHealthy(bundle);
  });

  test('② 看板停在项目甲，对话框选项目乙建任务：跳到的是**乙的那一条**', async ({ page }) => {
    const bundle = watchBundle(page);

    // 看板当前项目 = 甲，甲里先有一条任务
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    await page
      .locator('label.proj select')
      .selectOption({ label: '项目甲' });
    await page.waitForTimeout(300);
    await page.getByRole('button', { name: '新建任务' }).click();
    let dialog = page.locator('form.dialog');
    await dialog.locator('.field', { hasText: '标题' }).locator('input').fill('甲里的任务');
    await dialog.getByRole('button', { name: '创建并启动' }).click();
    await expect(page).toHaveURL(/#\/task\//);
    const taskInA = page.url().split('/task/')[1] ?? '';
    expect(taskInA).not.toBe('');

    // 回看板（仍在甲），这次把对话框的项目改成乙
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    await page.getByRole('button', { name: '新建任务' }).click();
    dialog = page.locator('form.dialog');
    await dialog.locator('.field', { hasText: '项目' }).locator('select').selectOption(projectB);
    await dialog.locator('.field', { hasText: '标题' }).locator('input').fill('乙里的任务');
    await dialog.getByRole('button', { name: '创建并启动' }).click();

    await expect(page).toHaveURL(/#\/task\//);
    taskInB = page.url().split('/task/')[1] ?? '';
    // 修复前这里会跳到甲的任务（`tasks[0]`），甲没有任务时则哪儿都不去
    expect(taskInB, '必须跳到新建的那一条').not.toBe('');
    expect(taskInB, '不能跳到甲的任意任务上').not.toBe(taskInA);

    const detail = (await (await fetch(`${app.apiBase}/tasks/${taskInB}`)).json()) as {
      task: { project_id: string; title: string };
    };
    expect(detail.task.title).toBe('乙里的任务');
    expect(detail.task.project_id).toBe(projectB);

    // 切到乙的看板：这条任务真的在
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    await page.locator('label.proj select').selectOption(projectB);
    await expect(page.locator('.card, .tcard', { hasText: '乙里的任务' }).first()).toBeVisible({
      timeout: 30_000,
    });
    expectBundleHealthy(bundle);
  });

  test('③ provider 表单：非法 base_url 当场拦下，一行都不落库', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/providers`);
    await settleBundle(page, bundle);
    await page.getByRole('button', { name: '新增 provider' }).click();

    const form = page.locator('form.prov-form');
    await form.locator('.field', { hasText: '厂商 vendor' }).locator('input').fill('openai');
    await form.locator('.field', { hasText: '模型 model' }).locator('input').fill('not-a-url-model');
    const baseInput = form.locator('.field', { hasText: 'base_url' }).locator('input');
    // 实测里那个值：此前照单落库、字段零报错
    await baseInput.fill('not a url');
    await form.getByRole('button', { name: '创建' }).click();

    // 字段级错误 + 拦下（表单还在、那一格被标出来）
    await expect(form.locator('.error[role=alert]')).toContainText('base_url');
    await expect(baseInput).toHaveAttribute('aria-invalid', 'true');
    const rows = await page.locator('li.row', { hasText: 'not-a-url-model' }).count();
    expect(rows, '不该落库').toBe(0);

    // 改对了就能提交（不是把整页卡死）
    await baseInput.fill('https://api.example.com/v1');
    await form.getByRole('button', { name: '创建' }).click();
    await expect(page.locator('li.row', { hasText: 'not-a-url-model' })).toBeVisible();
    expectBundleHealthy(bundle);
  });

  test('④ 对讲台的配对提示里没有字面 `**`（未配对态）', async ({ page }) => {
    const bundle = watchBundle(page);
    // 造未配对态：读会话的那一读被拒（403 是「这台设备还没配对」的那一档）。
    // **必须带 `kind`**：判据按字段走（票 04 / 决策 259，`isPairingRequired`），只有报文
    // 没有 `kind` 的话界面认不出这是配对缺失，那条指引就不会出现——这一栏是生产
    // `stream.rs::pairing_rejected()` 真下的形状，mock 照它写。
    await page.route('**/foreman/session*', (route) =>
      route.fulfill({
        status: 403,
        contentType: 'application/json',
        body: JSON.stringify({ error: '这台设备还没配对', kind: 'pairing_required' }),
      }),
    );

    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const note = page.locator('p.note', { hasText: '重新添加一次' }).first();
    await expect(note).toBeVisible({ timeout: 30_000 });
    // 加粗走真样式（模板里此前是字面的 **，Svelte 不解析 Markdown）
    expect(await note.locator('b').count()).toBeGreaterThan(0);
    // 「本次启动地址」那一行（决策 285）：standalone 窗口没有地址栏，这一行是排障唯一的读数处
    // ——这一栏是这条地址里没有令牌、本机也没存过的那一档（e2e 用的是干净上下文）。
    const launch = page.locator('p.note', { hasText: '本次启动地址' }).first();
    await expect(launch).toBeVisible();
    await expect(launch).toContainText('不带令牌的地址');
    const body = await page.evaluate(() => document.body.innerText);
    expect(body, '页面上不该出现字面的 **').not.toContain('**');
    expectBundleHealthy(bundle);
  });
});
