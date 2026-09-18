/**
 * UX 第二轮 · 票 03（R2-03）：对话框动作的「提交中」态与重入护栏。
 *
 * 拆分 / 换模型此前**从不进提交中态**：按钮不禁用、没有转圈，慢网络下再点一次就发出第二个
 * `POST /tasks/{id}/split`——原任务被取消两次、子任务建两套。这条用例钉三件事：
 *
 * 1. 点了「确认拆分」之后按钮**真的禁用**并转圈（`submitting` 有落点）；
 * 2. 连点两次**只发出一个请求**（禁用的按钮是第一道，store 的重入护栏是第二道）；
 * 3. 失败路径不进死循环：错误可读、busy 复位（那一条在 store 单测里钉，见
 *    `src/stores/taskDetail.test.ts`）。
 *
 * **为什么能造出这个场景**：`context_overflow` 这个 pending 态前端 harness
 * （按 persona 投喂 mock LLM）造不出来，票面因此要求「想个办法让它可测」。办法是
 * `page.route` 只改写**这一段响应**：把待办原因换成 `context_overflow`、动作集换成
 * 「拆分任务 / 更换长上下文模型」。任务在后端的真实状态一字未动（它仍停在
 * `merge_approval`），被测的是**前端在这套输入下的行为**——正是本票的对象。
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

/** 把这一段详情响应改写成 `context_overflow` 的待办（其余字段原样透传）。 */
async function fabricateOverflowPending(page: import('@playwright/test').Page, taskId: string) {
  await page.route(`**/tasks/${taskId}`, async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const response = await route.fetch();
    const body = (await response.json()) as {
      task: { pending_reason: unknown; status: string };
      cursors: Array<{ cursor_id: string }>;
      allowed_actions: unknown[];
    };
    const cursorId = body.cursors[0]?.cursor_id;
    body.task.status = 'pending';
    body.task.pending_reason = {
      type: 'context_overflow',
      stage: 'develop',
      node: 'execute',
      message: '这一轮的输入超出了模型窗口。',
    };
    body.allowed_actions = [
      { action: 'split_task', kind: 'side_effect', label: '拆分任务', cursor_id: cursorId },
      {
        action: 'model_override',
        kind: 'side_effect',
        label: '更换长上下文模型',
        cursor_id: cursorId,
      },
    ];
    await route.fulfill({ response, json: body });
  });
}

test.describe('UX2 ④ 拆分 / 换模型的重入护栏（票 03）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('UX2R'), title: 'UX2 重入' });
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval', 180_000);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('「确认拆分」连点两次只发出一个请求，提交中按钮禁用', async ({ page }) => {
    const bundle = watchBundle(page);
    let splits = 0;
    await fabricateOverflowPending(page, app.taskId);
    await page.route(`**/tasks/${app.taskId}/split`, async (route) => {
      splits += 1;
      // 慢网络：响应挂 1.5s，这就是用户看不出反应、想补点第二下的那个窗口
      await new Promise((resolve) => setTimeout(resolve, 1500));
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ task: { id: app.taskId } }),
      });
    });

    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);

    // 待办是 context_overflow → 档案盒里出现这两颗旁路动作
    const splitAction = page.getByRole('button', { name: '拆分任务', exact: true });
    await expect(splitAction).toBeVisible({ timeout: 60_000 });
    await splitAction.click();

    const dialog = page.getByRole('dialog', { name: '拆分任务' });
    await expect(dialog).toBeVisible();
    await dialog.locator('textarea').fill('实现 A 部分 | 说明');
    const submit = dialog.getByRole('button', { name: '确认拆分' });
    await expect(submit).toBeEnabled();

    await submit.click();

    // ① 进提交中态：按钮禁用并转圈（修复前这一格恒为 enabled）
    await expect(submit).toBeDisabled();
    await expect(submit.locator('.spin')).toHaveCount(1);

    // ② 连点第二次：不产生第二个请求
    await submit.click({ force: true }).catch(() => undefined);
    await page.waitForTimeout(2500);

    expect(splits, '连点两次只能有一个 POST /split').toBe(1);
    expectBundleHealthy(bundle);
  });

  test('换模型对话框同样进提交中态（同一个口径，不是只修了拆分那一处）', async ({ page }) => {
    const bundle = watchBundle(page);
    let overrides = 0;
    await fabricateOverflowPending(page, app.taskId);
    await page.route(`**/tasks/${app.taskId}/model-override`, async (route) => {
      overrides += 1;
      await new Promise((resolve) => setTimeout(resolve, 1500));
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ task: { id: app.taskId } }),
      });
    });

    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);

    const openAction = page.getByRole('button', { name: '更换长上下文模型', exact: true });
    await expect(openAction).toBeVisible({ timeout: 60_000 });
    await openAction.click();

    const dialog = page.getByRole('dialog', { name: '更换长上下文模型' });
    await expect(dialog).toBeVisible();
    // harness 的临时 home 里播了一个可用 provider → 下拉有值、按钮可用
    const submit = dialog.getByRole('button', { name: '应用' });
    await expect(submit).toBeEnabled();

    await submit.click();
    await expect(submit).toBeDisabled();
    await expect(submit.locator('.spin')).toHaveCount(1);
    await submit.click({ force: true }).catch(() => undefined);
    await page.waitForTimeout(2500);

    expect(overrides, '连点两次只能有一个 POST /model-override').toBe(1);
    expectBundleHealthy(bundle);
  });
});
