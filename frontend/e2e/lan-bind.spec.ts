/**
 * E2E ⑪：手机访问页上的绑定开关（决策 186）。
 *
 * 这一条钉的是**用户的抱怨本身**：「桌面端手机访问没有配置绑定全网卡的按钮」——此前要开
 * 局域网访问只能设环境变量重启。现在这一页上有一颗钮，按下去**当场改绑**。
 *
 * 断言口径（只测外部行为）：按钮存在且可点；点完页面从「手机现在连不上」变成二维码区；
 * 关回来之后回到指引区。**绑定地址本身**的变化由 Rust 侧 `crates/app/tests/lan_bind.rs`
 * 用真二进制钉（那里能读 `/server-info` 的原文），这里只钉界面这一层。
 *
 * 改绑会切断当前所有连接（包括发出这次请求的那条），故这里**不**断言按钮点完立刻读到
 * 成功文案——判定以重读为准（`lib/lanToggle.ts`），界面据此要么切到二维码区、要么给出
 * 带原因的失败说明；两种情况都有明确的可断言形态。
 */

import { expect, test } from '@playwright/test';
import { startApp, settleBundle, watchBundle, expectBundleHealthy, type App } from './harness';
import { foremanScript } from './scripts';

test.describe('手机访问 · 绑定全网卡（决策 186）', () => {
  let app: App;

  test.beforeAll(async () => {
    // 空 home 即可：这一页只用 /server-info 与 /server/lan（不播项目与任务）
    app = await startApp({ script: foremanScript([[]]), providerOnly: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('缺省只绑回环：给出可点的钮；按下后进入二维码区，关回来又回到指引区', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/share`);
    await settleBundle(page, bundle);

    // ① 缺省（只绑回环）：不再只是一段「请去设环境变量」的文字，而是一颗钮
    const gate = page.locator('.gate');
    await expect(gate).toContainText('手机现在连不上这台机器');
    const open = page.getByRole('button', { name: /绑定全网卡/ });
    await expect(open).toBeVisible();
    await expect(open).toBeEnabled();
    // 老办法仍在，但收进了折叠区（不与那颗钮抢注意力）
    await expect(page.locator('.manual summary')).toContainText('启动时指定');

    // ② 按下 → 改绑（本机来源，被允许）→ 页面按重读到的状态重画成二维码区
    await open.click();
    await expect(page.locator('.qrbox')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('.picked')).toContainText(`:${new URL(app.webBase).port}`);
    // 说清这次绑定是谁定的（界面上的选择 / 启动参数 / 配置文件）
    await expect(page.locator('.switch-note')).toContainText(/界面上的选择|启动参数|配置文件/);

    // ③ 关回来 → 回到指引区（手机又连不上了）
    await page.getByRole('button', { name: /改回只绑本机/ }).click();
    await expect(page.locator('.gate')).toContainText('手机现在连不上这台机器', {
      timeout: 30_000,
    });
    await expect(page.getByRole('button', { name: /绑定全网卡/ })).toBeVisible();

    expectBundleHealthy(bundle);
  });
});
