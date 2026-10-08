/**
 * UX 第二轮 · 票 12 / 13 / 17：超时与失败说得出口、断线看得见、长值不撑破。
 *
 * 三条都靠**注入失败**取证，不靠肉眼：
 *   - 请求挂住不回（`page.route` 不 handle）→ 超时兜底必须给出可读错误并复位；
 *   - 命令输出接口 500 → 面板说失败，不再永远「正在加载完整输出…」；
 *   - 掐断 SSE → 详情页出现断线指示；此时点动作有「流未连通」的反馈，而不是静默等 30 秒。
 *
 * 真 axum 后端 + mock LLM + 临时 home；只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  expectBundleHealthy,
  expandSceneTurns,
  pendingTypeOf,
  settleBundle,
  startApp,
  waitForTaskById,
  watchBundle,
  type App,
} from './harness';
import { clickConfirmed } from './confirm';
import {
  NODE,
  ValidateInput,
  designRounds,
  fullPassScript,
  implementationRounds,
  submit,
  type NodeScript,
} from './scripts';

/** 一个长得离谱的探测路径：它的**尾巴**才是区别（`final-artifact.md`）。 */
const LONG_PATH = `/Users/somebody/code/${'nested/'.repeat(24)}final-artifact.md`;

/**
 * 「等人补一句话」的任务：先停在 `pending(info_insufficient)`，resume 之后重入 archVI
 * 消费第二轮、走完余下流水线。
 *
 * 用例 ⑤ 要的正是一次**在页面开着时发生的** pending 迁移（那才会弹 toast），
 * 而迁移由用例自己按一下「补充信息并继续」触发——不靠 sleep 赌时序。
 *
 * 第二次停靠在哪个 pending 上**随整份用例的运行次序而变**（实测两种都出现过）：
 * 同项目的兄弟任务若已合入，脚本在 test 节点用尽 → `retry_exhausted@test.execute`；
 * 若那一刻它仍占着同一批 `affected_files` → `conflict_wait`。两种都是后台的正当行为，
 * 与本用例要量的东西（toast 与动作坞的叠放）无关，所以 ⑤ 只断言**几何**，
 * 不认具体是哪一个 pending。「同名动作不是一个动作」那条回归由组件级用例确定性钉住
 * （`components/board/PendingActions.test.ts`，撞 key 会直接抛）。
 */
function resumableScript(taskId: string): NodeScript {
  return {
    ...designRounds(),
    ...implementationRounds(taskId),
    [NODE.archVI]: [[submit(ValidateInput(false, ['缺少技术约束']))], [submit(ValidateInput(true))]],
  };
}

