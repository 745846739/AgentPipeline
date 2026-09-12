/**
 * E2E-① happy path（票 18 / 决策 151）：看板 → 任务详情 → 页签切换 → diff 审批合入。
 *
 * 真 axum 后端 + mock LLM（FakeAgent 同一替换边界）+ 临时 home（决策 148）。
 * 只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import { startApp, waitForTask, pendingTypeOf, type App } from './harness';
import { fullPassScript } from './scripts';

test.describe('前端 E2E ①：happy path（看板 → 详情 → 页签 → diff 审批合入）', () => {
  let app: App;
  const title = 'E2E happy path';

  test.beforeAll(async () => {
    // 任务 id 在播种后才知道：先以占位脚本启动，再用任务 id 无关的脚本站位——
    // develop 的 CodeChanges.branch_name 由后端从游标取，脚本里的 branch_name 只是元数据，
    // 因此这里用固定前缀即可（真 git 提交命令里的 task id 也不影响流水线推进）。
    app = await startApp({ script: fullPassScript('E2E'), title });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('看板展示任务 → 进详情 → 切页签 → Diff 审批合入到 done', async ({ page }) => {
    // ── 看板：任务卡出现在看板上（真后端 seeding） ──
    await page.goto(`${app.webBase}/#/`);
    const card = page.locator('article.card', { hasText: title });
    await expect(card).toBeVisible({ timeout: 60_000 });

    // ── 详情：点卡进入 `#/task/{id}` ──
    await card.locator('a.card-link').click();
    await expect(page).toHaveURL(new RegExp(`#/task/${app.taskId}`));
    await expect(page.locator('h1.d-title')).toHaveText(title);

    // ── 页签切换：时间线 → 会话 → 命令与输出 → 产出文件 ──
    for (const label of ['[会话]', '[命令与输出', '[产出文件]']) {
      const tab = page.locator('nav.tabs button.tab', { hasText: label });
      await tab.click();
      await expect(tab).toHaveClass(/on/);
    }
    // 产出文件页签真的渲染了产物清单（真后端文件 API）
    await expect(page.locator('.fileview .list')).toContainText('design.md');

    // ── 等流水线推进到 pending(merge_approval)：dossier 琥珀面板出现 ──
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');
    const dossier = page.locator('aside.dossier');
    await expect(dossier).toBeVisible({ timeout: 60_000 });
    await expect(dossier.locator('.dtag')).toContainText('合并提案');

    // ── Diff 页签：切过去并点击「合入」 ──
    const diffTab = page.locator('nav.tabs button.tab', { hasText: '[Diff]' });
    await expect(diffTab).toBeEnabled({ timeout: 60_000 });
    await diffTab.click();
    // diff 面板在页签区与 dossier 各有一处（同一组件两处渲染），限定页签容器内那个
    await expect(page.locator('.pane .diffpanel').first()).toBeVisible();

    // 合入按钮（DiffReviewPanel 的 approve；决策 23：没有「拒绝」）
    const approve = dossier.locator('button.btn.solid', { hasText: '合入' });
    await expect(approve).toBeVisible();
    await approve.click();

    // ── 断言：任务到终态 done（合入真发生） ──
    await waitForTask(app, (t) => t.status === 'done', '任务 done', 120_000);
    await expect(page.locator('.terminal.done')).toBeVisible({ timeout: 60_000 });

    // 终态后顶栏待办计数归零（面板消失由 pending 状态驱动）
    await expect(page.locator('.pending-count .c')).toHaveText(/\*0/, { timeout: 30_000 });
  });
});
