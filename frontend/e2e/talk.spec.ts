/**
 * 前端 E2E ⑩：对讲台（`#/talk`，决策 174 / 182 / 183 / theme-6-pixel.md §3.3）。
 *
 * **版面前提在票 04 被推翻重写**：本页不再是「真实状态的只读转述」（旧稿把它写成
 * 「不是可自由对话的 chat，输入口只是一段说明」），而是一个**任务无关**的自由对话界面
 * （用户原话：「对话不需要依赖任务」）。三分区是它的硬要求：
 * 状态区（急停）钉在第一屏、对话时间线会滚、输入坞钉底；值班板是独立的一块
 * （桌面右栏、窄屏收成时间线之上的横向灯条），**不在状态区里**。
 *
 * 断言口径与其它像素主题用例一致：只测**外部行为**——路由可达、真数据渲染、
 * 急停那轮的后端下发动作可下发、给值班长发话后**回话里没有按钮**（时间线里唯一的钮是
 * 操作台的确认钮，票 03）、两张急停同挂时两张都留在第一屏（决策 183）。
 */

import { expect, test, type Locator } from '@playwright/test';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  startApp,
  waitForTask,
  waitForTaskById,
  pendingTypeOf,
  watchBundle,
  settleBundle,
  expectBundleHealthy,
  type App,
} from './harness';
import { foremanScript, fullPassScript, archBlockerRounds, readTask, text, tool } from './scripts';

/**
 * 值班长的回话：10 行，**每轮同文**（只有开头的标记用于断言）。
 *
 * 两件事同时成立才有意义：时间线要靠它**真的溢出**——只有溢出了，「滚到底之后
 * 急停仍在视口里」才是一条有效断言；而「每轮同文」是为了在 CI 重试时也不虚——
 * 重试会接着消费脚本的下一轮，同文就不怕轮号错位。
 */
const REPLY_MARK = '值班长回话标记';
const FOREMAN_REPLY = [
  REPLY_MARK,
  '本轮态势：没有需要你处理的事。',
  ...Array.from({ length: 8 }, (_, i) => `- 工位读数 ${i + 1}：安静。`),
].join('\n');

/**
 * 回执那一轮要验的是「回执怎么渲染」，三步各钉一件事：
 *
 * ① 查一个**不存在**的任务 → 工具执行成功、台账里没这个号，标签是**已读**。
 *    「查无此任务」不是工具故障（决策 33 的分层：它不该累计 `tool_retry_max`），
 *    值班长收到的是可转述的文本，不是错误。
 * ② 发一个**不在清单里**的工具 → 在执行点被拒，标签是**未读到**。
 *    这一步顺带把安全边界钉在界面上：值班长调不动越权工具。
 *    （票 06 之后这里用 `spawn_sub_agent`：`run_command` 已经进清单，它走确认钮
 *    而不是被拒——边界由「压根不在清单里」的那些名字取证。）
 * ③ 提一条 `write_file` → `ask` 档下**不执行**，落成一条**提议轮**（票 03）：
 *    时间线上多一颗等人按的钮，而**回话轮里一颗钮都没有**。
 *
 * 工位来源（`stage`）不在这条用例里断言：它要把**真实**任务 id 写进脚本，而 id 由后端
 * 在 `startApp` 之后生成，mock 没有事后注入脚本的口子。来源渲染由 Talk.svelte 的
 * 回执分支按 `trace.stage` 读出，属理由可证、e2e 不可达。
 */
const UNKNOWN_TASK_ID = '01K0000000000000000000000X';
/** 提议要写的那个文件（相对家目录根；app 的家目录是每次 `startApp` 新建的临时目录）。 */
const PROPOSED_FILE = 'foreman-note.md';

/** 十二轮回话（每轮：查台账 + 试一个越权工具 + 提一条写文件）；留足余量给 CI 的一次重试。 */
const foremanRounds = foremanScript(
  Array.from({ length: 12 }, () => [
    readTask(UNKNOWN_TASK_ID),
    tool('spawn_sub_agent', { task: '去干点别的' }),
    tool('write_file', { path: PROPOSED_FILE, content: '夜班交接：一切正常' }),
    text(FOREMAN_REPLY),
  ]),
);

/**
 * 直接对值班长说话，不经 UI。
 *
 * 铺长时间线时用：每次 UI 发送都要等一次异步收尾，连铺几轮会让用例又慢又脆。
 * 「用户能不能从界面把话说出去」由 `空看板也能对话` 那条用例覆盖（点发送钮），
 * 以及本组第一条用例的 Enter 发送。
 */
async function sayDirect(app: App, body: string): Promise<void> {
  const res = await fetch(`${app.apiBase}/foreman/messages`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
    body: JSON.stringify({ text: body }),
  });
  if (!res.ok) throw new Error(`POST /foreman/messages -> ${res.status}: ${await res.text()}`);
}

