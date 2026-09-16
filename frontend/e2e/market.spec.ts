/**
 * E2E ⑫：技能市场设置页（决策 187）。
 *
 * 钉的是那条抱怨：「没有配置 skill 的 market 的地方」——市场（票 10 / 决策 177）此前只有
 * `config.toml` 一条入口，改完还得重启。现在这一页能改来源白名单、保存即生效、清掉即回到
 * 配置文件那一级，并且能搜索与安装。
 *
 * 断言口径（只测外部行为）：路由可达、页面说出「现在以谁为准」、加一项非法来源被拦、
 * 合法来源保存后 origin 变成界面那一份、清掉后回到配置文件。**不**在这里打真网络：
 * 搜索与安装的网络路径由 Rust 侧契约测试用 `FakeMarket` 离线钉住（票 10 / 决策 177）。
 */

import { expect, test } from '@playwright/test';
import { startApp, settleBundle, watchBundle, expectBundleHealthy, type App } from './harness';
import { foremanScript } from './scripts';

test.describe('设置 · 技能市场（决策 187）', () => {
  let app: App;

  test.beforeAll(async () => {
    // 空 home：这一页只碰 /market/config（无来源时白名单是空的）
    app = await startApp({ script: foremanScript([[]]), providerOnly: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('导航可达；白名单可加可存可退回；非法来源当场被拦', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    // ① 页面到位，且确实说明了「现在以谁为准」与「是否允许远程安装」
    await expect(page.locator('.p-title')).toContainText('技能市场');
    await expect(page.locator('.chart-head .tag')).toContainText('config.toml');
    await expect(page.locator('.blk').first()).toContainText('不允许远程安装');

    // ② 非法来源（非回环明文 http）在输入框旁就被拦下，加不进去
    const input = page.getByPlaceholder('https://skills.example.com');
    await input.fill('http://skills.example.com');
    await expect(page.locator('.err').first()).toContainText('https');
    await expect(page.getByRole('button', { name: '＋ 添加' })).toBeDisabled();

    // ③ 换成合法来源 → 加上去 → 保存（当场生效）
    await input.fill('HTTPS://Skills.Example.com/');
    await page.getByRole('button', { name: '＋ 添加' }).click();
    await expect(page.locator('.src-row .grow')).toHaveText('https://skills.example.com');
    await page.getByRole('button', { name: /保存/ }).click();
    await expect(page.locator('.chart-head .tag')).toContainText('界面上的这一份', {
      timeout: 15_000,
    });
    await expect(page.locator('.src-row .grow')).toHaveText('https://skills.example.com');
    await expect(page.locator('.ok')).toContainText('已保存');

    // 保存之后这一份成了「界面上说了算」，页面上要能看到那条交还入口
    await expect(page.getByRole('button', { name: /改回 config.toml/ })).toBeVisible();

    // ④ 退回配置文件那一级：白名单回到「空」（本用例的 home 里没有 [market]）
    await page.getByRole('button', { name: /改回 config.toml/ }).click();
    await expect(page.locator('.chart-head .tag')).toContainText('config.toml', {
      timeout: 15_000,
    });
    await expect(page.locator('.blank')).toContainText('白名单是空的');

    expectBundleHealthy(bundle);
  });
});
