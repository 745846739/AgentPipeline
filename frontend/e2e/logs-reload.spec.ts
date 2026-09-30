/**
 * E2E-⑦ 日志/对话内容可信 + 刷新恢复（主流程票 07）。
 *
 * 「看任务跑」的两件高频事此前只验到「页签能点」：
 *   ① 现场页签里要有**可识别的命令内容**（mock 脚本里的 git commit 串）；
 *   ② 现场页签要有**可断言的模型文本**（脚本 text 步骤），不是空面板（决策 349：两半并成一枚页签）；
 *   ③ 页面开着时任务**继续推进**（命令计数增长）——实时流真的到达界面，
 *      而不是「打开时的一次性快照」；
 *   ④ pending 时刷新：状态与 pending 面板正确恢复；网络断开→恢复后界面收敛到真值，
 *      且刷新后的动作（合入）照常生效（决策 153②③：fetch 流式 SSE，不用 EventSource）。
 */

import { expect, test } from '@playwright/test';

import {
  expectBundleHealthy,
  fetchTask,
  pendingTypeOf,
  settleBundle,
  startApp,
  waitForTask,
  watchBundle,
  type App,
} from './harness';
import { observabilityRounds } from './scripts';

test.describe('前端 E2E ⑦：日志对话可信 + 刷新恢复', () => {
  test('① 命令/对话内容非空可识别 + 页面开着时实时推进', async ({ page }) => {
    const bundle = watchBundle(page);
    const app = await startApp({
      script: observabilityRounds('E2E'),
      title: 'E2E observability',
    });
    try {
      // 尽早打开任务页：此时任务还在跑，后续推进必须经实时流到达界面
      await page.goto(`${app.webBase}/#/task/${app.taskId}`);
      await settleBundle(page, bundle);

      // ── 实时流：页面不刷新，现场计数从 0（或少量）涨到出现 git commit ──
      const tabButton = page.getByRole('tab', { name: /现场/ });
      await expect(tabButton).toBeVisible();
      await waitForTask(
        app,
        (t) => pendingTypeOf(t) === 'merge_approval',
        'merge_approval',
        120_000,
      );
      // 全程未刷新：计数徽章应已反映落库的命令（SSE live 路径）。
      // 像素主题（票 06 / 决策 169）：页签 = 工位标签盒，计数是页签内的 `.c` 徽章，
      // 定位按可访问名「现场」（无方括号）；断言徽章非 0，不删原断言语义。
      await expect(tabButton.locator('.c')).not.toHaveText('0');

      // ── 命令回执：展开那条 git 命令，输出可识别（决策 349：命令是现场轮里的回执）──
      await tabButton.click();
      const commandRow = page.locator('.rcpt.cmd', { hasText: 'git add -A' }).first();
      await expect(commandRow).toBeVisible({ timeout: 30_000 });
      await commandRow.locator('summary').click();
      await expect(commandRow).toContainText("commit -m 'feat: task", { timeout: 30_000 });

      // ── 会话轮：review.execute 的名牌下有脚本 text 步骤的原文 ──
      await expect(
        page.locator('article.turn', { hasText: '评审要点标记-obs7' }).first(),
      ).toBeVisible({ timeout: 30_000 });

      expectBundleHealthy(bundle);
    } finally {
      await app.stop();
    }
  });

  test('② pending 时刷新恢复 + 断网容错 + 刷新后动作生效', async ({ page }) => {
    const bundle = watchBundle(page);
    const app = await startApp({
      script: observabilityRounds('E2E'),
      title: 'E2E reload recovery',
    });
    try {
      await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');

      // ── 刷新：pending 面板与任务状态正确恢复 ──
      await page.goto(`${app.webBase}/#/task/${app.taskId}`);
      await settleBundle(page, bundle);
      await page.reload();
      await settleBundle(page, bundle);
      await expect(page.locator('aside.dossier')).toBeVisible();
      await expect(page.locator('aside.dossier')).toContainText('合并提案');

      // ── 断网→恢复：界面不白屏，恢复后收敛到真值 ──
      await page.context().setOffline(true);
      await expect(page.locator('body')).toBeVisible(); // 不白屏
      await expect(page.locator('aside.dossier')).toBeVisible(); // 已加载的 DOM 不丢
      await page.context().setOffline(false);
      await expect(page.locator('aside.dossier')).toContainText('合并提案', {
        timeout: 30_000,
      });

      // ── 刷新后的动作照常生效：合入 → done（SSE 重连后新推进反映到界面）──
      await page.locator('aside.dossier').getByRole('button', { name: /合入/ }).click();
      await waitForTask(app, (t) => t.status === 'done', '合入到 done', 60_000);
      await expect(
        page.locator('body').getByText(/done|已完成/).first(),
      ).toBeVisible({ timeout: 30_000 });

      expectBundleHealthy(bundle);
    } finally {
      await app.stop();
    }
  });
});
