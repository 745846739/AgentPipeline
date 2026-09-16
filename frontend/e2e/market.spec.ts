/**
 * E2E ⑫：技能市场设置页的**两级关系**（决策 194，页骨架承自 187）。
 *
 * 钉的是那条抱怨：「没有配置 skill 的 market 的地方」——市场此前只有 `config.toml` 一条入口，
 * 改完还得重启。现在这一页能改**仓名单**、保存即生效、清掉即回到配置文件那一级。
 *
 * 与 `market-install.spec.ts`（E2E ⑬）的分工：这一条**一张网都不打**（一个技能都不列、一个
 * 包都不装），只钉「界面这一份与 `config.toml` 那一份谁说了算」，外加那条冷启动约束：
 * **内置的推荐名单在用户点「添加」之前一个字节都不下载**。
 *
 * 装置是 `gitRepo.ts` 起的离线 smart HTTP 仓（票 01 的接缝形态）：这一条里它**只用来当
 * 「网络有没有被打过」的听诊器**——它的请求日志必须一直是空的。
 *
 * 断言口径（只测外部行为）：路由可达、页面说出「现在以谁为准」、非法 `owner/repo` 在输入框旁
 * 被拦、合法项保存后 `origin` 变成界面那一份、清掉后回到配置文件，以及**显式清空 ≠ 未保存过**
 * （决策 187 记下的那条：空数组是「一个仓都不放行」，不是「回落配置文件」）。
 */

import { expect, test } from '@playwright/test';
import { startApp, settleBundle, watchBundle, expectBundleHealthy, type App } from './harness';
import { startGitRepo, type GitRepoFixture } from './gitRepo';
import { panel } from './panels';
import { foremanScript } from './scripts';

const OWNER = 'acme';
const REPO = 'skills';

