/**
 * 前端 E2E：窄版面**不许横向溢出**（决策 341）+ 窄档**控件字号 ≥16px**（决策 399）。
 *
 * 钉的是 2026-09-30 的那条用户报障——「很多页面未对文字做换行导致整体被缩小」。
 * 「缩小」不是修辞：iOS 在 `width=device-width` 下遇到比视口宽的版面会把**布局视口**撑开
 * 再整体缩放显示，现场量到的是 `screen.width=390` 而 `window.innerWidth=1560`（对讲台，
 * 竖屏）与 `1930`（横屏）。所以这条用例断言两件事，缺一不可：
 *   ① 文档不横向溢出（`scrollWidth ≤ clientWidth`）——版面本身没被顶宽；
 *   ② 布局视口不比设备宽（`innerWidth ≤ screen.width`）——**没有发生收缩**。
 * 只断言①会漏掉「已经缩过、内容反而塞下了」那种退化。
 *
 * **内容必须是真的长 token**，否则这条用例恒绿（假绿比没有更坏）：值班长的回话里带着
 * 一段没有空格的 JSON，它就是报障现场那一段的等价物。用例先断言这段真的渲染出来了，
 * 再量几何。
 *
 * 2026-10-07 用户报障补了第二类「整页大小会变」——**点中输入框 iOS 会把整页放大**
 * （控件字号 <16px 即触发，且不自退）。同一份文件里另立两支（决策 399）：每条路由量
 * 所有表单控件的**计算字号**（作用域组件样式会改权重、只读源码量不出来），以及
 * 「产出文件」页签的**真 diff 产物**不顶宽（markdown 产物自带 `overflow-wrap: anywhere`，
 * 只在它上面量会假绿——顶宽的是 diff 的 `.dl { min-width: max-content }`）。
 *
 * 断言口径沿用本仓既有约定：只测外部行为（视口几何、渲染出来的计算字号），不测 CSS 源码
 * 措辞、不测 class 名。只 Chromium（决策 144）；后端与产物走 `harness`（回环绑定，故配对
 * 闸门不参与——闸门那半由 `crates/app/tests/integration/api_contract.rs` 钉）。
 */

import { expect, test, type Page } from '@playwright/test';
import {
  expectBundleHealthy,
  pendingTypeOf,
  settleBundle,
  startApp,
  waitForTask,
  watchBundle,
  type App,
} from './harness';
import {
  ArchitectExecute,
  NODE,
  foremanScript,
  fullPassScript,
  submit,
  text,
  writeFile,
  type NodeScript,
} from './scripts';

/** 报障现场那一段的等价物：**一个没有空格的长词**，markdown 正文里最常见的一类。 */
const LONG_TOKEN = `{"archived_at":null,"branch_name":null,"note":"${'A'.repeat(700)}"}`;
/** 断言「它真的渲染出来了」用的标记（缺了它，几何断言可能只是因为页面空着而绿）。 */
const MARK = '溢出闸门标记';

const REPLY = [MARK, '回执如下：', '', LONG_TOKEN].join('\n');

/** 一档视口下的四个读数。`layout` 就是「收缩有没有发生」的那个数。 */
async function viewport(page: Page) {
  return page.evaluate(() => ({
    screen: window.screen.width,
    layout: window.innerWidth,
    doc: document.documentElement.scrollWidth,
    client: document.documentElement.clientWidth,
  }));
}