test.describe('UX2 ⑨ 超时 / 断线 / 长值（票 12 / 13 / 17）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({
      script: {
        ...fullPassScript('UX2R'),
        // 长 metadata 值：`submit_metadata` 的原文会进会话里的元数据卡
        [NODE.archVI]: [[submit({ ...ValidateInput(true), probe_path: LONG_PATH })]],
      },
      title: 'UX2 韧性',
      additionalTasks: [{ title: 'UX2 等人补话', script: resumableScript('UX2PEND') }],
    });
    await waitForTaskById(
      app.taskIds[0],
      app,
      (t) => pendingTypeOf(t) === 'merge_approval',
      'merge_approval',
      180_000,
    );
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('① 命令输出接口 500：面板说失败，不再永远「正在加载」', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.route('**/commands/*/output', (route) =>
      route.fulfill({ status: 500, contentType: 'text/plain', body: 'injected failure' }),
    );

    await page.goto(`${app.webBase}/#/task/${app.taskIds[0]}`);
    await settleBundle(page, bundle);
    // 现场页签（真 tablist；决策 349：命令回执住在现场时间线里）
    await page.getByRole('tab', { name: /现场/ }).click();
    // 轮默认收起（决策 366）：正文在轮里，先展开才进视口
    await expect(page.locator('article.turn details.turnfold').first()).toBeVisible({
      timeout: 30_000,
    });
    await expandSceneTurns(page);

    const firstCmd = page.locator('.rcpt.cmd').first();
    await expect(firstCmd).toBeVisible({ timeout: 30_000 });
    await firstCmd.locator('summary').click();

    const alert = page.locator('.rcpt-more.ferr');
    await expect(alert).toBeVisible({ timeout: 30_000 });
    await expect(alert).toContainText('完整输出没读回来');
    expect(await page.locator('.rcpt-more', { hasText: '正在加载完整输出' }).count()).toBe(0);
    expectBundleHealthy(bundle);
  });

  test('② 长 metadata 值不把页面撑出横向滚动；diff 的文件路径可悬停看全', async ({ page }) => {
    const bundle = watchBundle(page);
    // 900 宽：这一档主栏约 560px，长值若不给换行点就会顶出去
    await page.setViewportSize({ width: 900, height: 1000 });
    await page.goto(`${app.webBase}/#/task/${app.taskIds[0]}`);
    await settleBundle(page, bundle);

    await page.getByRole('tab', { name: /现场/ }).click();
    // 现场没有「选中 run」：进页签即批量装载完整会话（决策 349），等那一轮的名牌出现
    const turn = page.locator('article.turn', { hasText: 'validate_input' }).first();
    await expect(turn).toBeVisible({ timeout: 30_000 });
    // 名牌（`stage · node`）落在折叠之外、元数据卡在折叠之内（决策 366）：长值要看得见才测得出
    await expandSceneTurns(page);

    // 元数据卡里那条长路径整条在 DOM 里（没被截断成省略号）
    await expect(page.getByText(LONG_PATH, { exact: false }).first()).toBeVisible({
      timeout: 30_000,
    });
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(overflow, '页面上不该出现横向滚动（长值要能自己换行）').toBeLessThanOrEqual(0);

    // 截断的那些值能悬停看全（票 17 / R2-22）：diff 的文件路径
    await page.getByRole('tab', { name: /^Diff$/ }).click();
    const path = page.locator('.dfile h3 .path').first();
    await expect(path).toBeVisible({ timeout: 30_000 });
    expect(await path.getAttribute('title')).toBeTruthy();
    expectBundleHealthy(bundle);
  });

  test('③ 请求挂住不回：超时兜底给出可读错误，重试钮仍可用', async ({ page }) => {
    const bundle = watchBundle(page);
    // 不 handle = 一直挂着（模拟「TCP 连上但不回包」）
    await page.route('**/providers', () => undefined);

    await page.goto(`${app.webBase}/#/settings/providers`);
    await settleBundle(page, bundle);
    await expect(page.locator('.banner', { hasText: '正在加载 provider' })).toBeVisible();

    // 默认超时 30s：等它自己认栽，而不是永远转圈
    const alert = page.locator('.banner.error[role=alert]');
    await expect(alert).toBeVisible({ timeout: 60_000 });
    await expect(alert).toContainText('超时');
    // 按钮复位：能再按（不是一直禁着）
    const retry = page.getByRole('button', { name: '重试', exact: true });
    await expect(retry).toBeEnabled();
    expectBundleHealthy(bundle);
  });

  test('④ 掐断 SSE：详情页出现断线指示；此时点动作有「流未连通」的反馈', async ({ page }) => {
    const bundle = watchBundle(page);
    let blocked = true;
    // 数一数重连之后有没有**真的对齐过一次**（票 13 的验收含「内容恢复」，不只是「横幅消失」）：
    // 详情页的重连路径是 `onRecalibrate` → 全量 `GET /tasks/{id}`（决策 76 的唯一状态入口）。
    let refetches = 0;
    await page.route(`**/tasks/*/stream`, async (route) => {
      if (blocked) return route.abort();
      return route.continue();
    });
    await page.route(`**/tasks/${app.taskIds[0]}`, async (route) => {
      if (route.request().method() === 'GET') refetches += 1;
      return route.continue();
    });

    await page.goto(`${app.webBase}/#/task/${app.taskIds[0]}`);
    await settleBundle(page, bundle);

    // 断线指示（与看板同一句话）
    const banner = page.locator('.banner[role=status]', { hasText: '实时流已断开' });
    await expect(banner).toBeVisible({ timeout: 30_000 });

    // 流没连着时点「合入」：动作发出去了，但要**说出来**「回执要等重连」，
    // 而不是静默等 30 秒（票 13 / R2-15）。`合入` 是 destructive 档 → 两步确认
    // （决策 216②）：第一下只亮后果句，第二下才发出请求。
    await clickConfirmed(page.locator('aside.dossier').getByRole('button', { name: '合入' }));
    await expect(page.locator('.banner[role=status]', { hasText: '实时流未连通' })).toBeVisible({
      timeout: 30_000,
    });

    // 放行之后重连：指示消失**且内容真的对齐一次**
    blocked = false;
    const before = refetches;
    await expect(banner).toBeHidden({ timeout: 90_000 });
    await expect
      .poll(() => refetches, { timeout: 60_000 })
      .toBeGreaterThan(before);
    // 正文还在（对齐不是把页面清空）
    await expect(page.locator('.d-title')).toBeVisible();
    expectBundleHealthy(bundle);
  });

  test('⑤ 430 宽：待办迁移弹出的 toast 在顶部，不压住动作坞的按钮', async ({ page }) => {
    const bundle = watchBundle(page);
    const pendingTask = app.taskIds[1];
    await page.setViewportSize({ width: 430, height: 932 });
    await page.goto(`${app.webBase}/#/task/${pendingTask}`);
    await settleBundle(page, bundle);

    // 停在 info_insufficient：动作坞在（移动款的底部件），输入框要补一句话
    await expect(page.locator('.dock')).toBeVisible({ timeout: 60_000 });
    // 决策 281 起 pending 坞默认收成一行手柄，动作面要点开才在——先按手柄展开
    const dockHead = page.locator('.dock').getByRole('button', { name: /等你拍板/ });
    await expect(dockHead).toBeVisible({ timeout: 30_000 });
    await dockHead.click();
    await expect(dockHead).toHaveAttribute('aria-expanded', 'true');
    const box = page.locator('.dock textarea');
    await expect(box).toBeVisible({ timeout: 30_000 });
    await box.fill('技术约束：仅 Chromium，单机本地运行。');
    await page.locator('.dock').getByRole('button', { name: /补充信息并继续/ }).click();

    // resume 之后它会再进一次 pending（重试耗尽）→ 页面开着，于是弹一条 toast
    const toast = page.locator('.toast').first();
    await expect(toast).toBeVisible({ timeout: 120_000 });
    // 坞回到屏上（新的 pending）
    await expect(page.locator('.dock')).toBeVisible({ timeout: 60_000 });

    // 新一轮 pending 重新收起（决策 281），先按手柄展开——下面量的是**动作面**的几何，
    // 展开层的按钮（`cancel` 恒在）才是「不压住动作坞的按钮」要保的东西。
    // 重试链里 pending 会进出几轮、坞随之重挂载（展开态随之收起），故点到「真的展开」为止。
    const head2 = page.locator('.dock').getByRole('button', { name: /等你拍板/ });
    await expect(head2).toBeVisible({ timeout: 60_000 });
    await expect(async () => {
      await head2.click();
      await expect(head2).toHaveAttribute('aria-expanded', 'true', { timeout: 2_000 });
    }).toPass({ timeout: 60_000 });

    // 坞里至少有一颗能按的钮（任何 pending 都有：`cancel` 恒在），下面的命中测试才成立
    const dockEl = page.locator('.dock');
    await expect(dockEl.getByRole('button').first()).toBeVisible({ timeout: 60_000 });

    const placements = await page.evaluate(() => {
      const toasts = document.querySelector('.toasts') as HTMLElement | null;
      const dock = document.querySelector('.dock') as HTMLElement | null;
      if (!toasts || !dock) return null;
      const t = toasts.getBoundingClientRect();
      const d = dock.getBoundingClientRect();
      return {
        toastsBottom: Math.round(t.bottom),
        dockTop: Math.round(d.top),
        overlap: Math.round(Math.max(0, Math.min(t.bottom, d.bottom) - Math.max(t.top, d.top))),
      };
    });
    expect(placements).not.toBeNull();
    expect(placements!.overlap, 'toast 与动作坞不该重叠').toBe(0);
    expect(placements!.toastsBottom).toBeLessThanOrEqual(placements!.dockTop);

    // 坞的按钮仍然按得到（点动作的位置不会跳去另一个任务）
    const hit = await page.evaluate(() => {
      const btn = document.querySelector('.dock button') as HTMLElement | null;
      if (!btn) return null;
      const r = btn.getBoundingClientRect();
      const el = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return { same: el === btn || btn.contains(el as Node) };
    });
    expect(hit?.same, '坞按钮的中心点必须命中坞按钮自己').toBe(true);

    // 截断的那条消息能悬停看全（`title` 与可见文字同值）
    expect(await toast.locator('.title').getAttribute('title')).toBeTruthy();

    // 票 05（ux-audit-3）：真页面上的 Escape 关闭路径——审计 [r3] ⑤.1 因 harness 那一刻
    // 没弹出 toast 而标「运行时未验证」，这条把缺口闭合：焦点在关闭钮（toast 归属域内）
    // → 按 Escape → 这一条从 DOM 消失（归属守卫的另一半「焦点在外不动」由组件级
    // ToastStack.test.ts 钉住）。
    const toastId = await toast.getAttribute('data-toast-id');
    expect(toastId, 'toast 应当带 data-toast-id（稳定定位器）').not.toBeNull();
    await toast.locator('button.close').focus();
    await page.keyboard.press('Escape');
    await expect(page.locator(`.toast[data-toast-id="${toastId}"]`)).toHaveCount(0);

    expectBundleHealthy(bundle);
  });
});

