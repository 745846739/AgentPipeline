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
 * 急停那轮的后端下发动作可下发、给值班长发话后它的回复里没有按钮、两张急停同挂时
 * 两张都留在第一屏（决策 183）。
 */

import { expect, test, type Locator } from '@playwright/test';
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
 * 回执那一轮要验的是「回执怎么渲染」，两步各钉一个标签：
 *
 * ① 查一个**不存在**的任务 → 工具执行成功、台账里没这个号，标签是**已读**。
 *    「查无此任务」不是工具故障（决策 33 的分层：它不该累计 `tool_retry_max`），
 *    值班长收到的是可转述的文本，不是错误。
 * ② 发一个**不在白名单里**的工具 → 在执行点被拒，标签是**未读到**。
 *    这一步顺带把票 02 的安全边界钉在界面上：值班长调不动越权工具。
 *
 * 工位来源（`stage`）不在这条用例里断言：它要把**真实**任务 id 写进脚本，而 id 由后端
 * 在 `startApp` 之后生成，mock 没有事后注入脚本的口子。来源渲染由 Talk.svelte 的
 * 回执分支按 `trace.stage` 读出，属理由可证、e2e 不可达。
 */
const UNKNOWN_TASK_ID = '01K0000000000000000000000X';

/** 十二轮回话（每轮先查台账、再试一个越权工具）；留足余量给 CI 的一次重试。 */
const foremanRounds = foremanScript(
  Array.from({ length: 12 }, () => [
    readTask(UNKNOWN_TASK_ID),
    tool('run_command', { command: 'echo pwned' }),
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
    // 值班员说的那句也在时间线里（台账那一行，不是只在输入框里）
    await expect(
      page.locator('.timeline .turn.mine', { hasText: '班次问句 · 标记 talk03' }),
    ).toHaveCount(1);

    // **值班长的回复里永远没有按钮**（票 04 的硬要求）：写动作只在状态区的急停轮里
    await expect(reply.locator('button')).toHaveCount(0);

    // 工位回执留在对话里：这一轮查过台账、也试过一个越权工具 → 两条回执挂在回话那一轮内，
    // 形状与发言**不同**——左缘 4px 亮度阶、无框（转述不是发言）
    const rcpt = reply.locator('.rcpt');
    await expect(rcpt).toHaveCount(2);
    const ledger = rcpt.filter({ hasText: UNKNOWN_TASK_ID });
    await expect(ledger).toContainText('读任务台账');
    // 查无此任务是「已读」而不是「未读到」：工具执行成功了，只是台账里没这个号
    await expect(ledger).toContainText('已读');
    // 越权工具在执行点被拒 → 「未读到」，且那条命令**没有真的跑起来**
    const denied = rcpt.filter({ hasText: 'run_command' });
    await expect(denied).toContainText('未读到');
    await expect(rcpt.locator('svg.sprite').first()).toBeVisible();
    await expect(rcpt.first()).toHaveCSS('border-left-width', '4px');
    await expect(rcpt.first()).toHaveCSS('border-top-width', '0px');

    // 值班长不占琥珀档：它的轮次与急停那一轮不是同一个描边色（全站唯一的响仍在急停一处）
    const stopColor = await page
      .locator('.zone-status .turn.warn')
      .evaluate((el) => getComputedStyle(el).borderTopColor);
    const replyColor = await reply.evaluate((el) => getComputedStyle(el).borderTopColor);
    expect(replyColor).not.toBe(stopColor);

    // 会话合计来自台账（不是编的 0）
    await expect(page.locator('.talk-head .ts')).toContainText(/本次会话 [1-9]\d* tok/);

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

    // 空态的「去看板新建任务」是**页面固定的导航钮**：它不在任何对话框（`.turn`）里，
    // 也不来自 allowed_actions——此刻全页没有任何后端下发的动作。
    // 它的出现同时证明看板已经装载完（空态等的是装载完成，不是"还没读"）。
    const navBtn = page.getByRole('button', { name: '去看板新建任务' });
    await expect(navBtn).toBeVisible();
    await expect(page.locator('.zone-status .turn button')).toHaveCount(0);

    // 验收锚点：空看板照样能对话
    await input.fill('现在能做什么');
    await page.getByRole('button', { name: /发送/ }).click();

    const reply = page.locator('.timeline .turn.fm').first();
    await expect(reply).toContainText('夜班安静', { timeout: 30_000 });
    await expect(reply.locator('button')).toHaveCount(0);
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
