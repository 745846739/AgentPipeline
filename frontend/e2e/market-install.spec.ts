/**
 * E2E ⑬：通过技能市场**添加技能**（决策 177 / 181 / 187）。
 *
 * 与 `market.spec.ts`（E2E ⑫）的分工：那一条钉的是「来源白名单在界面上可改」的**配置**通路，
 * 明说不碰搜索与安装；这一条钉的正是它留下的那段——**从市场搜到一个技能、装上、看到三项
 * 预览、技能真的落到技能根**，全程只经过界面。
 *
 * 为什么值得占一条 E2E（而 L3 契约已有 5 条）：契约测试注入 `FakeMarket`，走的是
 * 「处理器 → core」，**界面这一段没有替身**——保存来源后 `client_ready` 是否跟着翻、
 * 装完是否立刻去拉预览、三项预览是否真的逐行摆出来（决策 181③）、同名冲突是否给出一次
 * 显式确认的机会（而不是静默覆盖），都只有在真 bundle + 真后端上才看得见。
 *
 * 来源是一个**本机回环的离线 registry**（`marketRegistry.ts`）：回环明文 http 由决策 177③
 * 放行，故用例不打真网络、结果确定，且顺带验证了那条放行规则真的生效。
 *
 * **关于 skillhub.tencent.com**：诉求里点名要用它做市场，实测它今天当不了本系统的来源
 * （没有 `index.json`、任何地方都不给包的 `sha256`、下载走明文 http + 302 跨源、
 * 包是根级 `SKILL.md`），四道都过不去——逐条证据与复现见 `marketRegistry.ts` 的模块头。
 * 要用真 skillhub 得先写适配器并修订决策 177 的②③，那是产品口径的变更，不在一条用例里解决。
 */

import { expect, test } from '@playwright/test';
import { startApp, settleBundle, watchBundle, expectBundleHealthy, type App } from './harness';
import { startRegistry, type Registry, type RegistrySkill } from './marketRegistry';
import { foremanScript } from './scripts';

/**
 * 两个 fixture 技能，各钉一个分支：
 * - `grilling` 正文三类特征**全中**（网络调用 / 命令执行 / 密钥路径），钉决策 181③的
 *   「逐行摆出命中」；名字在 `STAGE_RECOMMENDATIONS` 里，故①推荐去向也有内容可看。
 * - `tdd` 正文干净，钉③的另一条分支（「未命中已知特征」）。
 */
const SKILLS: RegistrySkill[] = [
  {
    name: 'grilling',
    version: '1.2.0',
    description: '拷问设计树',
    // 行号是断言的一部分（①）：1–4 是 frontmatter，命中落在 7 / 8 / 9。
    body: [
      '拷问规则：事实自己查，只把决定问用户。',
      '联网取一手资料时用 curl 拉文档，不要凭记忆。',
      '命令执行一律走 run_command，不绕路。',
      '不要把 .env 里的密钥贴进正文。',
    ],
  },
  {
    name: 'tdd',
    version: '2.0.0',
    description: '测试驱动',
    body: ['先写会失败的用例，再写让它通过的实现。', '红 → 绿 → 重构，三步不许跳。'],
  },
];

async function listSkills(app: App): Promise<Array<{ name: string; description: string | null }>> {
  const res = await fetch(`${app.apiBase}/skills`);
  if (!res.ok) throw new Error(`GET /skills -> ${res.status}`);
  const body = (await res.json()) as {
    skills: Array<{ name: string; description: string | null }>;
  };
  return body.skills;
}