test.describe('设置 · 技能市场：界面与配置的两级关系（决策 194）', () => {
  let app: App;
  let remote: GitRepoFixture;

  test.beforeAll(async () => {
    remote = await startGitRepo({
      owner: OWNER,
      repo: REPO,
      skills: [
        {
          dir: 'skills/grilling',
          description: '拷问设计树',
          body: ['拷问规则：事实自己查，只把决定问用户。'],
        },
      ],
    });
    // 起点是 `config.toml` 那一级：harness 把 github_repos 写进临时 home 的 [market]
    app = await startApp({
      script: foremanScript([[]]),
      providerOnly: true,
      market: { owner: OWNER, repo: REPO, gitBase: remote.base },
    });
  });

  test.afterAll(async () => {
    await app?.stop();
    await remote?.close();
  });

  test('仓名单可加可存可退回；非法 owner/repo 当场被拦；冷启动名单零网络', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    const repoPanel = panel(page, '仓名单');

    // ① 页面到位，且确实说明了「现在以谁为准」与「放行了几个仓」
    await expect(page.locator('.p-title')).toContainText('技能市场');
    await expect(repoPanel.locator('.chart-head .tag')).toContainText('config.toml');
    await expect(repoPanel.locator('.src-row .grow')).toHaveText(`${OWNER}/${REPO}`);
    await expect(repoPanel).toContainText('现在放行 1 个仓');
    // 旧层那三条说辞（registry / sha256 / 非回环必须 https）随那整层退场，不该再出现在这一页
    await expect(repoPanel).not.toContainText('非回环');
    await expect(repoPanel).not.toContainText('sha256');
    await expect(repoPanel).not.toContainText('index.json');

    // ② 冷启动推荐名单：它只是若干条**字符串**
    const recommended = repoPanel.locator('.rec-row');
    await expect(recommended.first()).toBeVisible();
    await expect(repoPanel).toContainText('一个字节都不会下载');
    // 给「加载时顺手把推荐仓 head 一下」留出犯错的机会：这段时间里网络必须安静
    await page.waitForTimeout(1000);
    expect(
      remote.requests,
      '推荐名单是配置默认值，不是目录：没点「添加」之前不该有任何 git 请求',
    ).toEqual([]);

    // ③ 非法 owner/repo 在输入框旁就被拦下，加不进去
    const input = repoPanel.getByPlaceholder('owner/repo');
    const addButton = repoPanel.locator('.subform').getByRole('button', { name: '＋ 添加' });
    await input.fill('acme');
    await expect(repoPanel.locator('.err').first()).toContainText('两段');
    await expect(addButton).toBeDisabled();

    await input.fill('git@github.com:acme/skills');
    await expect(repoPanel.locator('.err').first()).toContainText('@');
    await expect(addButton).toBeDisabled();

    await input.fill('acme/skills/extra');
    await expect(repoPanel.locator('.err').first()).toContainText('两段');
    await expect(addButton).toBeDisabled();

    await input.fill('https://gitlab.com/acme/skills');
    await expect(repoPanel.locator('.err').first()).toContainText('scheme');
    await expect(addButton).toBeDisabled();

    // ④ 合法项：粘贴 GitHub 网址也认（前缀与 .git 后缀会被去掉）
    await input.fill('https://github.com/other/tools.git');
    await addButton.click();
    await expect(repoPanel.locator('.src-row .grow')).toHaveCount(2);
    await expect(repoPanel.locator('.src-row .grow').nth(1)).toHaveText('other/tools');

    // 保存 = 当场生效（不重启），而保存本身不下载任何东西
    await repoPanel.getByRole('button', { name: /保存/ }).click();
    await expect(repoPanel.locator('.chart-head .tag')).toContainText('界面上的这一份', {
      timeout: 15_000,
    });
    await expect(repoPanel.locator('.ok')).toContainText('已保存');
    await expect(repoPanel.locator('.src-row .grow')).toHaveCount(2);
    expect(remote.requests, '保存仓名单不该触发任何下载').toEqual([]);

    // 保存之后这一份成了「界面上说了算」，页面上要能看到那条交还入口
    await expect(repoPanel.getByRole('button', { name: /改回 config.toml/ })).toBeVisible();

    // ⑤ 退回配置文件那一级：仓名单回到 config.toml 里那一条
    await repoPanel.getByRole('button', { name: /改回 config.toml/ }).click();
    await expect(repoPanel.locator('.chart-head .tag')).toContainText('config.toml', {
      timeout: 15_000,
    });
    await expect(repoPanel.locator('.src-row .grow')).toHaveText(`${OWNER}/${REPO}`);

    expectBundleHealthy(bundle);
  });

  test('显式清空 ≠ 未保存过：清空后不回落配置文件，点交还才回落', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    const repoPanel = panel(page, '仓名单');
    const listPanel = panel(page, '技能列表');

    // 上一条用例最后退回了配置文件那一级，故这里仍是它留下的状态
    await expect(repoPanel.locator('.src-row .grow')).toHaveText(`${OWNER}/${REPO}`);

    // 显式清空并保存：`origin` 仍是「界面上的这一份」——空数组是**一个仓都不放行**
    await repoPanel.getByRole('button', { name: '移除' }).click();
    await expect(repoPanel.locator('.blank')).toContainText('仓名单是空的');
    await repoPanel.getByRole('button', { name: /保存/ }).click();
    await expect(repoPanel.locator('.chart-head .tag')).toContainText('界面上的这一份', {
      timeout: 15_000,
    });
    await expect(repoPanel.locator('.blank')).toContainText('仓名单是空的');
    // 一个仓都没放行时，下半栏不能假装有技能可看
    await expect(listPanel.locator('.blank')).toContainText('一个仓都没放行');

    // 点交还 → 那一条又回来了（这才是「回到配置文件」）
    await repoPanel.getByRole('button', { name: /改回 config.toml/ }).click();
    await expect(repoPanel.locator('.src-row .grow')).toHaveText(`${OWNER}/${REPO}`, {
      timeout: 15_000,
    });
    await expect(repoPanel.locator('.chart-head .tag')).toContainText('config.toml');

    expectBundleHealthy(bundle);
  });
});