test.describe('窄版面横向溢出（决策 341）', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({
      script: foremanScript([[text(REPLY)]]),
      providerOnly: true,
      title: '窄版面溢出',
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** 两档都量：竖屏是报障那台机器的常态，横屏是 `@media (max-width: 479px)` 那批兜底规则**失效**的那一档。 */
  for (const [width, height] of [
    [390, 844],
    [844, 390],
  ] as const) {
    test(`对讲台回话里的长 token 不顶穿版面（${width}×${height}）`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      const bundle = watchBundle(page);
      await page.goto(`${app.webBase}/#/talk`);
      await settleBundle(page, bundle);

      // 让值班长真的回一段话（脚本的第一轮）
      const input = page.locator('.typer textarea');
      await expect(input).toBeVisible();
      await input.fill('回一段带长 token 的回执');
      await input.press('Enter');

      const reply = page.locator('.timeline .turn.fm', { hasText: MARK }).first();
      await expect(reply, '回话没进时间线，后面的几何断言会因为页面空着而假绿').toBeVisible({
        timeout: 30_000,
      });
      // 长 token 真的落进了 markdown 正文（不是被截断掉、也不是进了 `pre` 那种有意的横滚容器）。
      // 断言挂在**回话那一轮**上：时间线上还有别的 `.md`（「脚本已结束」那条也走 MarkdownView），
      // 拿整条时间线去 `toContainText` 会撞 strict mode。
      const md = reply.locator('.md').first();
      await expect(md).toContainText(MARK);
      await expect(md).toContainText('A'.repeat(50));
      await page.waitForTimeout(500);

      const m = await viewport(page);
      expect(m.doc, `版面被顶宽了（${m.doc} > ${m.client}）`).toBeLessThanOrEqual(m.client + 1);
      expect(
        m.layout,
        `布局视口被撑到 ${m.layout}（设备只有 ${m.screen}）——iOS 会据此把整页缩小`,
      ).toBeLessThanOrEqual(m.screen + 1);

      expectBundleHealthy(bundle);
    });
  }

  /**
   * 全路由扫一遍：把这条闸门铺到**没被报障的那些页**上，将来谁往版面里塞一个长 token
   * 都会被拦下来。路由取自 `router` 的全表（任务详情走空态那条，本 fixture 没有任务）。
   */
  const ROUTES = [
    '#/',
    '#/metrics',
    '#/settings',
    '#/settings/compaction',
    '#/settings/foreman',
    '#/settings/market',
    '#/settings/notify',
    '#/settings/projects',
    '#/settings/providers',
    '#/settings/stages',
    '#/settings/tools',
    '#/share',
    '#/talk',
    '#/talk/watch',
    '#/task/nope',
  ];

  test('每条路由在 390px 下都不横向溢出、不触发收缩', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const offenders: string[] = [];
    for (const route of ROUTES) {
      const bundle = watchBundle(page);
      await page.goto(`${app.webBase}/${route}`);
      await settleBundle(page, bundle);
      await page.waitForTimeout(800);
      const m = await viewport(page);
      if (m.doc > m.client + 1 || m.layout > m.screen + 1) {
        offenders.push(`${route}: doc=${m.doc} client=${m.client} layout=${m.layout} screen=${m.screen}`);
      }
      expectBundleHealthy(bundle);
    }
    expect(offenders, `这些路由在 390px 下版面超宽：\n${offenders.join('\n')}`).toEqual([]);
  });

  /**
   * 焦点缩放闸门（决策 341 在**输入面**上的补齐）：iOS 只在控件字号 ≥16px 时才不放大，
   * 低一档点中输入的瞬间整页被 zoom 进去，且不会自己退回（= 用户说的「点了输入框整个
   * 页面就变了大小」）。这条钉的是**计算值**而不是 CSS 源码措辞：组件里的作用域样式会把
   * `select` 编译成 `select.svelte-*`、把 `.input` 加到两三个类，权重都高于 `app.css` 的
   * 元素 / 单类选择器——只看源码会漏，只有量渲染出来的字号才拦得住。
   *
   * 扫到的控件数为 0 时**必须失败**：那样这条闸门是假绿（一条控件都没测到却说自己过了）。
   */
  test('每条路由上表单控件字号 ≥16px（iOS 聚焦不放大）', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const offenders: string[] = [];
    let seen = 0;
    for (const route of ROUTES) {
      const bundle = watchBundle(page);
      await page.goto(`${app.webBase}/${route}`);
      await settleBundle(page, bundle);
      await page.waitForTimeout(600);
      const bad = await page.evaluate(() => {
        const out: string[] = [];
        document.querySelectorAll('input, textarea, select').forEach((el) => {
          const fs = Number.parseFloat(getComputedStyle(el).fontSize);
          if (!Number.isFinite(fs) || fs < 16) {
            const type = (el as HTMLInputElement).getAttribute('type') ?? '';
            out.push(`${el.tagName.toLowerCase()}${type ? `[${type}]` : ''}=${fs}px`);
          }
        });
        return out;
      });
      seen += await page.locator('input, textarea, select').count();
      if (bad.length) offenders.push(`${route}: ${bad.join(', ')}`);
      expectBundleHealthy(bundle);
    }
    expect(seen, '一条控件都没扫到——这条闸门会在假绿里过').toBeGreaterThan(0);
    expect(
      offenders,
      `这些控件的计算字号 <16px，iOS 点中会把整页放大：\n${offenders.join('\n')}`,
    ).toEqual([]);
  });
});

