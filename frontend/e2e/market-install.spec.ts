/**
 * E2E ⑬：通过技能市场**装一个技能**（决策 194 / 票 02 / 03）。
 *
 * 与 `market.spec.ts`（E2E ⑫）的分工：那一条只钉「界面与 `config.toml` 的两级关系」，
 * 明说不碰列表与安装；这一条钉的正是它留下的那段——**添加一个仓 → 按父路径分组列出里面的
 * 技能 → 装一个 → 三项预览齐备 → 技能真的落到技能根**，全程只经过界面。
 *
 * 为什么值得占一条 E2E（而 Rust 侧已有用例）：core 的用例走「处理器 → core」，**界面这一段
 * 没有替身**——保存仓名单后列表会不会真的去取、列表钉住的 commit 有没有一路透传给安装、
 * 装完是否立刻去拉预览、三项预览是否真的逐行摆出来（决策 181③）、同名冲突是否给出一次
 * 显式确认的机会（而不是静默覆盖），都只有在真 bundle + 真后端上才看得见。
 *
 * 装置是 `gitRepo.ts` 起的**离线 smart HTTP git 仓**（回环明文 http 由票 01 的
 * `AGENTPIPELINE_MARKET_GIT_BASE` 接缝放行）：走的是真 libgit2 路径——真 HTTP 传输、
 * `depth(1)` shallow、按裸 SHA 取 commit，故不打真网络也没有替身冒充传输层。
 * 旧的「手搓 ZIP + 普通 HTTP 的假 registry」随之退场（决策 194 拆掉了那一整层）。
 */

import { expect, test } from '@playwright/test';
import { startApp, settleBundle, watchBundle, expectBundleHealthy, type App } from './harness';
import { startGitRepo, type GitRepoFixture, type GitSkill } from './gitRepo';
import { panel } from './panels';
import { foremanScript } from './scripts';

const OWNER = 'acme';
const REPO = 'skills';

/**
 * 三个技能，覆盖两件事：
 * - **两种深度形态**：`skills/grilling`（2 段）与 `plugins/agent-kit/skills/{tdd,pairing}`（4 段）
 *   ——列表按**父路径**分组，正是为这两种形态设计的（摊平了没法看）。
 * - **正文特征**：`grilling` 三类特征全中（网络调用 / 命令执行 / 密钥路径），钉决策 181③
 *   的「逐行摆出命中」；`tdd` / `pairing` 正文干净，钉③的另一条分支（「未命中已知特征」）。
 *
 * 行号是断言的一部分：`frontmatter` 占 1–4 行、第 5 行空行，故 `body[0]` 在第 6 行，
 * `grilling` 的三处命中落在 L7 / L8 / L9。
 */
