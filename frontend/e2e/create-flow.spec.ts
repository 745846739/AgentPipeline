/**
 * E2E-⑤ 三步创建全程走 UI（主流程票 05）。
 *
 * 主流程的前三步——配 provider、加项目、建任务——此前在端到端里被 harness
 * 播种完全绕过：表单校验、失败提示、空状态引导、提交跳转零证据，而用户
 * **必须**从这三步开始（没有 provider 建任务被决策 56 拒，没有项目连
 * 「新建任务」按钮都不渲染）。本用例在**空 home** 上：
 *   ① 断言两处空状态引导；
 *   ② 走 UI 建 provider（含「测试连接」），断言 api_key 列表只回显掩码（决策 112）；
 *   ③ 走 UI 建项目——先填不存在路径断言明确报错（校验失败路径是验收项），再填真路径；
 *   ④ 走 UI 建任务 → 断言跳转 `#/task/{id}`；
 *   ⑤ 断言**用户填的描述真的进了 prompt**（mock 记录的新节点运行请求里含描述原文）
 *      ——「用户填的字有没有被用上」的唯一端到端证据；
 *   ⑥ 依赖字段：建第二个任务填第一个任务 id，断言后端建出依赖关系。
 *
 * 真 axum 后端 + mock LLM + 临时 home；页面加载编译期内嵌的真实 bundle（票 01）；
 * 只 Chromium（决策 144）。
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
import { fullPassScript } from './scripts';

const DESCRIPTION_MARKER = '描述进 prompt 的唯一标记-8f3a：实现 add 使 npm test 通过';

test.describe('前端 E2E ⑤：三步创建走 UI', () => {
  let app: App;
  const title = 'E2E UI 创建的任务';
  let firstTaskId = '';

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('E2E'), title, seedless: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('① 空 home 的两处空状态引导可见（首启第一印象）', async ({ page }) => {
    const bundle = watchBundle(page);

    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    await expect(page.locator('.board-empty')).toContainText('还没有项目');
    await expect(page.locator('.board-empty')).toContainText('设置 · 项目');

    await page.goto(`${app.webBase}/#/settings/providers`);
    await settleBundle(page, bundle);
    await expect(page.locator('.banner', { hasText: '还没有 provider' })).toBeVisible();

    expectBundleHealthy(bundle);
  });

  test('② 走 UI 建 provider：列表只回显掩码，测试连接可用', async ({ page }) => {
    const bundle = watchBundle(page);

    await page.goto(`${app.webBase}/#/settings/providers`);
    await settleBundle(page, bundle);
    await page.getByRole('button', { name: '新增 provider' }).click();

    const form = page.locator('form.prov-form');
    await form.locator('.field', { hasText: '厂商 vendor' }).locator('input').fill('openai');
    await form.locator('.field', { hasText: '模型 model' }).locator('input').fill('mock');
    await form.locator('.field', { hasText: 'base_url' }).locator('input').fill(app.mockUrl);
    await form.locator('.field', { hasText: 'api_key' }).locator('input').fill('sk-e2e-secret-9012');

    // 「测试连接」（决策 160）：探针指向脚本 mock，成功结论 + 延迟可见
    await form.getByRole('button', { name: '测试连接' }).click();
    await expect(form.locator('.test-result')).toContainText('连接成功');

    await form.getByRole('button', { name: '创建' }).click();

    // 列表出现该行；api_key 只回显掩码，明文绝不上屏（决策 112）
    const row = page.locator('li.row', { hasText: 'mock' });
    await expect(row).toBeVisible();
    await expect(row).toContainText('api_key ***');
    const body = await page.content();
    expect(body).not.toContain('sk-e2e-secret-9012');

    expectBundleHealthy(bundle);
  });

  test('③ 走 UI 建项目：坏路径先得到明确报错，真路径成功', async ({ page }) => {
    const bundle = watchBundle(page);

    await page.goto(`${app.webBase}/#/settings/projects`);
    await settleBundle(page, bundle);
    await page.getByRole('button', { name: '新建项目' }).click();

    const form = page.locator('form.proj-form');
    await form.locator('.field', { hasText: '名称' }).locator('input').fill('e2e-ui');
    const pathInput = form.locator('.field', { hasText: 'local_path' }).locator('input');

    // 校验失败路径是验收项：不存在的路径必须得到明确提示，而不是静默或通用错误
    await pathInput.fill('/tmp/definitely-not-a-repo-e2e-xyz');
    await form.getByRole('button', { name: '创建' }).click();
    await expect(form.locator('.error')).toBeVisible();

    await pathInput.fill(app.repoDir);
    await form.getByRole('button', { name: '创建' }).click();
    await expect(page.locator('li.row', { hasText: 'e2e-ui' })).toBeVisible();

    expectBundleHealthy(bundle);
  });

  test('④ 走 UI 建任务：跳转任务详情，描述真的进了 prompt', async ({ page }) => {
    const bundle = watchBundle(page);

    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    // 无任务但有项目：第二种空状态
    await expect(page.locator('.board-empty')).toContainText('新建第一个任务');

    await page.getByRole('button', { name: '新建任务' }).click();
    const dialog = page.locator('form.dialog');
    await dialog.locator('.field', { hasText: '标题' }).locator('input').fill(title);
    await dialog
      .locator('.field', { hasText: '描述' })
      .locator('textarea')
      .fill(DESCRIPTION_MARKER);
    await dialog.getByRole('button', { name: '创建并启动' }).click();

    // 提交后跳转任务详情（NewTaskDialog 的 router.navigate）
    await expect(page).toHaveURL(/#\/task\//);
    firstTaskId = page.url().split('/task/')[1] ?? '';
    expect(firstTaskId).not.toBe('');
    await expect(page.locator('h1, .head, .crumb').first()).toBeVisible();
    await settleBundle(page, bundle);

    // mock 跑完整脚本到 merge_approval
    await waitForTaskById(
      firstTaskId,
      app,
      (t) => pendingTypeOf(t) === 'merge_approval',
      'merge_approval',
    );

    // 「用户填的字有没有被用上」的唯一端到端证据：新节点运行的 user prompt 含描述原文
    const prompts = app.prompts();
    expect(prompts.length).toBeGreaterThan(0);
    expect(
      prompts.some((p) => p.user.includes(DESCRIPTION_MARKER)),
      `描述应出现在 user prompt 中；实际 ${prompts.length} 条首请求`,
    ).toBe(true);

    expectBundleHealthy(bundle);
  });

  test('⑤ 依赖任务 ID 字段解析出依赖关系', async ({ page }) => {
    test.skip(!firstTaskId, '前置用例未产出任务 id');
    const bundle = watchBundle(page);

    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
    await page.getByRole('button', { name: '新建任务' }).click();
    const dialog = page.locator('form.dialog');
    await dialog.locator('.field', { hasText: '标题' }).locator('input').fill('E2E 依赖任务');
    await dialog
      .locator('.field', { hasText: '描述' })
      .locator('textarea')
      .fill('E2E 依赖任务：验证依赖任务 ID 字段');
    await dialog
      .locator('.field', { hasText: '依赖任务 ID' })
      .locator('input')
      .fill(` ${firstTaskId} ,`);
    await dialog.getByRole('button', { name: '创建并启动' }).click();
    await expect(page).toHaveURL(/#\/task\//);

    // 后端建出依赖关系（逗号分隔 + 空白容错）
    const secondId = page.url().split('/task/')[1] ?? '';
    expect(secondId).not.toBe('');
    expect(secondId).not.toBe(firstTaskId);
    const wrapped = (await fetchTaskById(app, secondId)) as {
      task: Record<string, unknown>;
      depends_on: string[];
    };
    expect(wrapped.depends_on).toContain(firstTaskId);

    expectBundleHealthy(bundle);
  });
});

async function fetchTaskById(app: App, id: string): Promise<Record<string, unknown>> {
  const res = await fetch(`${app.apiBase}/tasks/${id}`);
  if (!res.ok) throw new Error(`GET /tasks/${id} -> ${res.status}`);
  return (await res.json()) as Record<string, unknown>;
}