/**
 * 窄档**产物面板**（决策 341 在「产出文件」页签上的补齐，用户 2026-10-07 报障）。
 *
 * 现场：在 `#/task/<id>` 的「产出文件」页签里查看 `.diff` 产物时，面板按**内容**定宽
 * 而不是按栏宽——`FileViewer` 窄档是列向 flex，基类的 `align-items: flex-start` 让内容
 * 面板取自身 max-content，而 diff 的 `.dl { min-width: max-content }` 把最长行一路顶上来：
 * 390 视口下面板 430、文档 442，iOS 据此把整页缩小（与决策 341 同一条根因，只是那轮
 * `.md` 的 `overflow-wrap` 管不到这里）。markdown 产物顶不宽是因为 `.md` 自带
 * `overflow-wrap: anywhere`，所以**必须拿真 diff 立据**——只在 markdown 上量会假绿。
 */
test.describe('窄档产物面板（决策 341 补齐）', () => {
  let app: App;

  /** 全通过脚本 + 把 `design.md` 换成带长 token 的那一段：两条产物路径一次都覆盖到。 */
  function longArtifactScript(taskId: string): NodeScript {
    const base = fullPassScript(taskId);
    return {
      ...base,
      [NODE.archEx]: [
        [
          writeFile('design.md', `# 设计\n## 验收标准\n- AC-1 ${LONG_TOKEN}\n`),
          submit(
            ArchitectExecute({
              affectedFiles: ['src/lib.rs'],
              acceptanceCriteria: [{ id: 'AC-1', description: '能登录' }],
              designDocPath: 'design.md',
            }),
          ),
        ],
      ],
    };
  }

  test.beforeAll(async () => {
    app = await startApp({ script: longArtifactScript('窄档产物'), title: '窄档产物' });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** 两档都量：竖屏是报障那台机器的常态，横屏那一档 `@media (max-width: 479px)` 不生效。 */
  for (const [width, height] of [
    [390, 844],
    [844, 390],
  ] as const) {
    test(`产出文件里的长 token 与 diff 都不顶穿版面（${width}×${height}）`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      const bundle = watchBundle(page);
      await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');
      await page.goto(`${app.webBase}/#/task/${app.taskId}`);
      await settleBundle(page, bundle);

      await page.locator('.tabs button.tab', { hasText: '产出文件' }).click();

      // ① markdown 产物里的长 token（决策 341 修的那条路径，在产物面板里再钉一遍）
      await page.locator('.list button.cmd', { hasText: 'design.md' }).click();
      const md = page.locator('.content .md');
      await expect(md).toContainText('A'.repeat(50), { timeout: 30_000 });

      // ② 真 diff 产物——顶宽的那条。先断言它真的渲染出来了，否则几何断言会因为面板空着而假绿
      await page.locator('.list button.cmd', { hasText: 'merge-proposal.diff' }).click();
      const diffLines = page.locator('.content .dbody .dl');
      await expect(diffLines.first()).toBeVisible({ timeout: 30_000 });
      expect(await diffLines.count(), 'diff 没渲染出行来，几何断言会假绿').toBeGreaterThan(0);

      await page.waitForTimeout(500);
      const m = await viewport(page);
      expect(m.doc, `产物面板顶宽了版面（${m.doc} > ${m.client}）`).toBeLessThanOrEqual(m.client + 1);
      expect(
        m.layout,
        `布局视口被撑到 ${m.layout}（设备只有 ${m.screen}）——iOS 会据此把整页缩小`,
      ).toBeLessThanOrEqual(m.screen + 1);

      expectBundleHealthy(bundle);
    });
  }
});