/**
 * 解锁恢复链（票 04，stream-self-heal spec）：锁屏解锁（= 可见性恢复）之后——
 * 流必须重开；可见性回调里那次校准**失败**也必须有人兜（重连成功后补的第二次校准，
 * 票 03 的牙齿）；对讲台重连成功后必须补一次 reload（onRecalibrate 接线的牙齿）。
 *
 * 看门狗不进 e2e（45 秒太慢），它归连接层单测（缝 A，connection.test.ts）。
 * 断言落点是**请求数**而非横幅显隐：详情页没有周期 tick 兜底，内容恢复只可能来自校准。
 */
test.describe('解锁恢复链（票 04，stream-self-heal）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('UNLOCK'), title: '解锁恢复' });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('⑥ 详情页：首个校准失败也有人兜——流重开 + 第二次校准到达 + 内容仍正确', async ({
    page,
  }) => {
    const bundle = watchBundle(page);
    let refetches = 0;
    let streamOpens = 0;
    let failNextRefetch = false;
    await page.route('**/tasks/*/stream', async (route) => {
      streamOpens += 1;
      return route.continue();
    });
    await page.route(`**/tasks/${app.taskIds[0]}`, async (route) => {
      if (route.request().method() !== 'GET') return route.continue();
      // 只让解锁后「可见性回调里的那一次」失败：它模拟解锁瞬间网络还没醒
      if (failNextRefetch) {
        failNextRefetch = false;
        return route.abort();
      }
      refetches += 1;
      return route.continue();
    });

    await page.goto(`${app.webBase}/#/task/${app.taskIds[0]}`);
    await settleBundle(page, bundle);
    await expect(page.locator('.d-title')).toBeVisible({ timeout: 30_000 });
    await expect.poll(() => streamOpens, { timeout: 30_000 }).toBeGreaterThan(0);

    const refetchBefore = refetches;
    const streamBefore = streamOpens;
    failNextRefetch = true;
    // 模拟解锁：visibilityState 本来就是 visible，事件本身是唤醒信号
    await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));

    // ① 流被重开（reconnectNow 中断旧连接、立刻新开一条）
    await expect.poll(() => streamOpens, { timeout: 30_000 }).toBeGreaterThan(streamBefore);
    // ② 首次校准按计划失败，但重连成功后的补校准必须到达——没有票 03，
    //    这里会等到超时（主动重连不置「断过线」，系统以为没错过）
    await expect.poll(() => refetches, { timeout: 30_000 }).toBeGreaterThan(refetchBefore);
    // ③ 内容最终正确：失败的那次校准若清过现场，补校准把它拉了回来
    await expect(page.locator('.d-title')).toBeVisible({ timeout: 30_000 });
    expectBundleHealthy(bundle);
  });

  test('⑦ 对讲台：解锁后流重连成功，必须补一次 reload（onRecalibrate 接线的牙齿）', async ({
    page,
  }) => {
    const bundle = watchBundle(page);
    let sessionReads = 0;
    // **正则而不是 glob**：Playwright 的 glob 是对**整条 URL（含查询串）**匹配的
    // （`urlMatch.js`：`isRegExp(match) ? match.test(url) : globToRegex(...)`），
    // 而票 04 之后这一页读的是 `/foreman/session?session=…&kind=talk`——`**/foreman/session`
    // 因此一条也拦不到（本用例第一版就是这么红的：读数恒 0）。
    await page.route(/\/foreman\/session(\?|$)/, async (route) => {
      if (route.request().method() === 'GET') sessionReads += 1;
      return route.continue();
    });

    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect.poll(() => sessionReads, { timeout: 30_000 }).toBeGreaterThan(0);

    const before = sessionReads;
    await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));

    // 可见性回调自己 reload 一次（+1）；接了 onRecalibrate 之后重连成功再补一次（+2）。
    // 没接线只有 +1——这条用例的牙齿。安静班次没有轮询会来搅局（轮询只在真跟随时走）。
    await expect
      .poll(() => sessionReads, { timeout: 30_000 })
      .toBeGreaterThanOrEqual(before + 2);
    expectBundleHealthy(bundle);
  });
});