test.describe('对讲台 · 版面（票 04）', () => {
  let app: App;
  const title = 'E2E talk';

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('E2E'), title });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('路由可达：#/talk 与原型写法 #v-talk 都落到对讲台，顶栏有入口', async ({ page }) => {
    const bundle = watchBundle(page);

    // 正名
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.talk .tt')).toHaveText('对讲台');
    // 不是 not-found（这条是用户实际踩到的那个坑：应用当时没有该路由）
    await expect(page.locator('.notfound')).toHaveCount(0);

    // 原型视图 id 写法：照 theme-6-pixel.md §3.3 手敲的地址也要能进
    await page.goto(`${app.webBase}/#v-talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.talk .tt')).toHaveText('对讲台');
    await expect(page.locator('.notfound')).toHaveCount(0);

    // 输入坞里是一个**真的** textarea（旧稿的「不做假输入框/不自由对话」已作废）
    await expect(page.locator('.typer textarea')).toBeVisible();
    await expect(page.getByRole('button', { name: /发送/ })).toBeVisible();

    // 三分区的整页高度算式里含「桌面顶栏不超过 88px」这个假设（Talk.svelte 的 height）。
    // 顶栏长高超过它，整页就会开始能滚，「急停钉在第一屏」随之失效——故在这里钉住。
    const topBox = await page.locator('header.top').boundingBox();
    expect(topBox?.height ?? 0).toBeLessThan(88);

    // 顶栏入口（导航行第 5 个 chip，复用 foreman 头像；图标按 chip 节奏 16px）
    const chip = page.locator('.navbar .chip', { hasText: '对讲台' });
    await expect(chip).toBeVisible();
    const svg = chip.locator('svg.sprite');
    await expect(svg).toBeVisible();
    const box = await svg.boundingBox();
    expect(box?.width).toBe(16);

    expectBundleHealthy(bundle);
  });

  test('值班板 = 8 工位，且读数与看板同源', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.talk')).toBeVisible();

    const rows = page.locator('.talk .brow');
    await expect(rows).toHaveCount(8);
    // 8 工位名与看板列一一对应（含双轨合并列）
    const names = await rows.locator('.bnm').allTextContents();
    expect(names).toContain('init');
    expect(names).toContain('develop-design ∥ test-design');
    expect(names).toContain('done');
    // 每个工位一枚 8px 灯
    expect(await page.locator('.talk .blamp').count()).toBe(8);

    expectBundleHealthy(bundle);
  });

  test('急停 = 状态区里真数据渲染的对话框：标题、中文理由、后端下发的恢复动作可下发', async ({
    page,
  }) => {
    const bundle = watchBundle(page);

    // fullPassScript 会一路推到 merge_approval（合入需人工拍板）
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval', 180_000);

    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    // 急停在**状态区**（第一屏），不在会滚的时间线里——这是三分区的结构性断言
    const turn = page.locator('.zone-status .turn.warn').first();
    await expect(turn).toBeVisible({ timeout: 60_000 });
    await expect(page.locator('.timeline .turn.warn')).toHaveCount(0);
    // 内容来自真实读数：任务标题 + pending 理由（界面用中文短标签，
    // 不把 merge_approval 这类内部枚举暴露给用户，见 pipeline.ts 的 pendingLabel）
    await expect(turn).toContainText(title);
    await expect(turn.locator('.dtag')).toContainText('等你拍板');
    await expect(turn.locator('.dtag')).toContainText('合并提案');
    await expect(turn).not.toContainText('merge_approval');
    // 双线框（2px 外框）与 ▼ 光标在场
    await expect(turn).toHaveCSS('border-top-width', '2px');
    // 名牌 = 发言者：这一轮是操作台在报急停。值班长的话在时间线里，且永远没有按钮
    // ——同一个动作在两处各渲染一颗钮会让「哪个是真的」变成要思考的问题（票 04）。
    await expect(turn.locator('.dname')).toHaveText('操作台');
    // ▼ 光标只在这一轮点亮（其余轮次 content: none）——「全站唯一响点」的像素纪律
    const cursor = await turn.evaluate((el) => getComputedStyle(el, '::after').content);
    expect(cursor).toContain('▼');

    // 恢复动作 = 后端下发的那一份（决策 101 纯渲染），且真的能下发
    const approve = turn.getByRole('button', { name: /合入/ });
    await expect(approve).toBeVisible({ timeout: 60_000 });
    await approve.click();

    // 合入后回到非 pending：对讲台的急停轮消失（页面读的是同一份真实状态）
    await waitForTask(app, (t) => t.status !== 'pending', 'resumed', 180_000);
    await page.reload();
    await settleBundle(page, bundle);
    await expect(page.locator('.zone-status .turn.warn')).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test('移动款：顶栏仍为 138px，值班板收成对话之上的横向灯条', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.talk')).toBeVisible({ timeout: 60_000 });

    // 顶栏高度是 §5 两处定值（scroll-margin-top / 横幅 top = 148px）的依据：
    // 新增的 foreman 头像在 34px 页签盒里必须按 16px 显示，否则会撑到 156px。
    const headerBox = await page.locator('header.top').boundingBox();
    expect(headerBox?.height).toBe(138);

    // 单列：值班板不再是右侧 sticky 栏
    const side = page.locator('.talk-side');
    await expect(side).toBeVisible();
    await expect(side).toHaveCSS('position', 'static');

    // 收成横向灯条（`crew`）：工位横排、不缩不折，且排在状态区（急停）之上
    const brows = side.locator('.brow');
    await expect(brows).toHaveCount(8);
    const first = await brows.first().boundingBox();
    const last = await brows.last().boundingBox();
    expect(first).not.toBeNull();
    expect(last).not.toBeNull();
    expect(last?.y).toBe(first?.y);
    expect(last?.x ?? 0).toBeGreaterThan(first?.x ?? 0);
    const crewBox = await side.boundingBox();
    const statusBox = await page.locator('.zone-status').boundingBox();
    expect(crewBox?.y ?? 0).toBeLessThan(statusBox?.y ?? 0);

    expectBundleHealthy(bundle);
  });
});

/** 状态区里每张急停轮都完整落在状态区的可见范围内（= 没被区内滚动推到第一屏之外）。 */
async function expectEveryStopInsideZone(zone: Locator): Promise<void> {
  const zoneBox = await zone.boundingBox();
  expect(zoneBox).not.toBeNull();
  const cards = zone.locator('.turn.warn');
  const n = await cards.count();
  expect(n).toBeGreaterThan(0);
  for (let i = 0; i < n; i += 1) {
    const box = await cards.nth(i).boundingBox();
    expect(box).not.toBeNull();
    expect((box?.y ?? 0) + (box?.height ?? 0)).toBeLessThanOrEqual(
      (zoneBox?.y ?? 0) + (zoneBox?.height ?? 0) + 1,
    );
    expect(box?.y ?? 0).toBeGreaterThanOrEqual((zoneBox?.y ?? 0) - 1);
  }
}

/**
 * 状态区**不需要区内滚动**——「所有急停都在第一屏」的最干脆说法。
 *
 * 留 4px 余量：急停轮的 `4px 4px 0` 硬投影在 Chromium 里可能被算进 scrollable overflow，
 * 而 `.zone-status` 的下内边距恰好也是 4px。
 */
async function expectZoneNeedsNoScroll(zone: Locator): Promise<void> {
  const overflow = await zone.evaluate((el) => el.scrollHeight - el.clientHeight);
  expect(overflow, '状态区被内容撑到需要区内滚动').toBeLessThanOrEqual(4);
}

/**
 * 对讲台 · 两张急停同时挂在状态区（决策 183）。
 *
 * **这条用例钉的是折叠的存在理由**：一张急停轮内联着后端下发的动作集，最高的一种形状
 * （`info_insufficient`：带补充输入的 resume + 旁路动作）约 330px，而状态区上限桌面 46vh
 * 在 13″ 笔记本上可用只有约 344px——**展开一张就已经占满整个区**。不折叠的话，两张同挂时
 * 最老的那张必然被挤到区内滚动之外，正是 §3.3 说的「本页最不能出的错」。
 *
 * 断言口径是**几何**，不是文案：两张都完整落在状态区里、且状态区自己不需要区内滚动。
 * 两条任务都停在 `info_insufficient`（最高的急停轮形状），故更矮的形状自然也在第一屏。
 */
test.describe('对讲台 · 两张急停同时在状态区（决策 183）', () => {
  let app: App;
  const titleA = 'E2E 两张急停 A';
  const titleB = 'E2E 两张急停 B';

  test.beforeAll(async () => {
    // 两条任务各停在自己的 info_insufficient：mock 按任务标题路由脚本轮，互不消费。
    // 同项目并发第二任务是既有能力（主流程票 09），这里只是让两条都挂上急停。
    app = await startApp({
      script: archBlockerRounds(),
      title: titleA,
      additionalTasks: [{ title: titleB, script: archBlockerRounds() }],
    });
    for (const id of app.taskIds) {
      await waitForTaskById(id, app, (t) => pendingTypeOf(t) === 'info_insufficient', id, 180_000);
    }
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('默认一张都不展开：两张都在第一屏，点开后后端下发的动作仍可下发', async ({ page }) => {
    const bundle = watchBundle(page);
    // 桌面最紧的一档（也是 playwright 默认视口）：46vh ≈ 331px
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const zone = page.locator('.zone-status');
    await expect(zone.locator('.turn.warn')).toHaveCount(2, { timeout: 60_000 });

    // **核心几何断言**：两张都在状态区里，且状态区自己不需要区内滚动
    await expectZoneNeedsNoScroll(zone);
    await expectEveryStopInsideZone(zone);

    // 两张以上**一张都不展开**：展开一张就占满 13″ 上的状态区，第二张照样掉出第一屏
    await expect(zone.locator('.turn.warn:not(.folded)')).toHaveCount(0);
    const rows = zone.locator('.turn.warn.folded');
    await expect(rows).toHaveCount(2);

    // 折叠只收动作区，**不收身份**：摘要条仍挂琥珀框 + ▼（全站唯一的响不因折叠降级）
    const row = rows.first();
    await expect(row.locator('.dtag')).toContainText('信息不足');
    await expect(row).toContainText('2 个动作'); // 后端下发的那一份，不是前端算的
    const cursor = await row.evaluate((el) => getComputedStyle(el, '::after').content);
    expect(cursor).toContain('▼');

    // 点开一张：恢复动作在那一轮里内联（来自后端，一个也没少）
    const opened = (await row.innerText()).includes(titleA) ? titleA : titleB;
    await row.getByRole('button', { name: /展开恢复动作/ }).click();
    const openCard = zone.locator('.turn.warn:not(.folded)');
    await expect(openCard).toHaveCount(1);
    await expect(openCard).toContainText(opened);
    await expect(openCard.getByRole('button', { name: /补充信息并继续/ })).toBeVisible();
    await expect(openCard.getByRole('button', { name: /取消任务/ })).toBeVisible();

    // **同时只展开一张**：在已有一张展开的情况下点另一张的展开 → 换过去，展开数仍是 1。
    // 这条是本用例里唯一能挡住「每张各自一个布尔开关」那种退化的断言——只从「全折叠」出发
    // 点一张、得到计数 1，两种实现都会过
    await expect(rows).toHaveCount(1);
    await rows.getByRole('button', { name: /展开恢复动作/ }).click();
    await expect(zone.locator('.turn.warn:not(.folded)')).toHaveCount(1);
    await expect(openCard).not.toContainText(opened); // 换成展开的那张是另一张
    await expect(openCard.getByRole('button', { name: /补充信息并继续/ })).toBeVisible();

    // 收起之后回到「都看得到」的默认形态（人显式收起的态不被自动弹开）
    await openCard.getByRole('button', { name: /收起/ }).click();
    await expect(zone.locator('.turn.warn:not(.folded)')).toHaveCount(0);
    await expect(rows).toHaveCount(2);

    expectBundleHealthy(bundle);
  });

  test('手机上（≤479px 断点、38vh）两张也都在第一屏', async ({ page }) => {
    const bundle = watchBundle(page);
    // 窄屏是最吃紧的一档：状态区收到 38vh（900 × 0.38 ≈ 342px），而钮在移动款有 44px 触控底线
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const zone = page.locator('.zone-status');
    await expect(zone.locator('.turn.warn')).toHaveCount(2, { timeout: 60_000 });
    await expect(zone.locator('.turn.warn.folded')).toHaveCount(2);

    await expectZoneNeedsNoScroll(zone);
    await expectEveryStopInsideZone(zone);

    expectBundleHealthy(bundle);
  });
});

test.describe('对讲台 · 对话（票 03）', () => {
  let app: App;
  const title = 'E2E talk 长对话';

  test.beforeAll(async () => {
    // archBlockerRounds 在流水线第一步就挂 pending：本组要的是「有一个急停在场且一直挂着」
    // （fullPass 那条会走到 merge_approval，虽然慢不了多少，但要的是不为合入分心）
    app = await startApp({
      script: { ...archBlockerRounds(), ...foremanRounds },
      title,
    });
    // 急停真的挂上（tick 1s）
    await waitForTask(app, (t) => t.status === 'pending', 'pending', 60_000);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('给值班长发话：Enter 发送、回复真的来自脚本、回复里没有按钮', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const input = page.locator('.typer textarea');
    await expect(input).toBeVisible();
    // Shift+Enter 换行 / Enter 发送：这里走 Enter（按钮那条由「空看板也能对话」覆盖）
    await input.fill('班次问句 · 标记 talk03');
    await input.press('Enter');

    // 回话真的从后端来（脚本供上的那一轮），且进了时间线
    const reply = page.locator('.timeline .turn.fm', { hasText: REPLY_MARK }).first();
    await expect(reply).toBeVisible({ timeout: 30_000 });
    await expect(reply.locator('.dname')).toHaveText('值班长');
    // 值班经理说的那句也在时间线里（台账那一行，不是只在输入框里）
    const mine = page.locator('.timeline .turn.mine', { hasText: '班次问句 · 标记 talk03' });
    await expect(mine).toHaveCount(1);
    // 名分两个词各有各的落点（决策 193）：对面是值班长，人这一侧是值班经理；
    // 输入坞那块名牌与时间线这块必须同一个词——两处各写各的就会同屏两个称呼
    await expect(mine.locator('.dname')).toHaveText('值班经理');
    await expect(page.locator('.typer .dname')).toHaveText('值班经理');

    // **值班长的回复里永远没有按钮**（票 04 的硬要求）：写动作只在状态区的急停轮里
    await expect(reply.locator('button')).toHaveCount(0);

    // 工位回执留在对话里：这一轮查过台账、也试过一个越权工具、还提了一件事 → 三条回执挂在
    // 回话那一轮内，形状与发言**不同**——左缘 4px 亮度阶、无框（转述不是发言）
    const rcpt = reply.locator('.rcpt');
    await expect(rcpt).toHaveCount(3);
    const ledger = rcpt.filter({ hasText: UNKNOWN_TASK_ID });
    await expect(ledger).toContainText('读任务台账');
    // 查无此任务是「已读」而不是「未读到」：工具执行成功了，只是台账里没这个号
    await expect(ledger).toContainText('已读');
    // 越权工具在执行点被拒 → 「未读到」，且它**真的没跑起来**（清单之外，白名单挡下）
    const denied = rcpt.filter({ hasText: 'spawn_sub_agent' });
    await expect(denied).toContainText('未读到');
    // `ask` 档下的写工具是**提议**而不是失败：回执记「已读」（调用成功，只是没执行），
    // 人按不按是另一件事——把它记成「未读到」会让人以为模型调错了工具
    // 用**标签**找它：`TOOL_LABELS` 把 `write_file` 译成「写文件」，回执上不出现原始工具名
    const proposed = rcpt.filter({ hasText: '写文件' });
    await expect(proposed).toContainText(PROPOSED_FILE);
    await expect(proposed).toContainText('已读');
    await expect(rcpt.locator('svg.sprite').first()).toBeVisible();
    await expect(rcpt.first()).toHaveCSS('border-left-width', '4px');
    await expect(rcpt.first()).toHaveCSS('border-top-width', '0px');

    // ── 提议轮（票 03）──
    // 时间线里**唯一**的按钮是操作台的确认钮：它在提议轮里，不在回话轮里。
    // 「写动作要人按键」与「哪颗钮是真的」由此同时成立（决策 176④ / 207③）。
    const prop = page.locator('.timeline .turn.prop');
    await expect(prop).toHaveCount(1);
    await expect(prop.locator('.dname')).toHaveText('操作台');
    await expect(prop.locator('.dtag')).toContainText('等你按键');
    await expect(prop).toContainText(PROPOSED_FILE);
    // **不叫 `.warn`**（决策 203：全站唯一的响仍是急停）——时间线里不该出现急停轮
    await expect(page.locator('.timeline .turn.warn')).toHaveCount(0);
    await expect(prop).toHaveClass(/turn prop/);
    // 两颗钮：执行 + 拒绝。**回话轮里一颗都没有**（上面已断），故「时间线里的按钮」
    // 与「确认钮」是同一件事。
    await expect(prop.getByRole('button', { name: '执行' })).toBeVisible();
    await expect(prop.getByRole('button', { name: '拒绝' })).toBeVisible();
    await expect(page.locator('.timeline .turn.fm button')).toHaveCount(0);
    // 参数原文可展开（按键之前要看得出它到底要什么）
    await expect(prop.locator('.pargs')).toContainText('夜班交接：一切正常');

    // 值班长不占琥珀档：它的轮次与急停那一轮不是同一个描边色（全站唯一的响仍在急停一处），
    // 提议轮同样不占（决策 203）。
    const stopColor = await page
      .locator('.zone-status .turn.warn')
      .evaluate((el) => getComputedStyle(el).borderTopColor);
    const replyColor = await reply.evaluate((el) => getComputedStyle(el).borderTopColor);
    expect(replyColor).not.toBe(stopColor);
    const propColor = await prop.evaluate((el) => getComputedStyle(el).borderTopColor);
    expect(propColor).not.toBe(stopColor);

    // 会话合计来自台账（不是编的 0）
    await expect(page.locator('.talk-head .ts')).toContainText(/本次会话 [1-9]\d* tok/);

    expectBundleHealthy(bundle);
  });

  /**
   * 按下确认钮：**它不直接改状态**——发一次 `/foreman/proposals/{id}/execute`，让后端按
   * 提议里的参数走既有那条路，结果再以一轮对话回来（决策 207：成功失败都进时间线，不弹窗）。
   *
   * 三条一起看才成立：文件**按下之后**才出现（提议时没执行）、时间线多出一轮系统说明、
   * 那一轮的两颗钮收掉（终态）。
   */
  test('按下确认钮：文件真的写了，结果回灌成一轮', async ({ page }) => {
    const bundle = watchBundle(page);
    // **这一档要给对话区留出高度**：状态区占 46vh，720px 的窗口下时间线只剩一条缝——
    // 确认钮内联在时间线里（决策 207③），缝里那颗钮点不到。真人的窗口比这高，
    // 这里按真人的高度给一档（与窄屏用例显式设 430×900 是同一个手法）。
    await page.setViewportSize({ width: 1280, height: 1000 });

    // 先自己说一句，拿到一条**未决**提议（script 每轮都会提一条，取最后一条）
    await sayDirect(app, '提一条看看');
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const prop = page.locator('.timeline .turn.prop').last();
    await expect(prop).toBeVisible({ timeout: 30_000 });
    const proposalId = await prop.getAttribute('data-proposal');
    expect(proposalId).toBeTruthy();

    // 提议**没有**执行：家目录里那个文件还不存在（直接看磁盘，不经界面）
    const target = join(app.homeDir, PROPOSED_FILE);
    expect(existsSync(target)).toBe(false);

    await prop.getByRole('button', { name: '执行' }).click();

    // 按下之后文件**真的**落在那里——「确认钮不是只改了一行状态」靠这一条成立
    await expect
      .poll(() => existsSync(target), { timeout: 15_000 })
      .toBe(true);
    expect(readFileSync(target, 'utf8')).toBe('夜班交接：一切正常');

    // 结果以一轮**操作台记录**回到时间线（不是 toast、不是弹窗，也**不是值班长说的话**
    // ——动手的是按下那颗钮的人，挂在值班长的名牌下等于替它认领了它没做的事）
    const log = page.locator('.timeline .turn.console', { hasText: '提议已执行' });
    await expect(log).toBeVisible({ timeout: 30_000 });
    await expect(log.locator('.dname')).toHaveText('操作台');
    await expect(log.locator('button')).toHaveCount(0);
    // 终态：两颗钮收掉，那一轮**仍在**（审计）
    await expect(prop.getByRole('button', { name: '执行' })).toHaveCount(0);
    await expect(prop.locator('.dtag')).toContainText('执行过');
    await expect(prop).toBeVisible();

    expectBundleHealthy(bundle);
  });

  /**
   * 拒绝：同样是**按下之后才有的终态**，且要说清「没有执行任何动作」。
   *
   * 与「执行失败」分开是必要的：后者只是没成功，提议仍可再按一次。
   */
  test('按下拒绝：提议作废、明确说没有执行动作', async ({ page }) => {
    const bundle = watchBundle(page);
    // 同上：确认钮在时间线里，这一档要给它留出高度
    await page.setViewportSize({ width: 1280, height: 1000 });

    await sayDirect(app, '再提一条看看');
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const prop = page.locator('.timeline .turn.prop').last();
    await expect(prop).toBeVisible({ timeout: 30_000 });
    await prop.getByRole('button', { name: '拒绝' }).click();

    const log = page.locator('.timeline .turn.console', { hasText: '提议已拒绝' });
    await expect(log).toBeVisible({ timeout: 30_000 });
    await expect(log.locator('.dname')).toHaveText('操作台');
    await expect(prop.locator('.dtag')).toContainText('被拒绝');
    await expect(prop).toContainText('没有执行任何动作');
    await expect(prop.getByRole('button', { name: '执行' })).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test('长对话滚到底之后，急停仍在第一屏', async ({ page }) => {
    const bundle = watchBundle(page);

    // 铺长时间线（直连三轮；界面发送路径已由前一条用例覆盖）
    for (let i = 1; i <= 3; i += 1) {
      await sayDirect(app, `班次问句-${i}`);
    }
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    // 回话都进了台账（等它们渲染出来再谈滚动）
    await expect
      .poll(() => page.locator('.timeline .turn.fm').count(), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(3);

    const timeline = page.locator('.timeline');
    const stopTurn = page.locator('.zone-status .turn.warn').first();
    await expect(stopTurn).toBeVisible();

    // 时间线真的溢出（不溢出就谈不上「滚到底」）
    const overflow = await timeline.evaluate((el) => el.scrollHeight - el.clientHeight);
    expect(overflow).toBeGreaterThan(0);

    // 把时间线滚到底
    const scrolled = await timeline.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
      return el.scrollTop;
    });
    expect(scrolled).toBeGreaterThan(0);

    // 硬要求：一个两小时前挂起的急停滚出视野是本页最不能出的错
    await expect(stopTurn).toBeInViewport();
    await expect(stopTurn).toContainText(title);

    expectBundleHealthy(bundle);
  });
});

/**
 * 对讲台 · 窄屏（决策 192）：版面口径是**整页随手指滚，只有两条钉住物**。
 *
 * 宽屏靠「整页钉住 + 时间线是唯一滚动容器」；窄屏反过来——急停摘要条钉在顶栏下沿
 * （`.zone-status.stops`，`top: 138px`），输入坞钉在底栏上沿（`.typer`，`bottom: var(--sbar-h)`），
 * 对话从两者之间滚过去。
 *
 * **这一组自带装置**（不复用上面那组的）：那组最后一条用例会把唯一的急停按掉（点合入），
 * 之后再进来就没有 pending 了——几何断言会退化成「空状态区当然不挤」，绿灯但无意义。
 */
test.describe('对讲台 · 窄屏（决策 192）', () => {
  let app: App;
  const title = 'E2E 窄屏长对话';

  test.beforeAll(async () => {
    app = await startApp({
      script: {
        ...archBlockerRounds(),
        // 每轮 10 行（同 `FOREMAN_REPLY` 的量级）：三轮就足以让整页在 900px 上滚起来
        ...foremanScript(
          Array.from({ length: 6 }, () => [
            text(
              [
                '窄屏标记',
                '本轮态势：没有需要你处理的事。',
                ...Array.from({ length: 8 }, (_, i) => `- 工位读数 ${i + 1}：安静。`),
              ].join('\n'),
            ),
          ]),
        ),
      },
      title,
    });
    await waitForTask(app, (t) => t.status === 'pending', 'pending', 60_000);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /**
   * 这条钉的是那次改版的**结果**，不是机制：改版前同一装置（430×900、单张急停挂着）
   * 实测对话区只有 **26px**——钉死的状态区拿走 342px，页头 72px、值班板 67px、输入坞 145px
   * 再把剩下的分完。所以断言写成**几何**（对话区的高度、两条钉住物的贴合），而不是
   * 「CSS 里有没有 sticky」——后者在版面塌掉时照样为真。
   *
   * 必须排在本组第一条：后面的用例会把对话铺长（那时「不空滚」不再成立，是应该的）。
   */
  test('静置版面：对话区拿到整块屏幕、两条钉住物各就各位', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.zone-status .turn.warn')).toHaveCount(1, { timeout: 60_000 });

    // ① 单张急停也折成摘要条（宽屏那一档它应当是展开的——同一装置两种版面）
    await expect(page.locator('.zone-status .turn.warn.folded')).toHaveCount(1);
    // ② 状态区退出区内滚动：窄屏没有「状态区自己滚」这回事，去滚的是整页
    const zoneOverflow = await page
      .locator('.zone-status')
      .evaluate((el) => el.scrollHeight - el.clientHeight);
    expect(zoneOverflow, '状态区在窄屏仍然区内滚').toBeLessThanOrEqual(4);

    // ③ **对话区是「整块屏幕减去两条钉住物」**，不是它们之间的残渣
    const timelineBox = await page.locator('.timeline').boundingBox();
    expect(timelineBox?.height ?? 0).toBeGreaterThanOrEqual(320);

    // ④ 输入坞钉在底栏（`.statusline`）上沿：底边与底栏顶边不许有缝
    const typerBox = await page.locator('.typer').boundingBox();
    const sbarBox = await page.locator('.statusline').boundingBox();
    expect(typerBox).not.toBeNull();
    expect(sbarBox).not.toBeNull();
    expect(
      Math.abs((typerBox?.y ?? 0) + (typerBox?.height ?? 0) - (sbarBox?.y ?? 0)),
      '输入坞没有贴在底栏上沿',
    ).toBeLessThanOrEqual(1);

    // ⑤ 整页不空滚：没有对话时文档高度就是视口高度（多出来的每一像素都是从对话区借的）
    const stray = await page.evaluate(
      () =>
        Math.max(document.documentElement.scrollHeight, document.body.scrollHeight) -
        window.innerHeight,
    );
    expect(stray, '窄屏整页在没有对话时也能滚，说明版面高出了视口').toBeLessThanOrEqual(1);

    expectBundleHealthy(bundle);
  });

  test('整页滚到底之后，急停摘要条仍钉在顶栏下沿、输入坞仍贴在底栏上沿', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });

    // 铺长（直连；界面发送路径由「空看板也能对话」那条覆盖）
    for (let i = 1; i <= 3; i += 1) {
      await sayDirect(app, `窄屏问句-${i}`);
    }
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect
      .poll(() => page.locator('.timeline .turn.fm').count(), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(3);

    // 窄屏滚的是**整页**（时间线不再是滚动容器）
    const timelineOverflow = await page
      .locator('.timeline')
      .evaluate((el) => el.scrollHeight - el.clientHeight);
    expect(timelineOverflow, '窄屏的时间线仍是滚动容器').toBeLessThanOrEqual(1);
    const pageOverflow = await page.evaluate(
      () => document.documentElement.scrollHeight - window.innerHeight,
    );
    expect(pageOverflow, '窄屏的长对话没有把整页撑出滚动').toBeGreaterThan(0);

    // 滚到底
    const scrolled = await page.evaluate(() => {
      const d = document.scrollingElement as HTMLElement;
      d.scrollTop = d.scrollHeight;
      return d.scrollTop;
    });
    expect(scrolled).toBeGreaterThan(0);

    // **硬要求**：滚到底时急停摘要条仍钉在顶栏下沿（顶栏 138px 是 §5 定值）
    const headerBox = await page.locator('header.top').boundingBox();
    const zoneBox = await page.locator('.zone-status').boundingBox();
    expect(zoneBox).not.toBeNull();
    expect(
      Math.abs((zoneBox?.y ?? 0) - ((headerBox?.y ?? 0) + (headerBox?.height ?? 0))),
      '滚到底后急停摘要条没钉在顶栏下沿',
    ).toBeLessThanOrEqual(2);
    await expect(page.locator('.zone-status .turn.warn').first()).toBeInViewport();
    await expect(page.locator('.zone-status .turn.warn').first()).toContainText(title);

    // 输入坞也还在（同一时刻两样都在 = 这一档版面的全部合同）
    const typerBox = await page.locator('.typer').boundingBox();
    const sbarBox = await page.locator('.statusline').boundingBox();
    expect(
      Math.abs((typerBox?.y ?? 0) + (typerBox?.height ?? 0) - (sbarBox?.y ?? 0)),
      '滚到底后输入坞没贴在底栏上沿',
    ).toBeLessThanOrEqual(1);
    await expect(page.locator('.typer textarea')).toBeInViewport();

    // 滚回顶部：页头回来了（那些是随手指滚的部分）
    await page.evaluate(() => (document.scrollingElement as HTMLElement).scrollTo({ top: 0 }));
    await expect(page.locator('.talk-head .tt')).toBeInViewport();

    expectBundleHealthy(bundle);
  });
});

test.describe('对讲台 · 空看板也能对话（票 04 的验收锚点）', () => {
  let app: App;

  /** 空看板下的回话（同轮同文、备四轮：同样是给 CI 重试留的余量）。 */
  const emptyReply = '夜班安静：这台机器还没接入项目。说一句你要做什么，我记在台账上。';

  test.beforeAll(async () => {
    // providerOnly：有可用的模型、一个项目与任务都没有——首启用户的真实处境
    app = await startApp({
      script: foremanScript(Array.from({ length: 4 }, () => [text(emptyReply)])),
      providerOnly: true,
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('没有项目也没有任务时，输入是真的、发送能拿到值班长的回话', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.notfound')).toHaveCount(0);

    // 空看板：没有急停，也就没有任何恢复动作可下发（空态不靠后端动作集撑场面）
    await expect(page.locator('.zone-status .turn.warn')).toHaveCount(0);

    // 真的输入口，不是一段说明文字
    const input = page.locator('.typer textarea');
    await expect(input).toBeVisible();
    await expect(input).toBeEditable();

    // 空态的「去看板新建任务」是**页面固定的导航入口**（`EmptyState` 的可选入口，纯前端路由，
    // 不进后端动作契约）：它不在任何对话框（`.turn`）里，也不来自 allowed_actions——此刻全页
    // 没有任何后端下发的动作。
    // 它的出现同时证明看板已经装载完（空态等的是装载完成，不是"还没读"）。
    const navLink = page.getByRole('link', { name: '去看板新建任务' });
    await expect(navLink).toBeVisible();
    await expect(navLink).toHaveAttribute('href', '#/');
    await expect(page.locator('.zone-status .turn button')).toHaveCount(0);

    // 验收锚点：空看板照样能对话
    await input.fill('现在能做什么');
    await page.getByRole('button', { name: /发送/ }).click();

    const reply = page.locator('.timeline .turn.fm').first();
    await expect(reply).toContainText('夜班安静', { timeout: 30_000 });
    // 回话里没有按钮；这一轮的脚本也没提任何提议，故**整条时间线**一颗钮都没有
    //（时间线上唯一的钮是操作台的确认钮，票 03——没有提议就没有它）
    await expect(reply.locator('button')).toHaveCount(0);
    await expect(page.locator('.timeline .turn.prop')).toHaveCount(0);
    // 收在 `.turn` 上：时间线那一块里还有班次 chip 行（切换 / 新建 / 改名 / 归档四颗钮），
    // 它们不是「后端下发的动作」，不在本条断言的射程里
    await expect(page.locator('.timeline .turn button')).toHaveCount(0);
    await expect(
      page.locator('.timeline .turn.mine', { hasText: '现在能做什么' }),
    ).toHaveCount(1);
    // 发送成功才清空输入框（失败时保留，见 talk.svelte 的 send）
    await expect(input).toHaveValue('');
    await expect(page.locator('.talk-head .ts')).toContainText(/本次会话 [1-9]\d* tok/);

    expectBundleHealthy(bundle);
  });
});

test.describe('对讲台 · 发不出去时不清空输入框（票 01 的硬约束）', () => {
  let app: App;

  test.beforeAll(async () => {
    // provider 指向一个恒定报错的 mock：值班长必然答不上话。
    // 这正是「后端在叫模型**之前**已把 user 行落库」那条时序的现场（决策 182㉓）。
    app = await startApp({
      script: foremanScript([[]]),
      providerOnly: true,
      badProvider: { status: 401, body: '{"error":{"message":"bad key"}}' },
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('发送失败：错误轮进时间线、人说过的话仍在台账里、输入框内容保留', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const input = page.locator('.typer textarea');
    await input.fill('这句话要能改几个字再发');
    await page.getByRole('button', { name: /发送/ }).click();

    // 失败以**时间线里的一轮**呈现：不弹窗、不 toast
    const failed = page.locator('.timeline .turn.failed');
    await expect(failed).toContainText('发送失败', { timeout: 30_000 });
    await expect(failed.locator('.dname')).toHaveText('发送失败');

    // 人说过的话没丢（后端先落库、后叫模型）
    await expect(
      page.locator('.timeline .turn.mine', { hasText: '这句话要能改几个字再发' }),
    ).toHaveCount(1);
    // **输入框内容不清空**：人改几个字就能重发，而不是重打一遍
    await expect(input).toHaveValue('这句话要能改几个字再发');

    expectBundleHealthy(bundle);
  });
});

/**
 * E2E-⑩ 补：**输入法里敲英文再回车，不能把半截话发出去**（决策 184）。
 *
 * 这是桌面壳上必现的一条（WKWebView 先发 `compositionend` 再发那次 `keydown`，
 * `event.isComposing` 已经是 `false`），故只查 `isComposing` 的护栏挡不住它。
 * Chromium 的次序与 WebKit 不同，所以这里**按 WebKit 的次序合成事件**：
 * `compositionstart` → `compositionend` → 同一个任务里的 `keydown(Enter)`。
 * 护栏若只认 `isComposing`，这一条会打红（消息被提前发出）。
 *
 * 断言口径：只看**外部行为**——时间线里有没有多出「我」那一轮。
 */
test.describe('对讲台 · 输入法回车不发送（决策 184）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: foremanScript([[]]), providerOnly: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('选字那一次回车不发送；人手下一次回车照常发送', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const input = page.locator('.typer textarea');
    // 组合态：中文输入法里敲英文（`isComposing` 那一刻已是 false —— WebKit 的次序）。
    // **同一个任务里**发 `compositionend` + 那次 `keydown`，这就是 WKWebView 的次序；
    // 事件都带 `bubbles: true`，因为 Svelte 5 把会冒泡的事件代理到根节点上。
    await input.evaluate((el) => {
      const fire = (type: string, init: EventInit = {}) =>
        el.dispatchEvent(new Event(type, { bubbles: true, cancelable: true, ...init }));
      fire('compositionstart');
      (el as HTMLTextAreaElement).value = 'hello';
      fire('input');
      fire('compositionend');
      fire('keydown', { key: 'Enter' } as unknown as EventInit);
    });

    // 选字那一次不得产生任何一轮对话
    await expect(page.locator('.timeline .turn.mine')).toHaveCount(0);
    // 文本仍在输入框里（没被当成「已发送」清掉）
    await expect(input).toHaveValue('hello');

    // 人手下一次回车是独立的输入事件（远在 50ms 窗口之外），必须照常发送
    await page.waitForTimeout(120);
    await input.press('Enter');
    await expect(page.locator('.timeline .turn.mine', { hasText: 'hello' })).toHaveCount(1, {
      timeout: 30_000,
    });

    expectBundleHealthy(bundle);
  });
});

/**
 * 对讲台 · 班次（决策 204）。
 *
 * 一条长会话改成一排可以新建 / 切换 / 重命名 / 归档的班次。这一组要钉的是**边界**：
 *
 * - 班次 chip 行**不是第三件钉住物**（决策 192 的窄屏只有两件）也不在页头里（`.talk-head`
 *   在 `row auto` 上，涨的 px 直接吃对话区）——故两条断言：不 sticky、对话区几何不变；
 * - **换会话 ≠ 换看板**：切班次重置的是这一屏读到的台账，状态区的急停（派生自全局看板）
 *   一个字都不该动；
 * - 归档后**自动**切到最近有说话的班次，不是把善后留给使用者。
 */
test.describe('对讲台 · 班次（决策 204）', () => {
  let app: App;
  const title = 'E2E 班次';

  test.beforeAll(async () => {
    app = await startApp({
      script: {
        ...fullPassScript('E2E'),
        ...foremanScript(Array.from({ length: 8 }, () => [text('夜班安静，没有待办。')])),
      },
      title,
    });
    // 状态区得有一张真的急停，才能证明「切班次不动看板」
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval', 180_000);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('新建 / 切换 / 重命名 / 归档，且换会话不动看板', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const row = page.locator('.timeline .runrow');
    await expect(row).toBeVisible();

    // ① 首启一个班次都没有：chip 行只有「+ 新班次」那一颗，没有改名 / 归档（还没有当前班）
    const chips = row.locator('.runchip:not(.plus):not(.act)');
    await expect(chips).toHaveCount(0);
    await expect(row.locator('.runchip.plus')).toHaveText('+ 新班次');
    await expect(row.locator('.runchip.act')).toHaveCount(0);

    // ② **非 sticky**：它随手指滚，不是第三件钉住物（决策 192 的窄屏只有两件）
    await expect(row).toHaveCSS('position', 'static');
    // 也**不在页头里**：页头高度不因它变（那只会在 `row auto` 上吃掉对话区）
    await expect(page.locator('.talk-head .runrow')).toHaveCount(0);

    // ③ 空态：没有班次时也照样能说话——第一次说话会开一个班次（服务端兜底）
    const input = page.locator('.typer textarea');
    await input.fill('第一班的问题');
    await page.getByRole('button', { name: /发送/ }).click();
    await expect(page.locator('.timeline .turn.fm').first()).toContainText('夜班安静', {
      timeout: 30_000,
    });

    // ④ 新班次出现了，标题取自第一句话（决策 204②）；改名 / 归档随之出现在有当前班次时
    await expect(chips).toHaveCount(1);
    await expect(row.locator('.runchip.act')).toHaveCount(2);
    const firstChip = chips.first();
    await expect(firstChip).toHaveText('第一班的问题');
    await expect(firstChip).toHaveClass(/now/);

    // ⑤ 换会话 ≠ 换看板：状态区的急停（派生自全局看板）一个字都不动
    const stopText = await page.locator('.zone-status .turn.warn').first().innerText();

    // ⑥ 新建一个班次：切过去、时间线回到空态、急停仍在
    await row.locator('.runchip.plus').click();
    await expect(chips).toHaveCount(2);
    await expect(page.locator('.timeline .turn')).toHaveCount(0);
    await expect(row.locator('.runchip.now')).toContainText('新班次');
    await expect(page.locator('.zone-status .turn.warn').first()).toHaveText(stopText);

    // ⑦ 切回第一班：它的对话回来了（上下文按班次隔离）
    await row.locator('.runchip', { hasText: '第一班的问题' }).click();
    await expect(
      page.locator('.timeline .turn.mine', { hasText: '第一班的问题' }),
    ).toHaveCount(1);
    await expect(page.locator('.timeline .turn.fm').first()).toContainText('夜班安静');

    // ⑧ 重命名走 Modal（决策 204③）
    await row.locator('.runchip.act', { hasText: '改名' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await dialog.locator('input[type="text"]').fill('周三夜班');
    await dialog.getByRole('button', { name: '改名' }).click();
    await expect(dialog).toBeHidden();
    await expect(row.locator('.runchip.now')).toHaveText('周三夜班');

    // ⑨ 归档：从列表里收起来，并**自动切到最近有说话的班次**（不是留一个不在列表里的当前班）
    await row.locator('.runchip.act', { hasText: '归档' }).click();
    const archiveDialog = page.getByRole('dialog');
    await expect(archiveDialog).toBeVisible();
    await archiveDialog.getByRole('button', { name: '归档' }).click();
    await expect(archiveDialog).toBeHidden();
    await expect(row.locator('.runchip', { hasText: '周三夜班' })).toHaveCount(0);
    // 剩下的那一班里应该就看得到它自己的空态（它是空班次），且当前班次是它
    await expect(row.locator('.runchip.now')).toHaveText('新班次');
    await expect(page.locator('.zone-status .turn.warn').first()).toHaveText(stopText);

    expectBundleHealthy(bundle);
  });

  test('窄屏：chip 行横滚而不折行，对话区几何不被它吃掉', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });

    // 造够班次让 chip 行真的溢出（窄屏上三四个就够）
    for (const t of ['夜班甲', '夜班乙', '夜班丙']) {
      const res = await fetch(`${app.apiBase}/foreman/sessions`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
        body: JSON.stringify({ title: t }),
      });
      expect(res.ok).toBeTruthy();
    }

    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const row = page.locator('.timeline .runrow');
    await expect(row.locator('.runchip').first()).toBeVisible();
    // 不折行 + 横向滚（与任务详情页的 `.runrow` 同一形状）：430px 上折行会占掉两三行，
    // 而这一档的纵向空间是「两条钉住物之间的残渣」，不能喂给一排控件
    await expect(row).toHaveCSS('flex-wrap', 'nowrap');
    await expect(row).toHaveCSS('overflow-x', 'auto');
    const overflow = await row.evaluate((el) => el.scrollWidth - el.clientWidth);
    expect(overflow, 'chip 行在窄屏没有横滚余地（班次不够多？）').toBeGreaterThan(0);

    // **对话区仍是「整块屏幕减去两条钉住物」**（决策 192 的那条几何断言没有被 chip 行吃掉）：
    // chip 行长在会滚的时间线**里面**，故时间线自己的盒子一点没变。
    const timelineBox = await page.locator('.timeline').boundingBox();
    expect(timelineBox?.height ?? 0).toBeGreaterThanOrEqual(320);

    expectBundleHealthy(bundle);
  });
});
