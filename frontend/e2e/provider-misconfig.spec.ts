/**
 * E2E-④ provider 配错可理解、可恢复（主流程票 03）。
 *
 * 真实使用中最高频的失败：密钥 / 地址配错。此前它只表现为原始错误串
 * （英文 HTTP 状态 + 供应商返回体片段）——用户不知道哪一步失败、该改什么。
 * 本用例断言：
 *   ① 面板给**中文可操作提示**（指明去「设置 · 模型与密钥」检查什么）；
 *   ② 原始诊断保留在面板上（不丢，供排查），但**不**挤占提示行；
 *   ③ 用户修好配置 → 「重试执行」→ 流程恢复推进（不是死路）。
 *
 * 真 axum 后端 + mock LLM（坏 provider 指向恒定 401 的 mock）+ 临时 home；
 * 页面加载编译期内嵌的真实 bundle（票 01）；只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  fetchTask,
  startApp,
  waitForTask,
  pendingTypeOf,
  watchBundle,
  settleBundle,
  expectBundleHealthy,
  type App,
} from './harness';
import { fullPassScript } from './scripts';

test.describe('前端 E2E ④：provider 配错可恢复', () => {
  let app: App;
  const title = 'E2E bad provider';

  test.beforeAll(async () => {
    app = await startApp({
      script: fullPassScript('E2E'),
      title,
      badProvider: {
        status: 401,
        body: '{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}',
      },
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('面板给中文可操作提示 + 保留原始诊断；修好配置后恢复推进', async ({ page }) => {
    const bundle = watchBundle(page);

    // ── 等真实失败发生：任务挂在 retry_exhausted ──
    await waitForTask(app, (t) => pendingTypeOf(t) === 'retry_exhausted', 'retry_exhausted');

    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);

    const dossier = page.locator('aside.dossier');
    await expect(dossier).toBeVisible({ timeout: 60_000 });

    // ── 断言 1：中文可操作提示（指明去哪里、查什么），而不是原始英文错误串 ──
    await expect(dossier.locator('.msg')).toContainText('鉴权失败');
    await expect(dossier.locator('.msg')).toContainText('api_key');
    // 提示行**不**混入原始返回体——原始串有自己的位置，主提示保持可读
    await expect(dossier.locator('.msg')).not.toContainText('Incorrect API key');

    // ── 断言 2：原始诊断保留在面板上（可诊断性不倒退），但作为次要信息 ──
    const diagnostic = dossier.locator('.ctx', { hasText: '诊断：' });
    await expect(diagnostic).toContainText('401');
    await expect(diagnostic).toContainText('Incorrect API key');

    // ── 断言 3：动作集给了出路（重试 / 终止），不是死面板 ──
    await expect(dossier.locator('button', { hasText: '重试' }).first()).toBeVisible();
    await expect(dossier.locator('button', { hasText: '终止' }).first()).toBeVisible();

    // ── 恢复：用户修好配置（切回脚本 mock），点「重试」 ──
    await app.fixProvider();
    const retry = dossier.locator('button', { hasText: '重试' }).first();
    await expect(retry).toBeEnabled();
    await retry.click();

    // ── 断言 4：流程恢复——重试耗尽被清除，推进到合并提案 ──
    await waitForTask(
      app,
      (t) => pendingTypeOf(t) === 'merge_approval',
      '修复后推进到 merge_approval',
      180_000,
    );
    await expect(dossier.locator('.dtag')).toContainText('合并提案', { timeout: 60_000 });

    expectBundleHealthy(bundle);
  });
});
