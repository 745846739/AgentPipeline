/**
 * 前端 E2E ⑪：对讲台的**停钮**（决策 294 / 票 09）。
 *
 * 验的是**外部行为**：这一颗钮只属于「这一轮」——空闲时不渲染、在飞时出现在发送那颗钮的
 * 位置上、按下即转「正在停…」、这一轮收口之后退场；而按下的效果落在台账上：那一行带
 * `【已停】`。另有一条钉**撤回**：服务端回 `cancelled: false`（这一班此刻没有可停的一轮）
 * 时那颗钮退回可按的「停」，不钉在「正在停…」上假装。
 *
 * **装置**：脚本那一条 `text(..., { delayMs })`——回话拖 15 秒才发出去，于是「这一轮在飞」
 * 有一段**可观测的窗口**（与决策 260 那条刷新用例同一件装置）。用例在那段窗口里按停：
 * 它若真停下来，收口会发生在**远早于** 15 秒的地方（下面那条 8 秒超时就是这根牙齿）。
 *
 * 那 15 秒里**正在跑的那一次调用**就是它要打断的东西（core 侧的 `select!`，决策 294 的
 * 第②个观察点）：按停之后这一轮在一秒上下就收了口——那条 8 秒的超时因此不只是防慢，
 * 它同时是「打断的确实是**在飞的那一次调用**」这根牙齿（等 mock 自己回来要 15 秒）。
 * 同一个机制由 core 的集成用例再钉一遍（`pressing_stop_ends_the_human_turn_and_keeps_its_proposals`：
 * 替身永不返回，只有停钮解得开它）。
 *
 * 脚本铺**两轮**、每轮各自 `text(..., 15_000)`：一条用例用一轮（脚本耗尽时第二次说话
 * 拿到的是「脚本已结束」那句立刻返回的收尾话，在飞窗口就没了）。
 */

import { expect, test } from '@playwright/test';
import { expectBundleHealthy, settleBundle, startApp, watchBundle, type App } from './harness';
import { foremanScript, text } from './scripts';

test.describe('对讲台 · 停钮（决策 294）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({
      // 两轮都在飞：每一轮回话都要 15 秒后才发出去，用例在它回来之前按停。
      // 注意 `text()` 的第二个参数就是 `delayMs`（位置参数，不是 options 对象）。
      script: foremanScript([
        [text('（这一句本该 15 秒后才回——若它出现了，说明按停没生效）', 15_000)],
        [text('（第二轮的同一句话：给撤回那条用例一副同样的在飞窗口）', 15_000)],
      ]),
      providerOnly: true,
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('只在这一轮在飞时出现；按下去这一轮就停，台账那一行带【已停】', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    // ① 空闲：没有这一颗钮（它只属于「这一轮」），发送那颗钮照常在那儿
    await expect(page.locator('[data-stop="turn"]')).toHaveCount(0);
    await expect(page.locator('.typer button[type=submit]')).toHaveCount(1);

    await page.locator('.typer textarea').fill('停一下试试');
    await page.locator('.typer button[type=submit]').click();

    // ② 在飞：**发送那颗钮的位置**上换成「停」（同格同尺寸——一轮没落地发不出第二句）
    const stop = page.locator('[data-stop="turn"]');
    await expect(stop).toHaveText('停', { timeout: 10_000 });
    await expect(page.locator('.typer button[type=submit]')).toHaveCount(0);

    // ③ 按下。**不断言「正在停…」那一格**：它只是「按下 → 收口」之间的过场，而真机上
    // 收口是立刻发生的（cancel 打到服务端，后台那一轮在飞的调用当场被放弃）——那一格
    // 短到几十毫秒，在这里断言就是一条招摇的掷骰子。它是 `stopButton.test.ts` 的地盘
    // （纯函数三个态各一条），这里要验的是下一个断言：**它真的停了**。第②条用例把那
    // 一格钉在了「回包被按住」的确定性装置上。
    await stop.click();

    // ④ 它**真的**停了：收口在远早于那 15 秒的地方发生（这一条是按停的牙）
    const closed = page.locator('.timeline .turn.fm', { hasText: '【已停】' });
    await expect(closed).toHaveCount(1, { timeout: 8_000 });
    // ⑤ 收口之后那颗钮退场（一轮已经结束）
    await expect(page.locator('[data-stop="turn"]')).toHaveCount(0);
    await expect(page.locator('.typer button[type=submit]')).toHaveCount(1);

    // ⑥ 台账里那一行也带标记——不只是本地那一屏这么渲染
    const rows = await page.evaluate(async (apiBase) => {
      const res = await fetch(`${apiBase}/foreman/session`);
      const body = (await res.json()) as { messages: Array<{ role: string; content: string }> };
      return body.messages.filter((m) => m.role === 'assistant').map((m) => m.content);
    }, app.apiBase);
    expect(rows.some((c) => c.includes('【已停】')), `台账里要留痕：${JSON.stringify(rows)}`).toBe(
      true,
    );
    expectBundleHealthy(bundle);
  });

  /**
   * 服务端说「此刻没有可停的一轮」时，那颗钮**退回可按的「停」**。
   *
   * 这一支真机上抓不住：本机这一趟 POST 与服务端登记停钮通道之间有一段毫秒级的空隙
   * （票 09 的端点契约把那一段写明了——`cancelled: false` = 没有一轮可停，界面据此把那颗
   * 钮收回去）。抓不住的是那段**时间**，不是那条**分支**：把回包按住 → 它每次都会走到。
   * 按住之后先断言「正在停…」在场（否则这次点击压根没进那一态），再放行回包、断言它退回
   * 「停」——**撤掉实现里那一行，第二次断言就红**（钮会钉在「正在停…」上，而人的那一轮
   * 没有时间界，它可以一直跑下去）。
   */
  test('服务端说没有可停的一轮：那颗钮退回「停」，不钉在「正在停…」上', async ({ page }) => {
    let release: (() => void) | null = null;
    let held = false;
    await page.route(/\/foreman\/sessions\/[^/]+\/cancel$/, async (route) => {
      // 只按第一条：这一条用例只按一次停。
      if (!held) {
        held = true;
        await new Promise<void>((resolve) => {
          release = resolve;
        });
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ cancelled: false }),
      });
    });

    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    await page.locator('.typer textarea').fill('再停一下试试');
    await page.locator('.typer button[type=submit]').click();

    const stop = page.locator('[data-stop="turn"]');
    await expect(stop).toHaveText('停', { timeout: 10_000 });
    await stop.click();

    // 回包被按着：这一格是确定的「正在停…」（不是抢在收口前的几十毫秒）
    await expect(stop).toHaveText('正在停…', { timeout: 5_000 });
    // 等到「那一条请求真的到了被按住的这一支」再放行——`toHaveText` 说的是界面，
    // 而拦截发生在请求到达之后，两者之间还有一段网络时间。
    await expect.poll(() => held, { timeout: 5_000 }).toBe(true);

    // 放行：服务端答「没有可停的一轮」→ 钮退回可按的「停」
    release?.();
    await expect(stop).toHaveText('停', { timeout: 5_000 });
    // 那一轮**仍在飞**（这是撤回而不是收口：mock 那 15 秒还没到）
    await expect(page.locator('[data-stop="turn"]')).toHaveCount(1);
    expectBundleHealthy(bundle);
  });
});
