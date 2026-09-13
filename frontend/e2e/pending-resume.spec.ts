/**
 * E2E-② pending → dossier 面板 → resume（票 18 / 决策 151）。
 *
 * 断言「琥珀面板」（dossier 的 `--pending` 色）与顶栏待办计数，然后提交 resume，
 * 断言该 pending 被清除且流水线继续推进（信息不足 → 合并提案）。
 * 真 axum 后端 + mock LLM + 临时 home；页面加载**编译期内嵌的真实 bundle**
 * （主流程票 01）；只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  startApp,
  waitForTask,
  pendingTypeOf,
  watchBundle,
  settleBundle,
  type App,
} from './harness';
import {
  NODE,
  ValidateInput,
  designRounds,
  implementationRounds,
  submit,
  type NodeScript,
} from './scripts';

/**
 * architect-design.validate_input 首轮 readiness=false → pending(info_insufficient)；
 * resume 带补充输入后重入同一节点消费第二轮（true），随后走完余下流水线，
 * 最终停在 `merge_approval`——以此证明 info_insufficient 确实被 resume 清掉、
 * 而非停在原地（脚本耗尽会重试到 retry_exhausted，那样面板不会消失）。
 */
function pendingThenContinueScript(taskId: string): NodeScript {
  return {
    ...designRounds(),
    ...implementationRounds(taskId),
    [NODE.archVI]: [[submit(ValidateInput(false, ['缺少技术约束']))], [submit(ValidateInput(true))]],
  };
}

test.describe('前端 E2E ②：pending → dossier 面板 → resume', () => {
  let app: App;
  const title = 'E2E pending resume';

  test.beforeAll(async () => {
    app = await startApp({ script: pendingThenContinueScript('E2E'), title });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('琥珀 dossier 面板 + 顶栏待办计数，resume 后该 pending 清除并继续推进', async ({ page }) => {
    // 真实产物守卫（主流程票 01）。
    const bundle = watchBundle(page);

    // ── 等真后端把任务推进到 pending(info_insufficient) ──
    await waitForTask(app, (t) => pendingTypeOf(t) === 'info_insufficient', 'info_insufficient');

    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    // 内嵌产物健康先于业务断言（主流程票 01）：产物坏掉时立刻报根因，而非等 60s 超时。
    await settleBundle(page, bundle);

    // ── 琥珀 dossier 面板：文案 + `--pending` 色 ──
    // 像素主题（决策 169）：琥珀仍是全站唯一告警色，值由主题三的 #ffb454 换成
    // 像素主题的 #FFB545（theme-6-pixel.md §2.1）。断言的**行为**不变——
    // 「pending 面板是琥珀」，只是换成了像素主题的琥珀 token 值。
    const dossier = page.locator('aside.dossier');
    await expect(dossier).toBeVisible({ timeout: 60_000 });
    await expect(dossier.locator('.dtag')).toContainText('信息不足');
    await expect(dossier.locator('.dtag')).toHaveCSS('color', 'rgb(255, 181, 69)');
    // dossier 的正文是后端下发的 pending message（blockers 不进这个字段）
    await expect(dossier.locator('.msg')).toContainText('设计输入信息不足');

    // ── 顶栏待办计数 = 1 ──
    await expect(page.locator('.pending-count .c')).toHaveText(/\*1/, { timeout: 60_000 });

    // ── resume：`requires_input` 动作带自由输入（决策 79） ──
    const textarea = dossier.locator('textarea.input');
    await expect(textarea).toBeVisible();
    await textarea.fill('技术约束：仅 Chromium，单机本地运行。');

    const resumeBtn = dossier.locator('button.btn.solid', { hasText: '补充信息并继续' });
    await expect(resumeBtn).toBeEnabled();
    const resumed = page.waitForResponse(
      (res) => /\/tasks\/[^/]+\/resume$/.test(res.url()) && res.request().method() === 'POST',
      { timeout: 30_000 },
    );
    await resumeBtn.click();
    const response = await resumed;
    expect(response.ok(), `resume 应成功：${response.status()}`).toBe(true);

    // ── 断言：info_insufficient 已清除、流水线继续推进到 merge_approval ──
    // 面板不消失是正常的——推进后换成了下一个待办；关键是 pending 类型已变、
    // 顶点计数不再停在 info_insufficient。
    await waitForTask(
      app,
      (t) => pendingTypeOf(t) === 'merge_approval',
      'resume 后推进到 merge_approval',
      120_000,
    );
    await expect(dossier.locator('.dtag')).toContainText('合并提案', { timeout: 60_000 });
    await expect(page.locator('.pending-count .c')).toHaveText(/\*1/, { timeout: 30_000 });
  });
});
