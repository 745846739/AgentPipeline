/**
 * 前端 E2E ⑪：对讲台的**停钮**（决策 294 / 票 09）。
 *
 * 验的是**外部行为**：这一颗钮只属于「这一轮」——空闲时不渲染、在飞时出现在发送那颗钮的
 * 位置上、按下即转「正在停…」、这一轮收口之后退场；而按下的效果落在台账上：那一行带
 * `【已停】`。
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
 */

import { expect, test } from '@playwright/test';
import { expectBundleHealthy, settleBundle, startApp, watchBundle, type App } from './harness';
import { foremanScript, text } from './scripts';

test.describe('对讲台 · 停钮（决策 294）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({
      // 一轮在飞：回话要 15 秒后才发出去。用例在它回来之前按停。
      // 注意 `text()` 的第二个参数就是 `delayMs`（位置参数，不是 options 对象）。
      script: foremanScript([
        [text('（这一句本该 15 秒后才回——若它出现了，说明按停没生效）', 15_000)],
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
    // （纯函数三个态各一条），这里要验的是下一个断言：**它真的停了**。
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
});