const SKILLS: GitSkill[] = [
  {
    dir: 'skills/grilling',
    description: '拷问设计树',
    body: [
      '拷问规则：事实自己查，只把决定问用户。',
      '联网取一手资料时用 curl 拉文档，不要凭记忆。',
      '命令执行一律走 run_command，不绕路。',
      '不要把 .env 里的密钥贴进正文。',
    ],
    // 子树兄弟文件：仓里的技能目录不止一份 SKILL.md（读目录要**递归**，票 01 第 6 条）。
    // 它的落盘在本用例里看不出来（界面不展示兄弟文件），递归读本身由票 01 的用例钉——
    // 这里带上它是为了让 fixture 的形态与真仓一致（`pulumi/agent-skills` 就是这种形态）。
    siblings: { 'refs/notes.md': '拷问时先自己查一遍事实。\n' },
  },
  {
    dir: 'plugins/agent-kit/skills/tdd',
    description: '测试驱动',
    body: ['先写会失败的用例，再写让它通过的实现。', '红 → 绿 → 重构，三步不许跳。'],
  },
  {
    dir: 'plugins/agent-kit/skills/pairing',
    description: '结对',
    body: ['一次只让一个人握着键盘。'],
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

test.describe('技能市场：装一个技能（决策 194）', () => {
  let app: App;
  let remote: GitRepoFixture;

  test.beforeAll(async () => {
    remote = await startGitRepo({ owner: OWNER, repo: REPO, skills: SKILLS });
    // `seedConfig: false`：home 里**没有** `[market]`，仓名单必须由用例经过界面加进去
    // ——这正是本条的「添加 = 放行」那一步（决策 194 的信任单元）。
    app = await startApp({
      script: foremanScript([[]]),
      providerOnly: true,
      market: { owner: OWNER, repo: REPO, gitBase: remote.base, seedConfig: false },
    });
  });

  test.afterAll(async () => {
    await app?.stop();
    await remote?.close();
  });

  test('添加一个仓、按父路径分组列出技能、装一个并看到三项预览', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    const repoPanel = panel(page, '仓名单');
    const listPanel = panel(page, '技能列表');

    // ① 默认态：一个仓都没放行（决策 194 的「默认拒绝」在界面上看得见）
    await expect(repoPanel.locator('.blank')).toContainText('仓名单是空的');
    expect(remote.requests, '没有放行任何仓时不该有任何 git 请求').toEqual([]);

    // ② 添加一个仓并保存 = 放行它（保存即生效，故下面立刻能列）
    await repoPanel.getByPlaceholder('owner/repo').fill(`${remote.owner}/${remote.repo}`);
    await repoPanel.locator('.subform').getByRole('button', { name: '＋ 添加' }).click();
    await repoPanel.getByRole('button', { name: /保存/ }).click();
    await expect(repoPanel.locator('.chart-head .tag')).toContainText('界面上的这一份', {
      timeout: 15_000,
    });

    // ③ 选中这个仓 → 列出它里面的技能，**按技能目录的父路径分组**（组按 path 升序）
    await repoPanel.getByRole('button', { name: '查看技能' }).click();
    const heads = listPanel.locator('.grp-head');
    await expect(heads).toHaveCount(2, { timeout: 30_000 });
    await expect(heads.nth(0)).toHaveText('plugins/agent-kit/skills');
    await expect(heads.nth(1)).toHaveText('skills');
    const hits = listPanel.locator('.hit');
    await expect(hits).toHaveCount(3);

    // 每行显示技能名 + description，且带着它在仓里的目录（安装的身份就是这一个）
    const grilling = hits.filter({ hasText: 'grilling' });
    await expect(grilling.locator('.hit-name')).toHaveText('grilling');
    await expect(grilling).toContainText('拷问设计树');
    await expect(grilling.locator('.hit-dir')).toHaveText('skills/grilling');

    // ④ 列表钉住浏览那一刻的 commit：顶部写着「基于 <短 SHA>」（时间）
    const tip = remote.tip();
    await expect(listPanel.locator('.listmeta')).toContainText(`基于 ${tip.slice(0, 7)}`);

    // ⑤ 关键词是**对已 fetch 这一份的本地过滤**，不是去搜全 GitHub
    const filter = listPanel.getByPlaceholder('技能名或描述关键词（留空 = 列出全部）');
    await filter.fill('grill');
    await expect(hits).toHaveCount(1);
    await expect(hits.locator('.hit-name')).toHaveText('grilling');
    await filter.fill('');
    await expect(hits).toHaveCount(3);

    // ⑥ 装一个 → 立刻给出三项预览（决策 181③：特征命中要摆在眼前再决定要不要启用）
    await grilling.getByRole('button', { name: '安装', exact: true }).click();
    const installed = panel(page, '刚装上：grilling');
    await expect(installed).toBeVisible({ timeout: 30_000 });
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

    // 逐行命中：三类各一行，行号对着 SKILL.md 能直接定位
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

    // ⑦ 真的落盘了：读后端而不是信界面（技能根下多出这一份）
    const skills = await listSkills(app);
    const landed = skills.find((s) => s.name === 'grilling');
    expect(landed, `技能未落到技能根：${JSON.stringify(skills)}`).toBeDefined();
    expect(landed?.description).toBe('拷问设计树');

    // ⑧ fixture 真的被 fetch 过（ls-remote 一次 + pack 一次）——否则「装上了」可能只是界面演出来的
    expect(remote.requests.some((r) => r.includes('/info/refs'))).toBe(true);
    expect(remote.requests.some((r) => r.startsWith('POST '))).toBe(true);

    expectBundleHealthy(bundle);
  });

  test('同名重装不静默覆盖：先给一次显式确认的机会；覆盖安装成功', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    const repoPanel = panel(page, '仓名单');
    const listPanel = panel(page, '技能列表');

    // 上一条用例保存的仓名单是**服务端**留存的，刷新后仍在（界面这一级不是内存里的草稿）
    await expect(repoPanel.locator('.src-row .grow')).toHaveText(`${OWNER}/${REPO}`);
    await repoPanel.getByRole('button', { name: '查看技能' }).click();
    const hits = listPanel.locator('.hit');
    await expect(hits).toHaveCount(3, { timeout: 30_000 });

    // 装第二个技能：正文一个特征词都没有 → ③ 走「未命中」那条分支
    const tdd = hits.filter({ hasText: 'tdd' });
    await tdd.getByRole('button', { name: '安装', exact: true }).click();
    const installed = panel(page, '刚装上：tdd');
    await expect(installed).toBeVisible({ timeout: 30_000 });
    await expect(installed.locator('.prev-col.wide')).toContainText('未命中已知特征');

    // 再装一次同一个：不静默覆盖，先问一次（这句串被既有 E2E 锚着，不许改）
    await tdd.getByRole('button', { name: '安装', exact: true }).click();
    await expect(listPanel.locator('.err').first()).toContainText('已存在', { timeout: 30_000 });
    await expect(tdd).toContainText('同名已存在，覆盖？');

    // 显式覆盖 → 走通，且错误提示清干净
    await tdd.getByRole('button', { name: '覆盖安装' }).click();
    await expect(installed).toBeVisible({ timeout: 30_000 });
    await expect(listPanel.locator('.err')).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test('列表钉住浏览那一刻的 commit：远端前进后不刷新仍是那一份，刷新才换', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/settings/market`);
    await settleBundle(page, bundle);

    const repoPanel = panel(page, '仓名单');
    const listPanel = panel(page, '技能列表');
    await repoPanel.getByRole('button', { name: '查看技能' }).click();
    const hits = listPanel.locator('.hit');
    await expect(hits).toHaveCount(3, { timeout: 30_000 });

    const before = remote.tip();
    await expect(listPanel.locator('.listmeta')).toContainText(`基于 ${before.slice(0, 7)}`);

    // 远端前进：仓里多出一个技能目录
    const after = remote.commit('feat: 再加一个技能', {
      'skills/extra/SKILL.md': '---\nname: extra\ndescription: 后加的技能\n---\n\n后加的。\n',
    });
    expect(after).not.toBe(before);

    // 不刷新：列表还是那一份（新技能不出现），装的也只能是那一份
    await expect(hits).toHaveCount(3);
    await expect(listPanel.locator('.hit-name', { hasText: 'extra' })).toHaveCount(0);
    await hits.filter({ hasText: 'pairing' }).getByRole('button', { name: '安装', exact: true }).click();
    await expect(panel(page, '刚装上：pairing')).toBeVisible({
      timeout: 30_000,
    });
    // 「看到的 = 装到的」：不许出现对**新** tip 的 want（那说明中途偷偷取了最新）
    expect(remote.wants).not.toContain(after);

    // 显式刷新 → 取这个仓现在的 tip，新技能才出现
    await listPanel.getByRole('button', { name: '刷新' }).click();
    await expect(listPanel.locator('.listmeta')).toContainText(`基于 ${after.slice(0, 7)}`, {
      timeout: 30_000,
    });
    await expect(hits).toHaveCount(4);

    expectBundleHealthy(bundle);
  });
});
