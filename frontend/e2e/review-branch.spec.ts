/**
 * E2E-⑥ 人工评审分支 + 合并「返回修改」（主流程票 06）。
 *
 * 两条正常分支此前浏览器侧零覆盖：
 *   ① `review_mode = human`：任务停在 `pending(human_review)`，dossier 渲染人工评审
 *      三件套（diff / agent 预审报告 / 单测报告），approve / reject 经 `endpointFor`
 *      都打到 `POST /review` 但结论相反——同一 pendingType 分支最易接错处；
 *   ② `merge_approval` 的「返回修改」：与「合入」共用 `/merge/decision`、语义相反。
 *
 * 每条用例独占一套 app（脚本轮按 (stage,node) 指针消费，reject/return 的重入
 * 依赖第 2 轮，不能与其他任务共享 mock）。只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  expectBundleHealthy,
  pendingTypeOf,
  settleBundle,
  startApp,
  waitForTask,
  waitForTaskById,
  watchBundle,
  type App,
} from './harness';
import { clickConfirmed } from './confirm';
import { humanReviewRounds, mergeReturnRounds } from './scripts';

const REJECT_COMMENT = '打回标记-hr2：请补边界用例';

test.describe('前端 E2E ⑥：人工评审与返回修改', () => {
  test('① human 模式：评审面板三件套 + 通过 → 推进', async ({ page }) => {
    const bundle = watchBundle(page);
    const app = await startApp({
      script: humanReviewRounds('E2E'),
      title: 'E2E human approve',
      reviewMode: 'human',
    });
    try {
      await waitForTask(app, (t) => pendingTypeOf(t) === 'human_review', 'human_review');

      await page.goto(`${app.webBase}/#/task/${app.taskId}`);
      await settleBundle(page, bundle);

      // 人工评审面板：预审报告 + diff 都可见（单测报告在 review 之前属正常缺省）
      await expect(page.getByText('人工评审（review_mode = human）')).toBeVisible();
      await expect(page.getByText('agent 预审报告')).toBeVisible();
      await expect(page.locator('.reviewform')).toContainText('评审报告');

      // 抓 /review 请求：approved = true（与 reject 同端点、结论相反——防接错的关键断言）
      const reviewResp = page.waitForResponse(
        (r) => r.request().url().includes('/review') && r.request().method() === 'POST',
      );
      await page.getByRole('button', { name: '通过' }).click();
      const review = await reviewResp;
      expect(await review.request().postDataJSON()).toMatchObject({ approved: true });

      // 离开 human_review：清 pending 后一路推进到 merge_approval（test 轮消费）
      await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');

      expectBundleHealthy(bundle);
    } finally {
      await app.stop();
    }
  });

  test('② human 模式：打回附意见 → 回 develop → 再评审通过', async ({ page }) => {
    const bundle = watchBundle(page);
    const app = await startApp({
      script: humanReviewRounds('E2E'),
      title: 'E2E human reject',
      reviewMode: 'human',
    });
    try {
      await waitForTask(app, (t) => pendingTypeOf(t) === 'human_review', 'human_review');

      await page.goto(`${app.webBase}/#/task/${app.taskId}`);
      await settleBundle(page, bundle);

      const reviewResp = page.waitForResponse(
        (r) => r.request().url().includes('/review') && r.request().method() === 'POST',
      );
      await page
        .locator('.reviewform textarea')
        .fill(REJECT_COMMENT);
      await page.getByRole('button', { name: '打回并附意见' }).click();
      const review = await reviewResp;
      expect(await review.request().postDataJSON()).toMatchObject({
        approved: false,
        comments: REJECT_COMMENT,
      });

      // 打回 develop：mock 秒级重跑到二次 human_review，轮询中间态必竞态——
      // 用流转时间线断言（reject 落一条指向 develop 的流转）
      await waitForTask(app, (t) => pendingTypeOf(t) === 'human_review', '二次 human_review');
      const flowAfterReject = (await (
        await fetch(`${app.apiBase}/tasks/${app.taskId}/flow`)
      ).json()) as { transitions: Array<{ to_stage: string }> };
      expect(
        flowAfterReject.transitions.some((tr) => tr.to_stage === 'develop'),
        'reject 应落一条指向 develop 的流转',
      ).toBe(true);
      await page.reload();
      await settleBundle(page, bundle);
      await page.getByRole('button', { name: '通过' }).click();
      await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');

      // 打回意见随结论进流转原因（`GET /tasks/{id}/flow` 的 transitions）
      const flow = (await (
        await fetch(`${app.apiBase}/tasks/${app.taskId}/flow`)
      ).json()) as { transitions: Array<{ reason: string | null }> };
      expect(
        flow.transitions.some((tr) => (tr.reason ?? '').includes(REJECT_COMMENT)),
        '打回意见应出现在流转原因',
      ).toBe(true);

      expectBundleHealthy(bundle);
    } finally {
      await app.stop();
    }
  });

  test('③ merge「返回修改」→ 回 develop → 二次推进 → 合入到 done', async ({ page }) => {
    const bundle = watchBundle(page);
    const app = await startApp({
      script: mergeReturnRounds('E2E'),
      title: 'E2E merge return',
    });
    try {
      await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');

      await page.goto(`${app.webBase}/#/task/${app.taskId}`);
      await settleBundle(page, bundle);

      // 抓 /merge/decision：decision = return（与 approve 同端点、语义相反）
      const returnResp = page.waitForResponse(
        (r) => r.request().url().includes('/merge/decision') && r.request().method() === 'POST',
      );
      await page.getByRole('button', { name: '返回修改' }).click();
      const decision = await returnResp;
      expect(await decision.request().postDataJSON()).toMatchObject({ decision: 'return' });

      // 打回 develop：mock 秒级就会重跑并再次抵达 merge_approval，轮询中间态必竞态——
      // 改用流转时间线断言（user_resume = 用户 return 决策的落库证据，决策 79）
      const flowAfterReturn = (await (
        await fetch(`${app.apiBase}/tasks/${app.taskId}/flow`)
      ).json()) as { transitions: Array<{ trigger: string; to_stage: string }> };
      expect(
        flowAfterReturn.transitions.some(
          (tr) => tr.trigger === 'user_resume' && tr.to_stage === 'develop',
        ),
        'return 应落一条指向 develop 的 user_resume 流转',
      ).toBe(true);

      // 二次推进（消费第 2 轮）→ merge_approval → 合入 → done
      await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', '二次 merge_approval');
      const approveResp = page.waitForResponse(
        (r) => r.request().url().includes('/merge/decision') && r.request().method() === 'POST',
      );
      await clickConfirmed(page.locator('aside.dossier').getByRole('button', { name: /合入/ }));
      expect((await approveResp).status()).toBe(200);
      await waitForTask(
        app,
        (t) => t.status === 'done',
        '合入到 done',
        60_000,
      );

      expectBundleHealthy(bundle);
    } finally {
      await app.stop();
    }
  });
});
