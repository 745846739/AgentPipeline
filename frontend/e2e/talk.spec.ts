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

import { expect, test, type Locator, type Page } from '@playwright/test';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
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
 *
 * `sessionId`（决策 220③ 的「跨设备那一组」）：以**别的班次**的名义说一句话——
 * 它的流式增量会广播到本机那条 `/foreman/stream` 上、带着另一个 `session_id`，
 * 正是「正在回话」那枚标记要接住的东西。
 *
 * 返回落地那个班次的 id。**不给 `sessionId` 不是「新开一个班次」**：服务端在缺省时落到
 * 「最近活动的未归档班次」（`resolve_session`），故它会接着上一个用例留下的班次说话——
 * 要一个干净的班次，先 `POST /foreman/sessions`（`makeSession`）拿到 id 再往它里面说。
 * 另有一条命名规则要记着：班次的名字取自**它第一句话**（决策 204②），空班次上先起好名
 * 再说话的话，名字会被那句话冲掉。
 */
async function sayDirect(app: App, body: string, sessionId?: string): Promise<string> {
  const res = await fetch(`${app.apiBase}/foreman/messages`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
    body: JSON.stringify(sessionId ? { text: body, session_id: sessionId } : { text: body }),
  });
  if (!res.ok) throw new Error(`POST /foreman/messages -> ${res.status}: ${await res.text()}`);
  const payload = (await res.json()) as { session: { id: string } };
  return payload.session.id;
}