test.describe('技能市场：添加技能（决策 177 / 187）', () => {
  let app: App;
  let registry: Registry;

  test.beforeAll(async () => {
    registry = await startRegistry(SKILLS);
    // 空 home（只播 provider）：这一页只碰 /market/*，没有 [market] 段
    app = await startApp({ script: foremanScript([[]]), providerOnly: true });
  });

  test.afterAll(async () => {
    await app?.stop();
    await registry?.close();
  });

  test('搜到一个技能并装上：三项预览齐备，包真的落到技能根', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    // ① 默认态：没有来源 = 不允许远程安装（决策 177 的「默认拒绝」在界面上看得见）
    const sourcePanel = page.locator('.panel.blk').filter({ hasText: '来源白名单' });
    await expect(sourcePanel.locator('.blank')).toContainText('白名单是空的');
    await expect(sourcePanel).toContainText('不允许远程安装');

    // ② 填来源 → 添加 → 保存：保存即生效，故下面立刻能搜（这正是决策 187 那一页的理由）
    await page.getByPlaceholder('https://skills.example.com').fill(registry.origin);
    await page.getByRole('button', { name: '＋ 添加' }).click();
    await expect(sourcePanel.locator('.src-row .grow')).toHaveText(registry.origin);
    await page.getByRole('button', { name: /保存/ }).click();
    await expect(sourcePanel.locator('.chart-head .tag')).toContainText('界面上的这一份', {
      timeout: 15_000,
    });

    // ③ 搜索：按名字命中，候选带上版本 / 来源 / 摘要前缀
    const searchInput = page.getByPlaceholder('技能名或描述关键词（留空 = 列出全部）');
    await searchInput.fill('grill');
    await page.getByRole('button', { name: '搜索', exact: true }).click();
    const hits = page.locator('.hit');
    await expect(hits).toHaveCount(1, { timeout: 15_000 });
    const hit = hits.first();
    await expect(hit.locator('.hit-name')).toHaveText('grilling');
    await expect(hit.locator('.tag')).toHaveText('v1.2.0');
    await expect(hit).toContainText('拷问设计树');
    await expect(hit.locator('.sub.mono').first()).toHaveText(registry.origin);
    await expect(hit.locator('.sub.mono').last()).toContainText('sha256');

    // ④ 安装 → 立刻给出三项预览（决策 181③：特征命中要摆在眼前再决定要不要启用）
    await hit.getByRole('button', { name: '安装', exact: true }).click();
    const installed = page.locator('.panel.blk').filter({ hasText: '刚装上：grilling' });
    await expect(installed).toBeVisible({ timeout: 15_000 });
    await expect(installed.locator('.chart-head .tag')).toHaveText('尚未启用');
    // 装 ≠ 启用：这句话必须在明面上，否则用户以为装完就在跑了
    await expect(installed).toContainText('还没有任何阶段在用它');

    const columns = installed.locator('.prev-col');
    await expect(columns).toHaveCount(3);
    await expect(columns.nth(0).locator('.prev-head')).toHaveText('① 推荐去向');
    await expect(columns.nth(0)).toContainText('architect-design');
    await expect(columns.nth(1).locator('.prev-head')).toHaveText('② 注入模式与信任态');
    await expect(columns.nth(1)).toContainText('尚未被任何配置引用');
    await expect(columns.nth(1)).toContainText('未受信任');

    // ③ 逐行命中：三类各一行，行号对着 SKILL.md 能直接定位
    await expect(columns.nth(2).locator('.prev-head')).toHaveText('③ 正文特征（只用于告知）');
    const featureRows = columns.nth(2).locator('.prev-list li');
    await expect(featureRows).toHaveCount(3);
    await expect(featureRows.nth(0)).toContainText('网络调用');
    await expect(featureRows.nth(0)).toContainText('L7');
    await expect(featureRows.nth(0)).toContainText('curl');
    await expect(featureRows.nth(1)).toContainText('命令执行');
    await expect(featureRows.nth(1)).toContainText('L8');
    await expect(featureRows.nth(1)).toContainText('run_command');
    await expect(featureRows.nth(2)).toContainText('密钥路径');
    await expect(featureRows.nth(2)).toContainText('L9');
    await expect(featureRows.nth(2)).toContainText('.env');

    // ⑤ 真的落盘了：读后端而不是信界面（技能根下多出这一份）
    const skills = await listSkills(app);
    const landed = skills.find((s) => s.name === 'grilling');
    expect(landed, `技能未落到技能根：${JSON.stringify(skills)}`).toBeDefined();
    expect(landed?.description).toBe('拷问设计树');

    // ⑥ registry 真的被问过索引与包各一次——否则「装上了」可能只是界面演出来的
    expect(registry.requests).toContain('/index.json');
    expect(registry.requests).toContain('/skills/grilling-1.2.0.zip');

    expectBundleHealthy(bundle);
  });

  test('同名重装不静默覆盖：先给一次显式确认的机会；干净正文报「未命中已知特征」', async ({
    page,
  }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    // 上一条用例保存的来源是**服务端**留存的，刷新后仍在（界面这一级不是内存里的草稿）
    await expect(page.locator('.src-row .grow')).toHaveText(registry.origin);

    // 搜第二个技能：正文一个特征词都没有 → ③ 走「未命中」那条分支
    await page.getByPlaceholder('技能名或描述关键词（留空 = 列出全部）').fill('tdd');
    await page.getByRole('button', { name: '搜索', exact: true }).click();
    const hit = page.locator('.hit').filter({ hasText: 'tdd' });
    await expect(hit).toBeVisible({ timeout: 15_000 });

    await hit.getByRole('button', { name: '安装', exact: true }).click();
    const installed = page.locator('.panel.blk').filter({ hasText: '刚装上：tdd' });
    await expect(installed).toBeVisible({ timeout: 15_000 });
    await expect(installed.locator('.prev-col.wide')).toContainText('未命中已知特征');

    // 再装一次同一个：不静默覆盖，先问一次
    await hit.getByRole('button', { name: '安装', exact: true }).click();
    await expect(page.locator('.err').first()).toContainText('已存在', { timeout: 15_000 });
    await expect(hit).toContainText('同名已存在，覆盖？');

    // 显式覆盖 → 走通，且预览面板换了新的这一份
    await page.getByRole('button', { name: '覆盖安装' }).click();
    await expect(installed).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('.err')).toHaveCount(0);

    expectBundleHealthy(bundle);
  });
});
