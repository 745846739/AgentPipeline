/**
 * E2E · 拍板时不再在同一屏摆两份同样的 diff（票 08）。
 *
 * 任务卡在 `pending(merge_approval)` 等人拍板时，主区 Diff 页签渲染一份 diff、
 * 右栏档案盒又渲染一份——同一份 diff（同一对「返回修改 / 合入」）同屏两次，
 * 「哪个是真的」于是变成使用者必须思考的问题。本用例钉两件事：
 *
 * ① **去重只在「用户已经在 Diff 页签里」时发生**：档案盒收成不重复内容的状态摘要；
 *    不在 Diff 页签时它照旧内嵌 diff（那是既定设计，档案盒的存在理由是「不切页签也能拍板」）。
 * ② **动作行是红线，始终在**：六个 e2e 文件里的多处定位依赖它，它是被测合约的一部分。
 *    去掉的只是重复的那份 diff，不是动作行——用户不必先切页签就能拍板（用户故事 17）。
 *
 * 「一份 diff」的判据落在**用户看得见的东西**上：diff 正文里每个文件有一个标题
 * （`DiffView` 的文件块标题，可访问名就是文件路径），数它即可；不数 class、不数组件状态。
 *
 * 真 axum 后端 + mock LLM + 临时 home；页面加载编译期内嵌的真实 bundle；只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  pendingTypeOf,
  settleBundle,
  startApp,
  waitForTask,
  watchBundle,
  type App,
} from './harness';
import { fullPassScript } from './scripts';

test.describe('前端 E2E：档案盒不重复渲染 diff（票 08）', () => {
  let app: App;

  test.beforeAll(async () => {
    // fullPass 一路推到 merge 阶段 A 末尾的 pending(merge_approval) 就停住等人拍板，
    // 本用例全程不点「合入」，所以整条用例里它一直停在那一态。
    app = await startApp({ script: fullPassScript('E2E'), title: 'E2E dossier dedupe' });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('停在 Diff 页签只有一份 diff 且动作行仍在；切走则档案盒内嵌 diff 回来', async ({
    page,
  }) => {
    // 真实产物守卫（主流程票 01）。
    const bundle = watchBundle(page);

    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);

    const dossier = page.locator('aside.dossier');
    await expect(dossier).toBeVisible({ timeout: 60_000 });
    await expect(dossier.locator('.dtag')).toContainText('合并提案');

    // ── ① 默认停在时间线页签：档案盒内嵌 diff 按原样出现（既定设计，不动） ──
    const dossierFiles = dossier.getByRole('heading', { level: 3 });
    await expect(dossierFiles.first()).toBeVisible({ timeout: 30_000 });
    const fileCount = await dossierFiles.count();
    expect(fileCount, '合并提案的 diff 里应有可数的文件块').toBeGreaterThan(0);

    // ── ② 切到 Diff 页签：主区一份、右栏零份；动作行照旧在右栏 ──
    await page.locator('.tabs button.tab', { hasText: 'Diff' }).click();
    const pane = page.locator('.pane');
    await expect(pane.getByRole('heading', { level: 3 }).first()).toBeVisible({ timeout: 30_000 });
    // 屏上只有这一份：主区同一份 diff 的文件块数与刚才档案盒里的一样多，右栏一个不剩
    await expect(pane.getByRole('heading', { level: 3 })).toHaveCount(fileCount);
    await expect(dossier.getByRole('heading', { level: 3 })).toHaveCount(0);

    // 动作行是红线：右栏始终能拍板，不必先切回别的页签（用户故事 17）
    await expect(dossier.getByRole('button', { name: '合入' })).toBeVisible();
    await expect(dossier.getByRole('button', { name: '返回修改' })).toBeVisible();

    // ── ③ 切回时间线：档案盒内嵌 diff 回来，动作行还在 ──
    await page.locator('.tabs button.tab', { hasText: '时间线' }).click();
    await expect(dossier.getByRole('heading', { level: 3 })).toHaveCount(fileCount);
    await expect(dossier.getByRole('button', { name: '合入' })).toBeVisible();

    // ── ④ 移动款（<480px）：底部动作坞不受影响 ──
    // 坞里本来就只有动作、没有 diff 正文（`actionsOnly`），去重不该把它改坏。
    await page.setViewportSize({ width: 430, height: 900 });
    await page.reload();
    await settleBundle(page, bundle);
    const dock = page.locator('aside.dock');
    await expect(dock).toBeVisible({ timeout: 30_000 });
    await expect(dock.getByRole('button', { name: '合入' })).toBeVisible();
    await expect(dock.getByRole('heading', { level: 3 })).toHaveCount(0);
  });
});