/** 开一个班次（绕过界面；跨设备那一组要的是「列表里有它，但本机没在它里面说话」）。 */
async function makeSession(app: App, title: string): Promise<string> {
  const res = await fetch(`${app.apiBase}/foreman/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
    body: JSON.stringify({ title }),
  });
  if (!res.ok) throw new Error(`POST /foreman/sessions -> ${res.status}: ${await res.text()}`);
  const payload = (await res.json()) as { session: { id: string } };
  return payload.session.id;
}

/** 整页横向溢出的像素数。 */
async function horizontalOverflow(page: Page): Promise<number> {
  return page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
}

/** 整页纵向溢出的像素数（静置态应当 ≤1：多出来的每一像素都是从对话区借的）。 */
async function verticalOverflow(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      Math.max(document.documentElement.scrollHeight, document.body.scrollHeight) -
      window.innerHeight,
  );
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
    await expect(turn.locator('.dtag')).toContainText('急停');
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

  test('折行档（430×900）：页头收成一行、值班板整块不渲染、班次行进 ⋯', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.talk')).toBeVisible({ timeout: 60_000 });

    // 顶栏高度是 §5 的两处定值（`--topbar-h` 的来源，也是各钉位的依据）。
    // 对讲台**不是看板路由**：窄档道具栏行不露出、导航行又已移到屏幕底部（决策 243），
    // 顶栏**清零**——0 也是 `--topbar-h` 的合法值（`TopBar` 的 effect 不再挡 0）。
    const headerBox = await page.locator('header.top').boundingBox();
    expect(headerBox?.height).toBe(0);

    // ① 页头 = **46px 的钉住带子**（44px 行 + 2px 下框，决策 218 修订 ④）：
    // ⋯ 的 44px 触控目标因此直接落在行内，不需要任何溢出技巧。
    const headBox = await page.locator('.talk-head').boundingBox();
    expect(headBox?.height, '页头带子应当是 46px').toBe(46);
    // `<h1>` 视觉让位（visually-hidden：宽度 ≤1px），语义仍在（审计 R2-20 要求每路由有 h1）
    const ttBox = await page.locator('.talk-head .tt').boundingBox();
    expect(ttBox?.width ?? 99, '<h1> 应当 visually-hidden').toBeLessThanOrEqual(1);
    // 那一行是 19.2px 的次级读法（不折行）
    const lineBox = await page.locator('.talk-head .ts').boundingBox();
    expect(lineBox?.height ?? 99, '页头那一行应当是一行 19.2px').toBeLessThanOrEqual(20);
    // 右端的 ⋯：命中区 ≥44px（触控底线）
    const moreBox = await page.locator('.talk-head .more').boundingBox();
    expect(moreBox?.height ?? 0, '⋯ 的命中区应当 ≥44px').toBeGreaterThanOrEqual(44);
    expect(moreBox?.width ?? 0).toBeGreaterThanOrEqual(44);

    // ② 值班板整块不渲染（决策 218 ②：同一份读数在看板 8 列与顶栏灯带上各有一份，这是第三份）
    await expect(page.locator('.talk-side')).toBeHidden();
    expect(
      await page.locator('.talk .brow:visible').count(),
      '值班板灯条不该还在页上',
    ).toBe(0);
    // 班次行也收进了 ⋯：页面上没有 chip 行
    await expect(page.locator('.runrow')).toHaveCount(0);

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
 * 确认钮**够得到**（决策 208 的回归门）。
 *
 * 四条一起看才成立——只测「钮可见」在一条缝里也是绿的（元素可见，只是点不到）：
 *   ① 时间线拿到了它的下限（`--timeline-floor`，§3.3 定值 160px）；
 *   ② 钮**中心点**命中的是钮自己，不是输入坞悬出框沿的名牌（2026-09-17 的实测里，
 *      把点击截走的正是它）；
 *   ③ 输入坞与整页都没被顶坏——零和的分法不许拆东墙补西墙。
 */
async function expectProposalReachable(page: Page): Promise<void> {
  const timeline = page.locator('.timeline');
  const btn = timeline.locator('.turn.prop .pacts button').first();
  await expect(btn).toBeVisible();

  // ① 这一格拿到了它的下限（26px 那次它只够放下自己的上下内边距）
  const tl = await timeline.boundingBox();
  expect(tl).not.toBeNull();
  expect(
    tl?.height ?? 0,
    '时间线短于下限：确认钮所在的这一格又被挤成一条缝',
  ).toBeGreaterThanOrEqual(160);

  // ② 滚进这一格之后它**整颗**在里面（这也就是点击前 playwright 会做的滚屏）
  await btn.scrollIntoViewIfNeeded();
  const b = await btn.boundingBox();
  expect(b).not.toBeNull();
  expect(b?.y ?? 0, '钮没被完全滚进时间线').toBeGreaterThanOrEqual((tl?.y ?? 0) - 1);
  expect(
    (b?.y ?? 0) + (b?.height ?? 0),
    '钮没被完全滚进时间线',
  ).toBeLessThanOrEqual((tl?.y ?? 0) + (tl?.height ?? 0) + 1);

  // ③ 它的中心点命中的是它自己——不是悬出输入坞框沿的名牌（实测里正是它截走的点击）
  const cx = Math.round((b?.x ?? 0) + (b?.width ?? 0) / 2);
  const cy = Math.round((b?.y ?? 0) + (b?.height ?? 0) / 2);
  const hit = await page.evaluate(
    ([x, y]) => {
      const el = document.elementFromPoint(x, y);
      return el ? `${el.tagName}.${String(el.className)}` : 'null';
    },
    [cx, cy] as [number, number],
  );
  expect(hit, '钮的中心点被别的东西截走了').toContain('BUTTON');

  // ④ 代价不许落在输入坞与整页上（零和的分法不能拆东墙补西墙）
  await expect(page.locator('.typer button[type=submit]')).toBeInViewport();
  const pageOverflow = await page.evaluate(
    () => document.documentElement.scrollHeight - window.innerHeight,
  );
  expect(pageOverflow, '整页长出了滚动条（急停钉在第一屏就只剩口头保证）').toBeLessThanOrEqual(0);
}

/**
 * 对讲台 · 两张急停同时挂在状态区（决策 183）。
 *
 * **这条用例钉的是折叠的存在理由**：一张急停轮内联着后端下发的动作集，最高的一种形状
 * （`info_insufficient`：带补充输入的 resume + 旁路动作）约 330px，而状态区上限桌面是
 * 「46vh 与『先留给时间线的那一份』两项取小」（决策 208），13″ 上可用约 202px——
 * **展开一张就已经占满整个区**。不折叠的话，两张同挂时
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
    // 桌面最紧的一档（也是 playwright 默认视口）：上限 min(46vh≈331px, 720-386-160=174px)
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
    // **1280×720 就是这一档的口径**（playwright 默认视口，也是三分区最紧的一档）：
    // 这条用例原先被逼着显式设 1280×1000 绕开「时间线被挤成一条缝、确认钮点不到」
    // （决策 207③ 的落地缺口），决策 208 修掉之后回到默认视口——缝回来了这条就红。
    await page.setViewportSize({ width: 1280, height: 720 });

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

    // **够得到吗**（决策 208 的回归门）：三分区是零和的，720px 是它最紧的一档
    await expectProposalReachable(page);

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
    // 与上一条同一档：默认视口，不给它留高度（决策 208 修的就是「不给也点得到」）
    await page.setViewportSize({ width: 1280, height: 720 });

    await sayDirect(app, '再提一条看看');
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const prop = page.locator('.timeline .turn.prop').last();
    await expect(prop).toBeVisible({ timeout: 30_000 });
    await expectProposalReachable(page);
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

    // 班次行挂在**页头右端**（决策 218 Q15），故时间线滚到底它仍在视口里——
    // 原先它长在这个滚动容器**里面**，实测滚到底时已经在屏幕上方 320.2px，
    // 那正是「新建对话要往上翻很久」的病根。
    const row = page.locator('.talk-head .runrow');
    await expect(row).toBeVisible();
    await expect(page.locator('.timeline .runrow'), '班次行不该还在时间线里').toHaveCount(0);
    await expect(row.locator('.runchip.plus')).toBeInViewport();
    await expect(page.locator('.talk-head .runchip[aria-pressed="true"]')).toHaveCount(1);
    // 页头高度没因它变：**页头 = 标题那一行**。拿 `<h1>` 的行盒当基准而不是写死数字——
    // 改字号/行距时它会自己跟上，而「班次行把页头撑高」照样红（班次行一旦换行或高于标题行，
    // 页头就会高过标题行）。`--talk-chrome: 386px` 里预留的「页头 38px」（决策 209 逐块量的）
    // 比实测的 28.8px 多 9px，那几像素只让状态区少拿、时间线多拿，方向是安全的（决策 218 ⑧
    // 明写这个常量不动）。
    const headBox = await page.locator('.talk-head').boundingBox();
    const ttlBox = await page.locator('.talk-head .tt').boundingBox();
    expect(headBox?.height ?? 0, '班次行把页头撑高了').toBeLessThanOrEqual((ttlBox?.height ?? 0) + 1);

    expectBundleHealthy(bundle);
  });

  /**
   * 工位回执的默认态**分档**（决策 218 ②/Q11）：桌面照旧展开、折行档默认收起。
   *
   * 这条要用**界面真的发一句话**（`page.route` 把那一次 POST 拖住 2.5s），因为要验的第二件事
   * 是「手动展开的那一轮不被流式增量打回」——增量只在 `sending` 期间进时间线，
   * 直连铺长（`sayDirect`）产生的是别的浏览器上下文的流，进不了这一屏。
   */
  test('折行档：工位回执默认收起，手动展开后不被流式增量打回（票 06）', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });
    // 先直连说一句：拿到一轮**带回执**的回话（脚本每轮都查一次台账）
    await sayDirect(app, '窄屏回执看一眼');
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const reply = page.locator('.timeline .turn.fm', { hasText: REPLY_MARK }).last();
    await expect(reply).toBeVisible({ timeout: 30_000 });
    const details = reply.locator('details.rcpts');
    await expect(details).toHaveCount(1);

    // 默认收起：内容不可见，而 summary 仍把**出处与条数**说全（可追溯性只是换了个开销方式）
    await expect(details.locator('.rcpt').first()).toBeHidden();
    await expect(details.locator('summary')).toContainText('次台账查读');

    // 人点一下 → 展开
    await details.locator('summary').click();
    await expect(details.locator('.rcpt').first()).toBeVisible();

    // 让这一趟慢下来：流式增量因此真的落在这个窗口里（决定性的「再来一次增量」）
    await page.route('**/foreman/messages', async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 2500));
      await route.continue();
    });
    await page.locator('.typer textarea').fill('再来一句');
    await page.locator('.typer button[type=submit]').click();
    await expect(page.locator('.timeline .turn p.streaming')).toBeVisible({ timeout: 30_000 });

    // **受控展开态**：流式增量反复重渲染同一轮，也把人手动展开的那一轮打不回去
    await expect(details.locator('.rcpt').first()).toBeVisible();

    expectBundleHealthy(bundle);
  });
});

/**
 * 对讲台 · 折行档（决策 192 / 218）：版面口径是**整页随手指滚，钉住的是三样**
 * （页头 46px 带子 / 急停摘要条 / 输入坞）。
 *
 * 桌面靠「整页钉住 + 时间线是唯一滚动容器」；这一档反过来——页头那一行钉在顶栏下沿
 * （`top: var(--topbar-h)`，带子 46px），急停摘要条钉在**它的**下沿，输入坞钉在底栏上沿
 * （`bottom: var(--sbar-h)`），对话从三者之间滚过去。
 *
 * **这一组自带装置**（不复用上面那组的）：那组最后一条用例会把唯一的急停按掉（点合入），
 * 之后再进来就没有 pending 了——几何断言会退化成「空状态区当然不挤」，绿灯但无意义。
 */
test.describe('对讲台 · 折行档（决策 192 / 218）', () => {
  let app: App;
  const title = 'E2E 窄屏长对话';

  test.beforeAll(async () => {
    app = await startApp({
      script: {
        ...archBlockerRounds(),
        // 每轮 10 行（同 `FOREMAN_REPLY` 的量级）：三轮就足以让整页在 900px 上滚起来
        ...foremanScript(
          Array.from({ length: 12 }, () => [
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
   * 再把剩下的分完。所以断言写成**几何**（对话区的高度、三条钉住物的贴合），而不是
   * 「CSS 里有没有 sticky」——后者在版面塌掉时照样为真。
   *
   * 必须排在本组第一条：后面的用例会把对话铺长（那时「不空滚」不再成立，是应该的）。
   */
  test('静置版面（430×900）：对话区拿到余下的整块、三条钉住物各就各位', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.zone-status .turn.warn')).toHaveCount(1, { timeout: 60_000 });

    // ① 单张急停也折成摘要条（桌面那一档它应当是展开的——同一装置两种版面）
    await expect(page.locator('.zone-status .turn.warn.folded')).toHaveCount(1);
    // ② 状态区退出区内滚动：这一档没有「状态区自己滚」这回事，去滚的是整页
    const zoneOverflow = await page
      .locator('.zone-status')
      .evaluate((el) => el.scrollHeight - el.clientHeight);
    expect(zoneOverflow, '状态区在折行档仍然区内滚').toBeLessThanOrEqual(4);

    // ③ **三条钉住物**（决策 218 修订 ④：从两只变三只）各钉在自己的下沿上。
    // 静置态整页还滚不动，sticky 因此还没生效（带子仍在容器那 10px 上内边距之下）——
    // 「钉在顶栏下沿」由下面那条「滚到底」的用例量，这里量的是**两者的相对关系**：
    // 摘要条紧贴带子的下沿（钉位 138 → 184 那一处修订的可见后果）。
    const headerBox = await page.locator('header.top').boundingBox();
    const headBox = await page.locator('.talk-head').boundingBox();
    expect(
      (headBox?.y ?? 0) - ((headerBox?.y ?? 0) + (headerBox?.height ?? 0)),
      '页头带子没紧接着顶栏',
    ).toBeLessThanOrEqual(10);
    const zoneBox = await page.locator('.zone-status').boundingBox();
    expect(
      Math.abs((zoneBox?.y ?? 0) - ((headBox?.y ?? 0) + (headBox?.height ?? 0))),
      '急停摘要条没钉在页头带子下沿（钉位 138 → 184 那一处修订）',
    ).toBeLessThanOrEqual(2);

    // ④ **对话区是「整块屏幕减去三条钉住物」**，不是它们之间的残渣。
    // 实测 430×900：盒子 448px（内容区 422px；决策 218 修订 ④ 的账是 427.7px）
    const timelineBox = await page.locator('.timeline').boundingBox();
    expect(timelineBox?.height ?? 0, '对话区又被挤小了').toBeGreaterThanOrEqual(440);

    // ⑤ 输入坞钉在底栏（`.statusline`）上沿：底边与底栏顶边不许有缝
    const typerBox = await page.locator('.typer').boundingBox();
    const sbarBox = await page.locator('.statusline').boundingBox();
    expect(typerBox).not.toBeNull();
    expect(sbarBox).not.toBeNull();
    expect(
      Math.abs((typerBox?.y ?? 0) + (typerBox?.height ?? 0) - (sbarBox?.y ?? 0)),
      '输入坞没有贴在底栏上沿',
    ).toBeLessThanOrEqual(1);

    // ⑥ 整页不空滚：没有对话时文档高度就是视口高度（多出来的每一像素都是从对话区借的）
    expect(await verticalOverflow(page), '折行档整页在没有对话时也能滚').toBeLessThanOrEqual(1);
    expect(await horizontalOverflow(page)).toBeLessThanOrEqual(0);

    expectBundleHealthy(bundle);
  });

  /**
   * 375×667（iPhone SE）那一档**从来没有被量过**（决策 192 的「能放下」只在 430 宽上成立过），
   * 而它当时静置态就溢出 37px、对话区只剩 156.8px。这一条把那两个数钉住（决策 218 修订 ③/⑥）。
   */
  test('静置版面（375×667）：不再有静置态溢出', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 375, height: 667 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.zone-status .turn.warn')).toHaveCount(1, { timeout: 60_000 });

    expect(await verticalOverflow(page), '375×667 上静置态整页溢出').toBeLessThanOrEqual(1);
    expect(await horizontalOverflow(page)).toBeLessThanOrEqual(0);
    // 对话区：实测盒子 215px（决策 218 修订 ④ 的账是约 195px 内容区）
    const timelineBox = await page.locator('.timeline').boundingBox();
    expect(timelineBox?.height ?? 0).toBeGreaterThanOrEqual(205);
    // 一行页头 + ⋯：这一档也一样（同一条 899 的规则）
    expect((await page.locator('.talk-head').boundingBox())?.height).toBe(46);
    expect((await page.locator('.talk-head .more').boundingBox())?.height ?? 0).toBeGreaterThanOrEqual(
      44,
    );
    expectBundleHealthy(bundle);
  });

  test('整页滚到底之后，三条钉住物都还在（⋯ 恒在手边是这次钉页头的全部意义）', async ({ page }) => {
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

    // 这一档滚的是**整页**（时间线不再是滚动容器）
    const timelineOverflow = await page
      .locator('.timeline')
      .evaluate((el) => el.scrollHeight - el.clientHeight);
    expect(timelineOverflow, '折行档的时间线仍是滚动容器').toBeLessThanOrEqual(1);
    expect(await verticalOverflow(page), '长对话没有把整页撑出滚动').toBeGreaterThan(0);

    // 滚到底
    const scrolled = await page.evaluate(() => {
      const d = document.scrollingElement as HTMLElement;
      d.scrollTop = d.scrollHeight;
      return d.scrollTop;
    });
    expect(scrolled).toBeGreaterThan(0);

    // **硬要求**①：滚到底时页头那一条带子仍钉在顶栏下沿
    const headerBox = await page.locator('header.top').boundingBox();
    const headBox = await page.locator('.talk-head').boundingBox();
    expect(
      Math.abs((headBox?.y ?? 0) - ((headerBox?.y ?? 0) + (headerBox?.height ?? 0))),
      '滚到底后页头带子没钉在顶栏下沿',
    ).toBeLessThanOrEqual(2);
    // **硬要求**②：急停摘要条钉在带子下沿，且那一张摘要条还在视口里
    const zoneBox = await page.locator('.zone-status').boundingBox();
    expect(
      Math.abs((zoneBox?.y ?? 0) - ((headBox?.y ?? 0) + (headBox?.height ?? 0))),
      '滚到底后急停摘要条没钉在页头带子下沿',
    ).toBeLessThanOrEqual(2);
    await expect(page.locator('.zone-status .turn.warn').first()).toBeInViewport();
    await expect(page.locator('.zone-status .turn.warn').first()).toContainText(title);
    // **硬要求**③：⋯ 恒在手边（班次动作含开新对话不必滚回顶部——用户诉求指向的正是这件事）
    await expect(page.locator('.talk-head .more')).toBeInViewport();

    // 输入坞也还在（同一时刻三样都在 = 这一档版面的全部合同）
    const typerBox = await page.locator('.typer').boundingBox();
    const sbarBox = await page.locator('.statusline').boundingBox();
    expect(
      Math.abs((typerBox?.y ?? 0) + (typerBox?.height ?? 0) - (sbarBox?.y ?? 0)),
      '滚到底后输入坞没贴在底栏上沿',
    ).toBeLessThanOrEqual(1);
    await expect(page.locator('.typer textarea')).toBeInViewport();

    expectBundleHealthy(bundle);
  });

  test('⋯ 班次菜单：三条出口、命中区 ≥44px、当前项有选中语义、切换真的换班', async ({ page }) => {
    const bundle = watchBundle(page);
    await makeSession(app, '夜班甲');
    await makeSession(app, '夜班乙');
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const more = page.locator('.talk-head .more');
    const menu = page.locator('#talk-session-menu');
    await expect(more).toBeVisible({ timeout: 30_000 });
    expect((await more.boundingBox())?.height ?? 0, '⋯ 命中区应当 ≥44px').toBeGreaterThanOrEqual(44);

    // 开：`aria-expanded` 与面板同时到位（`aria-controls` 指向的目标常驻 DOM）
    await more.click();
    await expect(more).toHaveAttribute('aria-expanded', 'true');
    await expect(menu).toBeVisible();
    // 第一项是「+ 新班次」（这一页最常按的一颗，也是班次动作的唯一入口）
    await expect(menu.locator('button').first()).toHaveText('+ 新班次');
    // 行高按触控来（≥44px），不复用 20px 的芯片尺寸
    const plus = await menu.locator('button').first().boundingBox();
    expect(plus?.height ?? 0).toBeGreaterThanOrEqual(44);

    // 出口①：Escape 关得掉——**焦点从没进过面板时也算**（顶栏那条实测里的场景）
    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
    await expect(more).toHaveAttribute('aria-expanded', 'false');

    // 出口②：ArrowDown 从触发钮进第一项
    await more.focus();
    await page.keyboard.press('ArrowDown');
    await expect(menu).toBeVisible();
    await expect(menu.locator('button').first()).toBeFocused();

    // 出口③：点面板外面关掉
    await page.mouse.click(200, 640);
    await expect(menu).toBeHidden();

    // 当前班次是身份行，既可见又播报
    await more.click();
    await expect(menu.locator('.mi.now')).toHaveAttribute('aria-current', 'true');
    const current = (await menu.locator('.mi.now .mi-nm').innerText()).trim();
    const other = current === '夜班甲' ? '夜班乙' : '夜班甲';

    // 切换：时间线换成那一条、页头的班次名跟着变、地址写 `?session=`（决策 217①：
    // ⋯ 必须是它的消费者，不是替代品）
    await menu.locator('button.mi', { hasText: other }).click();
    await expect(page.locator('.talk-head .sess-name')).toHaveText(other);
    await expect.poll(() => new URL(page.url()).hash, { timeout: 10_000 }).toContain('session=');
    await more.click();
    await expect(menu.locator('.mi.now .mi-nm')).toHaveText(other);

    expectBundleHealthy(bundle);
  });

  /* 决策 218 ⑥ 的「顶栏信号灯跳段」用例已随**铭牌行整行退场**删除：窄档顶栏不再有
     `.railnav`（导航行升为首行、道具栏行只在看板露出），那颗灯没有了，判据自然无处落。 */

  test('输入坞：没有提示语行，常态 88px、贴底栏上沿', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const typerBox = await page.locator('.typer').boundingBox();
    const sbarBox = await page.locator('.statusline').boundingBox();
    expect(
      Math.abs((typerBox?.height ?? 0) - 88),
      '坞常态应当是 88px（撤掉那行提示语之后，决策 218 当日修订 ②）',
    ).toBeLessThanOrEqual(1);
    expect(
      Math.abs((typerBox?.y ?? 0) + (typerBox?.height ?? 0) - (sbarBox?.y ?? 0)),
      '输入坞没有贴在底栏上沿',
    ).toBeLessThanOrEqual(1);

    // 那一行整行撤掉了：坞里既没有「说的每句话都会记进审计」，也没有「值班长正在回话…」
    // （撤的是**告知**，不是纪律：审计照旧全量落库，决策 218 当日修订 ⑤ / 220④）
    await expect(page.locator('.typer')).not.toContainText('记进审计');
    await expect(page.locator('.typer')).not.toContainText('正在回话');
    await expect(page.locator('.typer .hint')).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test('掐断 SSE：坞里出现**唯一**那句断线告知，空闲时不占高', async ({ page }) => {
    const bundle = watchBundle(page);
    let blocked = true;
    await page.route('**/foreman/stream', async (route) => (blocked ? route.abort() : route.continue()));
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    // 「实时流断了不能静静不更新」是审计 R2-18 的既有能力，而这一句是全页**唯一**的告知
    const hint = page.locator('.typer .hint');
    await expect(hint).toBeVisible({ timeout: 30_000 });
    await expect(hint).toContainText('流断了');
    expect(
      await page.getByText('流断了：回话仍会以台账为准补上。').count(),
      '断线告知应当只此一处',
    ).toBe(1);

    // 放行 → 主动重连（与看板同一个口子）→ 告知消失、坞回到 88px
    blocked = false;
    await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
    await expect(hint).toHaveCount(0, { timeout: 30_000 });
    await expect
      .poll(async () => Math.abs((await page.locator('.typer').boundingBox())?.height ?? 0), {
        timeout: 30_000,
      })
      .toBeLessThanOrEqual(90);

    expectBundleHealthy(bundle);
  });

  test('展开一张急停：名牌整块在状态区里、不压到钉住带子底下（那 20px 是名牌的位子）', async ({
    page,
  }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.zone-status .turn.warn')).toHaveCount(1, { timeout: 60_000 });

    // 折叠态**不画名牌**（决策 218 修订 ⑦a：那张 20px 的净空是给悬出框沿 14px 的名牌留的，
    // 而摘要条上本就没有名牌——183 裁决③给它的三块里没有它）
    await expect(page.locator('.zone-status .turn.warn .dname')).toHaveCount(0);

    // 展开那一张：名牌回来、那 20px 也回来
    await page.locator('.zone-status .expander').click();
    const openCard = page.locator('.zone-status .turn.warn:not(.folded)');
    await expect(openCard).toHaveCount(1);
    const dname = openCard.locator('.dname');
    await expect(dname).toBeVisible();

    // **几何三条**（这条断言的存在理由：下一位做「空间清理」的人会把状态区的 padding-top
    // 当成浪费删掉，而名牌被盖住是几何问题、不会报错）：
    const zoneBox = await page.locator('.zone-status').boundingBox();
    const bandBox = await page.locator('.talk-head').boundingBox();
    const dnameBox = await dname.boundingBox();
    const top = dnameBox?.y ?? 0;
    const bottom = (dnameBox?.y ?? 0) + (dnameBox?.height ?? 0);
    expect(top, '名牌悬出了状态区上沿').toBeGreaterThanOrEqual((zoneBox?.y ?? 0) - 1);
    expect(bottom, '名牌悬出了状态区下沿').toBeLessThanOrEqual(
      (zoneBox?.y ?? 0) + (zoneBox?.height ?? 0) + 1,
    );
    const bandBottom = (bandBox?.y ?? 0) + (bandBox?.height ?? 0);
    const why =
      `名牌上沿 ${Math.round(top)} / 带子下沿 ${Math.round(bandBottom)}` +
      `（状态区 ${Math.round(zoneBox?.y ?? 0)}..${Math.round((zoneBox?.y ?? 0) + (zoneBox?.height ?? 0))}，` +
      `scrollTop=${await page.evaluate(() => document.scrollingElement?.scrollTop ?? 0)}）`;
    expect(top, `名牌被钉住的页头带子盖住了：${why}`).toBeGreaterThanOrEqual(bandBottom);
    expect(top - bandBottom, `名牌与钉住带子的下沿之间不足 4px：${why}`).toBeGreaterThanOrEqual(4);

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

    // 空态**不再自带**导航入口（决策 240）：原先的「去看板新建任务」是页面固定的导航钮
    // （纯前端路由、不进后端动作契约），看板收进顶栏那一枚页签之后它就没了；此刻全页
    // 没有任何后端下发的动作，也不该有任何前端自造的页面跳转。
    await expect(page.getByRole('link', { name: '去看板新建任务' })).toHaveCount(0);
    await expect(page.locator('.zone-status .turn button')).toHaveCount(0);
    // 看板入口**只**在顶栏那一行页签上
    await expect(
      page.getByRole('navigation', { name: '页面导航' }).getByRole('link', { name: '看板' }),
    ).toBeVisible();

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

    const row = page.locator('.talk-head .runrow');
    await expect(row).toBeVisible();

    // ① 首启一个班次都没有：chip 行只有「+ 新班次」那一颗，没有改名 / 归档（还没有当前班）
    const chips = row.locator('.runchip:not(.plus):not(.act)');
    await expect(chips).toHaveCount(0);
    await expect(row.locator('.runchip.plus')).toHaveText('+ 新班次');
    await expect(row.locator('.runchip.act')).toHaveCount(0);

    // ② 它**不在滚动容器里**（决策 218 Q15：原先长在 `.timeline` 里面，于是随对话上移）；
    // 挂页头右端、容器内横滚、不折行，页头高度不因它变（`--talk-chrome` 因此不必动）
    await expect(page.locator('.timeline .runrow'), '班次行不该还在时间线里').toHaveCount(0);
    await expect(row).toHaveCSS('flex-wrap', 'nowrap');
    await expect(row).toHaveCSS('overflow-x', 'auto');

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

  /**
   * 桌面那三条 `nowrap` / `overflow-x: auto` / `scrollWidth - clientWidth > 0` 断言**搬到了桌面档**
   * （上面那条的 ②），这一档换成三条等价事实：⋯ 可点、菜单开得出来、当前那一条被标出
   * （决策 218 Q10/Q12/Q14）。**它们说的是同一件事**：班次多到放不下时，它们仍然一条不少地
   * 够得着——只是从「一排芯片横滚」换成了「一份菜单」。
   */
  test('折行档：页头里没有 chip 行，班次都在 ⋯ 里（决策 218 Q10 / Q14）', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 430, height: 900 });

    // 造够班次：窄档那三四个就够（决策 218 实测里的那句「三四个班次铺成两三行」）
    for (const t of ['夜班甲', '夜班乙', '夜班丙', '夜班丁']) {
      await makeSession(app, t);
    }

    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    // ① 页面上没有 chip 行（它收进了 ⋯）
    await expect(page.locator('.runrow')).toHaveCount(0);
    // ② ⋯ 可点
    const more = page.locator('.talk-head .more');
    await expect(more).toBeVisible();
    await expect(more).toBeEnabled();
    // ③ 菜单开得出来，且**每一条班次都在**（条数与接口下发的一致——一条都没丢）、
    // 当前那条被标出。当前那一班在菜单里是身份行（不是按钮），故按钮数 = 总数 − 1。
    const list = (await (await fetch(`${app.apiBase}/foreman/sessions`)).json()) as {
      sessions: Array<{ id: string }>;
    };
    await more.click();
    const menu = page.locator('#talk-session-menu');
    await expect(menu).toBeVisible();
    await expect(menu.locator('.mi.now')).toHaveCount(1);
    await expect(menu.locator('button.mi:not(.plus):not(.act)')).toHaveCount(
      list.sessions.length - 1,
    );

    // **对话区仍是「整块屏幕减去三条钉住物」**（决策 192 的那条几何断言没有被班次列表吃掉）
    await page.keyboard.press('Escape');
    const timelineBox = await page.locator('.timeline').boundingBox();
    expect(timelineBox?.height ?? 0).toBeGreaterThanOrEqual(440);

    expectBundleHealthy(bundle);
  });

  /**
   * 回话中允许换班次（决策 220②）+ 两枚标记（决策 220③）。
   *
   * 那把 `sending || busy` 的锁撤掉之后，「那一轮回话去哪了」由标记接手：切走时原班次
   * 落下「有新动静」，点回去能看到**完整**那一轮（不是半截）。
   *
   * `page.route` 把这一次 POST 拖住 2.5s：切走的窗口因此是决定性的，而不是抢在几毫秒里。
   */
  test('回话中换班次：切走 → 回话落地 → 原班次带「有新动静」', async ({ page }) => {
    const bundle = watchBundle(page);
    await makeSession(app, '别处那一班');
    await page.route('**/foreman/messages', async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 2500));
      await route.continue();
    });
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    const more = page.locator('.talk-head .more');
    const menu = page.locator('#talk-session-menu');

    // 本机开一班新的（⋯ 的第一项），拿到一条干净的当前班次。
    // 按**名字**找它而不是 `/新班次/`：本组共用一个 app，上一群用例开出来的班次
    // 默认标题就叫「新班次 <时间>」，正则于是同时命中两枚按钮（实测 strict mode 报错）。
    await more.click();
    await menu.getByRole('button', { name: '+ 新班次' }).click();
    await expect(page.locator('.timeline .turn')).toHaveCount(0);

    // 说一句话（POST 被拖住）
    await page.locator('.typer textarea').fill('这一句会在别处落地');
    await page.locator('.typer button[type=submit]').click();

    // 发送中：⋯ **仍开得出来**（否则人读不到任何解释），当前那一条带「正在回话」
    await more.click();
    await expect(menu.locator('.mi.now .mi-mark.rep')).toHaveText('正在回话');

    // 切换**没有被禁用**（决策 220②）：切到「别处那一班」
    await menu.getByRole('button', { name: /别处那一班/ }).click();
    await expect(page.locator('.talk-head .sess-name')).toHaveText('别处那一班');
    // 切走之后这一屏是空对话——那一轮从视野里撤下，但回话照旧落台账（决策 220⑤）
    await expect(page.locator('.timeline .turn')).toHaveCount(0);

    // 回话落地 → 原班次出现「有新动静」（它更新的 last_active_at 晚于本机记的看过时刻）
    await more.click();
    const marked = menu.locator('button.mi').filter({ has: page.locator('.mi-mark') });
    await expect(marked).toHaveCount(1, { timeout: 30_000 });
    await expect(marked.locator('.mi-mark')).toHaveText('有新动静');

    // 点回去：那一轮是**完整**的（人那句 + 值班长的回话都从台账读回来），标记随之清零
    await marked.click();
    await expect(
      page.locator('.timeline .turn.mine', { hasText: '这一句会在别处落地' }),
    ).toHaveCount(1, { timeout: 30_000 });
    await expect(page.locator('.timeline .turn.fm')).not.toHaveCount(0);
    await more.click();
    await expect(menu.locator('button.mi').filter({ has: page.locator('.mi-mark') })).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  /**
   * 「正在回话」的第二支：SSE 里带**别的** `session_id` 的增量（决策 220③）。
   *
   * 这是本票的关键用例——它证明那个字段终于被用上了：`/foreman/stream` 把全部工头增量广播给
   * 所有订阅者，此前这个字段只被用来「丢掉不匹配的」，等于把「别的班次正在回话」白扔了。
   * 「另一台设备」在这里就是另一个上下文（直连接口说话），不必真开第二个浏览器。
   */
  test('远端开腔：别的班次带「正在回话」（跨设备那一组）', async ({ page }) => {
    const bundle = watchBundle(page);
    // 那台「别的设备」开的班。两条命名规则叠在一起，一步都不能省：
    //   ① `POST /foreman/sessions` 才**真的新开**一个班次——不带 `session_id` 说话是
    //      「落到最近活动的那个班次」（`resolve_session`），于是它会落在上一个用例留下的
    //      班次里（实测：菜单里根本没有「远端那一班」这一行）；
    //   ② 班次的名字取自**它第一句话**（决策 204②），空班次起好名再说话，名字会被那句话冲掉。
    // 故：先新开、再用**同一个名字**说第一句——名字立住，之后往里说话不会再改它。
    const remote = await makeSession(app, '远端那一班');
    await sayDirect(app, '远端那一班', remote);
    await page.setViewportSize({ width: 430, height: 900 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.talk')).toBeVisible({ timeout: 30_000 });
    // 先**离开**它：它刚建出来时是最活跃的那一条，故装载后当前班次正是它——
    // 那样一来它的增量带的就是 `session_id == currentId`，压根不是「别的班次」
    await page.locator('.talk-head .more').click();
    await page.locator('#talk-session-menu').getByRole('button', { name: '+ 新班次' }).click();
    await expect(page.locator('.talk-head .sess-name')).not.toHaveText('远端那一班');
    // 等那条 SSE 真的连上（增量会广播到所有订阅者，接不上就什么都收不到）
    await page.waitForTimeout(1500);

    // 以**那个班次**的名义说一句话：它的增量带着别的 `session_id` 到达本机的流
    await sayDirect(app, '远端的一句话', remote);

    await page.locator('.talk-head .more').click();
    const row = page.locator('#talk-session-menu button.mi', { hasText: '远端那一班' });
    await expect(row.locator('.mi-mark.rep')).toHaveText('正在回话', { timeout: 30_000 });
    // 标记只影响**读**、不影响点击：点它就是切过去（决策 220⑤，不拦）
    await row.click();
    await expect(page.locator('.talk-head .sess-name')).toHaveText('远端那一班');
    await expect(page.locator('.timeline .turn.mine', { hasText: '远端的一句话' })).toHaveCount(1);

    expectBundleHealthy(bundle);
  });
});

/**
 * 对讲台 · 折行档宽度扫描（决策 215 / 218）。
 *
 * 480–899 这一档此前**不存在**：`.talk` 只在 479 以下折，实测 480px 上对话列只剩 **82px**
 * （`minmax(0, 1fr)` 的 1fr 能缩到 0）。本叠按决策 215 折成一列，并按决策 218 把**控件形态**
 * 也一起换掉（否则这一档会先做出一个旧形态、再被改一次）。
 *
 * 三条一起看才成立：**不横向滚**、**控件形态与窄档同源**、**钉住关系按本档的顶栏算**
 * （这一档顶栏仍是桌面款 78–81px，不是窄档那两档 52 / 0——写死窄档的数会让摘要条钉偏）。
 */
test.describe('对讲台 · 折行档宽度扫描（决策 215 / 218）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({ script: archBlockerRounds(), title: 'E2E 折行档扫描' });
    await waitForTask(app, (t) => t.status === 'pending', 'pending', 60_000);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('480 / 600 / 768 / 899：单列、不横向滚、⋯ 在位、单张急停也折', async ({ page }) => {
    const bundle = watchBundle(page);
    for (const w of [480, 600, 768, 899]) {
      const why = `w=${w}`;
      await page.setViewportSize({ width: w, height: 800 });
      await page.goto(`${app.webBase}/#/talk`);
      await settleBundle(page, bundle);
      await expect(page.locator('.zone-status .turn.warn')).toHaveCount(1, { timeout: 60_000 });

      // 折成一列：右栏整块不渲染
      await expect(page.locator('.talk-side'), why).toBeHidden();
      // 控件形态与窄档同源：没有 chip 行、有 ⋯
      await expect(page.locator('.runrow'), why).toHaveCount(0);
      await expect(page.locator('.talk-head .more'), why).toBeVisible();
      // **单张急停也折**（决策 218 修订 ⑥：`forceFold` 的断点从 479 扩到 899）
      await expect(page.locator('.zone-status .turn.warn.folded'), why).toHaveCount(1);

      // 钉住关系：页头带子紧接着**本档**顶栏的下沿（这一档顶栏是桌面款 78px，
      // 不是窄档的 52 / 0——写死窄档的数会让摘要条钉偏）
      const hb = await page.locator('header.top').boundingBox();
      const head = await page.locator('.talk-head').boundingBox();
      expect(
        (head?.y ?? 0) - ((hb?.y ?? 0) + (hb?.height ?? 0)),
        `${why} 页头带子没紧接着顶栏`,
      ).toBeLessThanOrEqual(10);
      const zone = await page.locator('.zone-status').boundingBox();
      expect(
        Math.abs((zone?.y ?? 0) - ((head?.y ?? 0) + (head?.height ?? 0))),
        `${why} 摘要条没贴着带子的下沿`,
      ).toBeLessThanOrEqual(2);
      // 对话列 ≥420（决策 215 的 `minmax(420px, 1fr)` 下限；480px 上曾只剩 82px）
      const tl = await page.locator('.timeline').boundingBox();
      expect(tl?.width ?? 0, `${why} 对话列被挤窄了`).toBeGreaterThanOrEqual(420);
      // 坞贴底栏上沿（±2px：桌面那条底栏的盒高是 36px，而 `--sbar-h` 记的是 38px——既有偏差）
      const typerBox = await page.locator('.typer').boundingBox();
      const sbar = await page.locator('.statusline').boundingBox();
      expect(
        Math.abs((typerBox?.y ?? 0) + (typerBox?.height ?? 0) - (sbar?.y ?? 0)),
        `${why} 坞没贴在底栏上沿`,
      ).toBeLessThanOrEqual(2);
      // 静置态不空滚（与 ≤479 同一条算式：底盘底边距在这一档也按 `--sbar-h` 让位）
      expect(await verticalOverflow(page), `${why} 静置态空滚`).toBeLessThanOrEqual(1);
      // 整页不横向滚。480 上那 1px 是**顶栏过滤槽的既有溢出**（`#/` 与 `#/metrics` 上同样如此，
      // 与本叠无关）；600 以上要求严格 ≤0。
      expect(await horizontalOverflow(page), `${why} 整页横向溢出`).toBeLessThanOrEqual(
        w === 480 ? 1 : 0,
      );
      expectBundleHealthy(bundle);
    }
  });

  test('899 与 900 两侧：单张急停「折」与「不折」（两侧差异是刻意的）', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.setViewportSize({ width: 899, height: 800 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    await expect(page.locator('.zone-status .turn.warn.folded')).toHaveCount(1, {
      timeout: 60_000,
    });
    await expect(page.locator('.talk-side')).toBeHidden();

    await page.setViewportSize({ width: 900, height: 800 });
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    // 桌面口径逐像素不变：**单张展开**，右栏回来、⋯ 退场、班次行回到页头
    await expect(page.locator('.zone-status .turn.warn:not(.folded)')).toHaveCount(1, {
      timeout: 60_000,
    });
    await expect(page.locator('.zone-status .turn.warn')).toHaveCount(1);
    await expect(page.locator('.talk-side')).toBeVisible();
    await expect(page.locator('.talk-head .more')).toHaveCount(0);
    await expect(page.locator('.talk-head .runrow')).toHaveCount(1);
  });

  test('900 / 1099 / 1100：右栏 280 → 340（决策 215 的两档）', async ({ page }) => {
    const bundle = watchBundle(page);
    const cases: Array<[number, number]> = [
      [900, 280],
      [1099, 280],
      [1100, 340],
    ];
    for (const [w, right] of cases) {
      await page.setViewportSize({ width: w, height: 800 });
      await page.goto(`${app.webBase}/#/talk`);
      await settleBundle(page, bundle);
      const side = await page.locator('.talk-side').boundingBox();
      expect(Math.round(side?.width ?? 0), `w=${w} 右栏宽度`).toBe(right);
    }
  });
});

