/**
 * E2E ⑪：手机访问页上的绑定开关（决策 186）、「取不到令牌就不画码」（决策 189）与顶栏入口的
 * 来源判据（决策 190）。
 *
 * 这一条钉的是**用户的抱怨本身**：「桌面端手机访问没有配置绑定全网卡的按钮」——此前要开
 * 局域网访问只能设环境变量重启。现在这一页上有一颗钮，按下去**当场改绑**。
 *
 * 断言口径（只测外部行为）：按钮存在且可点；点完页面从「手机现在连不上」变成二维码区，
 * 且那区里的码**带着配对令牌**；关回来之后回到指引区。**绑定地址本身**的变化由 Rust 侧
 * `crates/app/tests/integration/lan_bind.rs` 用真二进制钉（那里能读 `/server-info` 的原文），这里只钉
 * 界面这一层。决策 189 的另一半（非回环来源下不画码）钉在单元层：playwright 跑不出一个
 * 非回环来源，而 `routes/Share.test.ts` 能直接让 `GET /pairing/token` 回 403。
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
    // 决策 190 / 198：这一页是在跑服务的这台机器本机上打开的，故「手机访问」**有一个入口**——
    // 本轮（决策 198）它从顶栏的页面导航行挪进了设置落地页，顶栏只留三项。那一半（非本机来源
    // 不给这个入口）在 playwright 里造不出来，由单元层钉（`lib/localPage.test.ts` 与
    // `routes/SettingsLanding.test.ts`）；正反两面的真应用断言在 `settings-landing.spec.ts`。
    await page.goto(`${app.webBase}/#/settings`);
    await expect(page.getByRole('link', { name: /手机访问/ })).toBeVisible();
    // 回这一页继续
    await page.goto(`${app.webBase}/#/share`);
    await expect(page.locator('.gate')).toContainText('手机现在连不上这台机器');
    const open = page.getByRole('button', { name: /绑定全网卡/ });
    await expect(open).toBeVisible();
    await expect(open).toBeEnabled();
    // 老办法仍在，但收进了折叠区（不与那颗钮抢注意力）
    await expect(page.locator('.manual summary')).toContainText('启动时指定');

    // ② 按下 → 改绑（本机来源，被允许）→ 页面按重读到的状态重画成二维码区
    await open.click();
    await expect(page.locator('.qrbox')).toBeVisible({ timeout: 30_000 });
    await expect(page.locator('.picked')).toContainText(`:${new URL(app.webBase).port}`);
    // 决策 189：这一页是在回环来源上打开的，令牌读得到，故它给出的码**必须带着令牌**
    await expect(page.locator('.picked')).toContainText('?pair=');
    await expect(page.locator('.pair-note')).toContainText('二维码已带上配对令牌');
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

  /**
   * 决策 191：扫码那条 URL 上的令牌**不被抹掉**。
   *
   * 手机「添加到主屏幕」保存的就是当时地址栏里那条 URL，而 iOS 的主屏 web app 与 Safari
   * **各有独立存储**（localStorage 不互通）——抹掉它等于让主屏图标在「URL 没令牌、容器也没
   * 存储」的空状态下启动，从此再也配不上（这是实测反馈：添加到主屏幕后无法二次访问）。
   * 故这里钉这条属性本身（它与配对是否生效无关，用一枚假令牌即可）。
   */
  test('扫码那条 URL 的令牌留在地址栏里，并同时进了本地存储', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/?pair=e2e-token`);
    await settleBundle(page, bundle);

    expect(new URL(page.url()).searchParams.get('pair')).toBe('e2e-token');
    expect(await page.evaluate(() => window.localStorage.getItem('agentpipeline.pairing'))).toBe(
      'e2e-token',
    );

    // 「从主屏图标再进来」的等价动作：**容器里没有任何存储**（iOS 的主屏 web app 与 Safari
    // 隔离），整页重载——令牌只能从 URL 再递进来一次。这一条就是那个缺陷的复现路径：
    // 抹掉地址栏参数时，这里会以「未配对」启动。
    await page.evaluate(() => window.localStorage.clear());
    await page.reload();
    await settleBundle(page, bundle);
    expect(new URL(page.url()).searchParams.get('pair')).toBe('e2e-token');
    expect(await page.evaluate(() => window.localStorage.getItem('agentpipeline.pairing'))).toBe(
      'e2e-token',
    );
    expectBundleHealthy(bundle);
  });
});

/**
 * E2E ⑪b：配了**公网入口**时的「手机访问」页（决策 334）。
 *
 * 这一条钉的是 106 上那个真实的尴尬：后端只绑回环、门外是反向代理，而这一页只说
 * 「手机现在连不上这台机器」并递上一颗按下去会把明文入口装回来的「绑定全网卡」。
 * 真后端 + 真 `/server-info` + 真二维码端点一起跑，故**字段名在后端与前端之间漂了**
 * （组件层喂假读数时不会红的那种漂）在这里会红。
 */
test.describe('手机访问 · 公网入口（决策 334）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({
      script: foremanScript([[]]),
      providerOnly: true,
      // 缺省绑定仍是回环（`serve --port 0`）：这正是「绑在回环、门在外面」的形态
      serverConfig: ['public_base_url = "https://phone.example:3389"'],
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('画的是指向入口的码，不再说「手机连不上」、也不给改绑钮', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/share`);
    await settleBundle(page, bundle);

    // ① 二维码指向配置的入口，且带着令牌（这一页在本机打开，令牌读得到）
    const img = page.getByAltText(/扫码访问 https:\/\/phone\.example:3389\/\?pair=/);
    await expect(img).toBeVisible();
    await expect(img).toHaveAttribute(
      'src',
      /\/server-info\/qr\.svg\?url=https%3A%2F%2Fphone\.example%3A3389%2F%3Fpair%3D/,
    );
    // 地址栏里那一行就是入口（不是私网 eth0 + 后端端口）
    await expect(page.locator('.picked')).toContainText('https://phone.example:3389/?pair=');
    // ② 「只绑回环」那两块内容都不在：指引块与那颗会把明文入口装回来的钮
    await expect(page.locator('.gate')).toHaveCount(0);
    await expect(page.getByRole('button', { name: /绑定全网卡/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /改回只绑本机/ })).toHaveCount(0);
    // ③ 文案改口：手机走的是外面那道门，不再是「同一 Wi-Fi」
    await expect(page.locator('.qr-cap')).toContainText('反向代理');
    await expect(page.getByText(/由外面那道反向代理转发进来/)).toBeVisible();

    expectBundleHealthy(bundle);
  });
});
