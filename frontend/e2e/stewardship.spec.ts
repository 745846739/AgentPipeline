/**
 * E2E · 任务级托管的界面开关（决策 210① / 票 14 的差额）。
 *
 * 这一条补的是票 14 记的那个缺口：托管此前**只有端点 + 回读**（`curl` 可开、值班长提议的
 * 确认钮也可开），界面上看不见状态、也拨不动。现在任务上摆一颗钮，本用例钉三件事：
 *
 * ① **摆得出来**：任务停在 pending（托管最有用的那一态）时那颗钮在，且初始是关的；
 * ② **拨得动**：按一下之后按钮与说明都变，且**库里的那一列真的变了**（直接问后端，
 *    不看界面自己拼的本地态）；
 * ③ **拨得回去**：再按一下恢复未托管。
 *
 * 「终态任务不摆」「值班长未接线不摆」两条判据由 `src/lib/stewardship.test.ts` 逐条钉住
 * ——那两条在 e2e 里构造成本高（前者要跑到 done，后者要一份没接线的运行），
 * 而它们是**纯判据**，单元层是它们该在的地方。
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

/** 直接问后端要托管那一列（界面说的与库里存的必须是同一件事）。 */
async function stewardshipEnabled(app: App): Promise<boolean> {
  const res = await fetch(`${app.apiBase}/tasks/${app.taskId}`);
  expect(res.ok).toBeTruthy();
  const body = (await res.json()) as { task: { stewardship: { enabled: boolean } | null } };
  return body.task.stewardship?.enabled ?? false;
}

test.describe('前端 E2E：托管开关（票 14 / 决策 210①）', () => {
  let app: App;

  test.beforeAll(async () => {
    // 空脚本：任务必然失败停在 pending——那正是托管最有用的那一态，也顺带避开终态
    // （终态任务上后端拒开托管，界面上也确实不该摆那颗钮）。
    app = await startApp({ script: {}, title: 'E2E 托管' });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('任务上摆得出、拨得动，且与库里的那一列一致', async ({ page }) => {
    const bundle = watchBundle(page);

    await waitForTask(app, (t) => pendingTypeOf(t) !== null, 'pending');
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);

    // ① 摆得出来，且初始是关的：说明那一行要说清「关着」意味着什么（只能提议、动手要按键）
    const btn = page.getByRole('button', { name: /托管/ });
    await expect(btn).toBeVisible({ timeout: 30_000 });
    await expect(btn).toHaveAttribute('aria-pressed', 'false');
    const note = page.locator('.steward .s-note');
    await expect(note).toContainText('按键');
    expect(await stewardshipEnabled(app), '初始不该是托管中').toBe(false);

    // ② 拨开：按钮与说明都跟着变，且**库里那一列真的开了**（界面不自己拼本地态）
    await btn.click();
    await expect(btn).toHaveAttribute('aria-pressed', 'true');
    await expect(btn).toContainText('托管：开');
    await expect
      .poll(() => stewardshipEnabled(app), { timeout: 15_000, message: '托管没真的开起来' })
      .toBe(true);
    // 开着时的说明要说清「它现在能免按键做什么」——这是那个授权的全部内容（决策 210②）
    await expect(note).toContainText('免按键');

    // ③ 拨回去：关掉是**清空那一列**（决策 210①），故回读应为 null
    await btn.click();
    await expect(btn).toHaveAttribute('aria-pressed', 'false');
    await expect
      .poll(() => stewardshipEnabled(app), { timeout: 15_000, message: '托管没真的关掉' })
      .toBe(false);

    // 归零之后再确认一次那颗钮**还在**（关掉不等于消失）
    await expect(btn).toBeVisible();

    expectBundleHealthy(bundle);
  });
});