test.describe('对讲台 · 修复提议（票 12 / 决策 208）', () => {
  let app: App;

  test.beforeAll(async () => {
    // 空脚本起步：每一轮的脚本都按前一轮的产出现给（见 `setForemanRounds`）。
    app = await startApp({ script: foremanScript([]), title: 'E2E 修复' });
    // 档位配成 `auto`——`repair` 归**环境层**（决策 206 的 C 层 / 210③）：`ask` 下它只会
    // 变成一条提议，而这条用例要的是「worktree 真的拉起来了」。
    const res = await fetch(`${app.apiBase}/stage-configs/foreman`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
      body: JSON.stringify({ env_mode: 'auto' }),
    });
    expect(res.ok, `PUT /stage-configs/foreman -> ${res.status}`).toBeTruthy();
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('start → 改 → finish 真的走完，合入钮在默认视口下点得到', async ({ page }) => {
    const bundle = watchBundle(page);
    // **1280×720 就是这一档的口径**（playwright 默认视口，也是三分区最紧的一档）
    await page.setViewportSize({ width: 1280, height: 720 });

    // 项目 id 由后端生成 → 脚本只能事后注入（`setForemanRounds`）。
    const projectsRes = await fetch(`${app.apiBase}/projects`);
    expect(projectsRes.ok).toBeTruthy();
    const projects = (await projectsRes.json()) as { projects: Array<{ id: string }> };
    const projectId = projects.projects[0].id;

    // ① start：值班长拿到一个**它写得进去**的 worktree
    app.setForemanRounds([
      [
        tool('repair', { action: 'start', project_id: projectId }),
        text('拿到 worktree 了，去改。'),
      ],
    ]);
    await sayDirect(app, '把 fixture 里那个占位的 add 修一下');

    // worktree 落在**家目录下**（值班长的写域之内）——这是可达性的硬约束，不是审美
    const worktreesDir = join(app.homeDir, 'worktrees');
    let repairDir = '';
    await expect
      .poll(
        () => {
          repairDir = existsSync(worktreesDir)
            ? (readdirSync(worktreesDir).find((n) => n.startsWith('repair-')) ?? '')
            : '';
          return repairDir;
        },
        { timeout: 30_000, message: 'start 没有拉起修复 worktree' },
      )
      .not.toBe('');
    const repairId = repairDir.slice('repair-'.length);
    const worktree = join(worktreesDir, repairDir);

    // ② 改（写进 worktree）+ ③ finish（闸门 → commit → 提议）
    app.setForemanRounds([
      [
        tool('write_file', {
          path: join(worktree, 'src/lib.js'),
          content: 'function add(a, b) { return a + b; }\nmodule.exports = { add };\n',
        }),
        text('改好了。'),
      ],
      [
        tool('repair', {
          action: 'finish',
          project_id: projectId,
          repair_id: repairId,
          conclusion: '把 throw 的占位换成真的加法',
        }),
        text('闸门过了，等你按合入。'),
      ],
    ]);
    await sayDirect(app, '改吧');
    // finish 那一轮里闸门（`npm test --silent`）真的跑，且**这次它绿**——绿不了就出不了提议
    await sayDirect(app, '收口');

    // 主干**没被动过**：fixture 里那份仍是占位（合入永远人按，决策 210⑦）
    expect(
      readFileSync(join(app.repoDir, 'src/lib.js'), 'utf8'),
      '改动不许进主干——它只该在修复分支上',
    ).toContain('throw new Error');

    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);

    // 修复提议那一轮：闸门读数 + 可展开的补丁 + 「合入」而不是「执行」
    const prop = page.locator('.timeline .turn.prop').last();
    await expect(prop).toBeVisible({ timeout: 30_000 });
    await expect(prop.locator('.dname')).toHaveText('操作台');
    await expect(prop).toContainText('闸门');
    await expect(prop.getByRole('button', { name: '合入' })).toBeVisible();
    // 参数摘要那一段**不该出现**——修复提议给的是补丁，不是工具参数
    await expect(prop.locator('.pargs')).toHaveCount(0);
    // 补丁看得全、能复制（票 12）：展开之后 `src/lib.js` 的两个版本都在
    await prop.locator('summary', { hasText: '补丁' }).click();
    const diff = prop.locator(`[data-repair-diff]`);
    await expect(diff).toBeVisible();
    await expect(diff).toContainText('src/lib.js');
    await expect(diff).toContainText('+function add(a, b) { return a + b; }');

    // **够得到吗**（决策 208 的回归门）：修复那一块比普通提议高，而这一档不给它留高度
    await expectProposalReachable(page);

    expectBundleHealthy(bundle);
  });
});
