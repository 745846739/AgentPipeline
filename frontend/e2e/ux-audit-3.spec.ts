/**
 * UX 审计（第三轮）取证 spec——**复核 + 漂移面**（证据，非闸门）。
 *
 * 与前两轮的分工：
 *   · `ux-audit.spec.ts`（第一轮）管**静息态画面**：一页一张 + 逐处特写；
 *   · `ux-audit-2.spec.ts`（第二轮）管**边界态数字**：语义 / 键盘 / 失败态 / 窄档几何 / 重入；
 *   · 本轮（第三轮）只管**复核 + 漂移面**——把前两轮的 open 条目（票 18/19/21/22、票 17 子项）
 *     拿到**当前**源码与真页面上对一次账，再把第二轮收口（2026-09-18）之后的**代码漂移面**
 *     （设置三页 / 看板道具栏 / 决策 240 wordmark 去链 / 决策 243·300 窄档底栏与状态条 /
 *     决策 215·218 折行档）走一遍。**不重复前两轮已测的点**。
 *
 * 输出口径：每一行 `[r3] …` 都是报告里可逐条引用的**数字**（不写「应该怎样」）；
 * 截图落 `.scratch/ux-audit-3/*.png`（PNG 被 .gitignore 排除）。默认 skip，不进 `make check-e2e`。
 *
 * 用法（仓库根）：
 *   bash scripts/e2e-artifacts.sh      # 先保证二进制内嵌的是当前工作树
 *   cd frontend && UX_AUDIT3=1 npx playwright test --project=chromium e2e/ux-audit-3.spec.ts
 */

import { mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, type Page } from '@playwright/test';
import { startApp, waitForTaskById, watchBundle, expectBundleHealthy, type App } from './harness';
import {
  NODE,
  foremanScript,
  fullPassScript,
  readTask,
  siblingDesignRounds,
  text,
  tool,
} from './scripts';

test.skip(process.env.UX_AUDIT3 !== '1', '第三轮审计取证是证据不是门；设 UX_AUDIT3=1 才跑');

const here = dirname(fileURLToPath(import.meta.url));
const outDir = resolve(here, '..', '..', '.scratch', 'ux-audit-3');

/** 一个**停在 develop** 的任务：`sleep` 让它在取证期间一直是 running（折行档量得到真内容）。 */
const stalledScript = {
  ...siblingDesignRounds(),
  [NODE.developEx]: [[tool('run_command', { command: 'sleep 900' })]],
};

/** 值班长：先翻一次台账，再给一句结论（够撑起会话面）。 */
const foremanRounds = foremanScript([
  [readTask('01JZZZZZZZZZZZZZZZZZZZZZZZ'), text('当前态势')],
  [text('两件货箱卡在合入前等拍板，一个工位还在跑。')],
]);

async function shot(page: Page, name: string): Promise<void> {
  await page.screenshot({ path: resolve(outDir, `${name}.png`), fullPage: true });
}

async function setTheme(page: Page, theme: 'dark' | 'light'): Promise<void> {
  await page.evaluate((t) => {
    document.documentElement.dataset.theme = t;
    localStorage.setItem('agentpipeline.theme', t);
  }, theme);
  await page.waitForTimeout(250);
}

/** 进页面并等产物执行完（不能用 networkidle：SSE 常驻）。 */
async function open(page: Page, app: App, hash: string): Promise<void> {
  await page.goto(`${app.webBase}/${hash}`);
  await page.waitForLoadState('load');
  await page.waitForTimeout(700);
}

/** 一行 stdout 就是一条证据：把量到的数字打出来，报告里逐条引。 */
function log(label: string, value: unknown): void {
  console.log(`[r3] ${label} ${JSON.stringify(value)}`);
}

async function go(page: Page, width: number, height = 1000): Promise<void> {
  await page.setViewportSize({ width, height });
  await page.waitForTimeout(300);
}

/* ─────────── ① 复核：折行档（ux-audit-2 票 18 详情 / 票 19 对讲台） ─────────── */

test.describe('① 复核：中间档折行（票 18 详情 / 票 19 对讲台）', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: { ...fullPassScript('R3-MAIN'), ...foremanRounds },
      title: '实现用户登录接口',
      additionalTasks: [
        { title: '把登录失败原因写进审计日志', script: { ...siblingDesignRounds() } },
        { title: '给导出命令加一个 --since 参数', script: stalledScript },
      ],
    });
    // 主任务挂到 merge_approval：档案盒（含动作坞）只有 pending 才在——票 21 复核要用
    await waitForTaskById(
      app.taskIds[0],
      app,
      (t) => (t.pending_reason as { type?: string } | null)?.type === 'merge_approval',
      'merge_approval',
      180_000,
    ).catch(() => undefined);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** 详情页宽度扫描：主栏 / 页面横向溢出 / hero 轨道（票 18 的 5a/5b）。 */
  test('①.1 详情宽度扫描（票 18）', async ({ page }) => {
    const guard = watchBundle(page);
    await go(page, 1440, 1100);
    await open(page, app, `#/task/${app.taskIds[0]}`);
    await setTheme(page, 'dark');
    for (const w of [1100, 900, 820, 768, 600, 520, 480]) {
      await go(page, w, 1000);
      const m = await page.evaluate(() => {
        const split = document.querySelector('.detail') as HTMLElement | null;
        const main = document.querySelector('.detail .main') as HTMLElement | null;
        const hero = document.querySelector('.rail.hero') as HTMLElement | null;
        return {
          cols: split ? getComputedStyle(split).gridTemplateColumns : null,
          main: main ? Math.round(main.getBoundingClientRect().width) : null,
          heroScrollW: hero?.scrollWidth ?? null,
          heroClientW: hero?.clientWidth ?? null,
          heroOverflowX: hero ? getComputedStyle(hero).overflowX : null,
          pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
      log(`①.1 详情 w=${w}`, m);
      if (w === 768) await shot(page, 'r3-detail-768');
      if (w === 480) await shot(page, 'r3-detail-480');
    }
    expectBundleHealthy(guard);
  });

  /** 对讲台宽度扫描：对话列宽 / 页面横向溢出（票 19 的 5c）。 */
  test('①.2 对讲台宽度扫描（票 19）', async ({ page }) => {
    const guard = watchBundle(page);
    await go(page, 1440, 1100);
    await open(page, app, '#/talk');
    await setTheme(page, 'dark');
    for (const w of [1100, 900, 820, 768, 600, 520, 480]) {
      await go(page, w, 1000);
      const m = await page.evaluate(() => {
        const talk = document.querySelector('.talk') as HTMLElement | null;
        const side = document.querySelector('.talk-side') as HTMLElement | null;
        return {
          cols: talk ? getComputedStyle(talk).gridTemplateColumns : null,
          sideW: side ? Math.round(side.getBoundingClientRect().width) : null,
          pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
      log(`①.2 对讲台 w=${w}`, m);
      if (w === 768) await shot(page, 'r3-talk-768');
      if (w === 480) await shot(page, 'r3-talk-480');
    }
    expectBundleHealthy(guard);
  });
});

/* ─────────── ② 复核：破坏性动作的确认步（ux-audit-2 票 21） ─────────── */

test.describe('② 复核：破坏性动作确认步（票 21）', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: { ...fullPassScript('R3-CONFIRM'), ...foremanRounds },
      title: '实现用户登录接口',
    });
    await waitForTaskById(
      app.taskId,
      app,
      (t) => (t.pending_reason as { type?: string } | null)?.type === 'merge_approval',
      'merge_approval',
      180_000,
    ).catch(() => undefined);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /**
   * 点「合入」（第一档）第一次：是否出现确认步？按钮是实心还是描边？
   * 契约（票 21 判据）：第一次点**不应**发请求、应出现「确认…？」+ 同一颗钮 + 取消。
   */
  test('②.1 「合入」第一次点击是否进确认步', async ({ page }) => {
    const guard = watchBundle(page);
    await go(page, 1440, 1100);
    await open(page, app, `#/task/${app.taskId}`);
    await setTheme(page, 'dark');
    const before = await page.evaluate(() => ({
      bodyHasConfirm: /\?\s*$|确认/.test(document.body.innerText),
      buttons: Array.from(document.querySelectorAll('.actions button, .dock button')).map((b) => ({
        text: (b.textContent ?? '').trim().slice(0, 24),
        cls: b.className,
      })),
    }));
    log('②.1 点击前', before);
    const merge = page.getByRole('button', { name: /^合入$/ }).first();
    const count = await merge.count();
    log('②.1 「合入」钮数量', { count });
    if (count) {
      await merge.click();
      await page.waitForTimeout(900);
      const after = await page.evaluate(() => ({
        bodyHasConfirm: /确认|真的|确定/.test(document.body.innerText),
        confirmTexts: Array.from(document.querySelectorAll('*'))
          .map((e) => (e.childElementCount === 0 ? (e.textContent ?? '').trim() : ''))
          .filter((t) => /确认|真的/.test(t))
          .slice(0, 6),
        buttons: Array.from(document.querySelectorAll('.actions button, .dock button')).map((b) => ({
          text: (b.textContent ?? '').trim().slice(0, 24),
          cls: b.className,
        })),
      }));
      log('②.1 第一次点击后', after);
      await shot(page, 'r3-merge-first-click');
    }
    expectBundleHealthy(guard);
  });

  /** 「终止任务」（destructive）按钮的量级：应是红描边（--stop），不是弱化 quiet。 */
  test('②.2 destructive 动作的按钮量级', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, `#/task/${app.taskId}`);
    await setTheme(page, 'dark');
    const m = await page.evaluate(() => {
      const btn = Array.from(document.querySelectorAll('button')).find((b) =>
        /终止任务|取消任务/.test(b.textContent ?? ''),
      ) as HTMLElement | undefined;
      if (!btn) return { found: false };
      const cs = getComputedStyle(btn);
      return {
        found: true,
        text: (btn.textContent ?? '').trim().slice(0, 20),
        cls: btn.className,
        borderColor: cs.borderColor,
        borderWidth: cs.borderWidth,
        background: cs.backgroundColor,
      };
    });
    log('②.2 终止任务按钮量级', m);
  });
});

/* ─────────── ③ 复核：中流状态持久化（ux-audit-2 票 22 余三件） ─────────── */

test.describe('③ 复核：中流状态持久化（票 22 余三件）', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: { ...fullPassScript('R3-PERSIST'), ...foremanRounds },
      title: '实现用户登录接口',
    });
    await waitForTaskById(
      app.taskId,
      app,
      (t) => (t.pending_reason as { type?: string } | null)?.type === 'merge_approval',
      'merge_approval',
      180_000,
    ).catch(() => undefined);
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** ① 详情页签 ?tab=：切到 Diff → 刷新后是否还在 Diff（票 22 余项一）。 */
  test('③.1 详情页签刷新留存', async ({ page }) => {
    const guard = watchBundle(page);
    await go(page, 1440, 1100);
    await open(page, app, `#/task/${app.taskId}`);
    await setTheme(page, 'dark');
    const diffTab = page.getByRole('tab', { name: /^Diff/ }).first();
    if (await diffTab.count()) {
      await diffTab.click();
      await page.waitForTimeout(500);
    }
    const selectedBefore = await page
      .evaluate(() => document.querySelector('[role=tab][aria-selected=true]')?.textContent?.trim() ?? null);
    const hashBefore = await page.evaluate(() => location.hash);
    await page.reload();
    await page.waitForLoadState('load');
    await page.waitForTimeout(1200);
    const selectedAfter = await page.evaluate(
      () => document.querySelector('[role=tab][aria-selected=true]')?.textContent?.trim() ?? null,
    );
    const hashAfter = await page.evaluate(() => location.hash);
    log('③.1 详情页签刷新', { selectedBefore, hashBefore, selectedAfter, hashAfter });
    await shot(page, 'r3-tab-after-reload');
    expectBundleHealthy(guard);
  });

  /** ② 看板过滤 ?filter=：设过滤 → 刷新后是否还是该过滤（票 22 余项二）。 */
  test('③.2 看板过滤刷新留存', async ({ page }) => {
    const guard = watchBundle(page);
    await go(page, 1440, 1100);
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    const doneSlot = page.locator('.slots button').filter({ hasText: '已完成' }).first();
    if (await doneSlot.count()) {
      await doneSlot.click();
      await page.waitForTimeout(400);
    }
    const before = await page.evaluate(() => ({
      pressed: Array.from(document.querySelectorAll('.slots button[aria-pressed=true]')).map((b) =>
        (b.textContent ?? '').trim().slice(0, 12),
      ),
      hash: location.hash,
      ls: Object.keys(localStorage).filter((k) => k.startsWith('agentpipeline.')),
    }));
    await page.reload();
    await page.waitForLoadState('load');
    await page.waitForTimeout(1500);
    const after = await page.evaluate(() => ({
      pressed: Array.from(document.querySelectorAll('.slots button[aria-pressed=true]')).map((b) =>
        (b.textContent ?? '').trim().slice(0, 12),
      ),
      hash: location.hash,
      ls: Object.keys(localStorage).filter((k) => k.startsWith('agentpipeline.')),
    }));
    log('③.2 看板过滤刷新', { before, after });
    await shot(page, 'r3-board-filter-after-reload');
    expectBundleHealthy(guard);
  });

  /** ③ 对讲台输入草稿 talk_draft：输入半句 → 刷新 → 是否还在（票 22 余项三）。 */
  test('③.3 对讲台草稿刷新留存', async ({ page }) => {
    const guard = watchBundle(page);
    await go(page, 1440, 1100);
    await open(page, app, '#/talk');
    await setTheme(page, 'dark');
    const box = page.locator('textarea').first();
    const count = await box.count();
    const probe = 'r3-半句草稿';
    if (count) {
      await box.fill(probe);
      await page.waitForTimeout(600);
    }
    const before = await page.evaluate(() => ({
      ls: Object.keys(localStorage).filter((k) => k.startsWith('agentpipeline.')),
      draftKey: localStorage.getItem('agentpipeline.talk_draft'),
    }));
    await page.reload();
    await page.waitForLoadState('load');
    await page.waitForTimeout(1500);
    const afterValue = (await page.locator('textarea').first().inputValue().catch(() => '')) ?? '';
    const after = await page.evaluate(() => ({
      ls: Object.keys(localStorage).filter((k) => k.startsWith('agentpipeline.')),
      draftKey: localStorage.getItem('agentpipeline.talk_draft'),
    }));
    log('③.3 对讲台草稿刷新', {
      tabInputs: count,
      before,
      afterValue,
      survived: afterValue.includes(probe),
      after,
    });
    await shot(page, 'r3-talk-draft-after-reload');
    expectBundleHealthy(guard);
  });
});

/* ─────────── ④ 漂移面（第二轮收口之后的界面） ─────────── */

test.describe('④ 漂移面：设置三页 / 看板道具栏 / wordmark / 窄档底栏', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: { ...fullPassScript('R3-DRIFT'), ...foremanRounds },
      title: '实现用户登录接口',
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** ④.1 设置三页可达性 + 语义探针（main / h1 / 标题层级）。 */
  test('④.1 设置三页语义探针', async ({ page }) => {
    const guard = watchBundle(page);
    await go(page, 1440, 1100);
    for (const [hash, name] of [
      ['#/settings/tools', '设置·命令执行'],
      ['#/settings/notify', '设置·离线通知'],
      ['#/settings/foreman', '设置·值守轮'],
    ] as Array<[string, string]>) {
      await open(page, app, hash);
      await setTheme(page, 'dark');
      const info = await page.evaluate(() => ({
        main: document.querySelectorAll('main').length,
        headers: Array.from(document.querySelectorAll('h1,h2,h3')).map((h) => ({
          level: Number(h.tagName[1]),
          text: (h.textContent ?? '').trim().slice(0, 30),
        })),
        tablist: document.querySelectorAll('[role=tablist]').length,
        roleAlert: document.querySelectorAll('[role=alert]').length,
        title: document.title,
        hScroll: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      }));
      log(`④.1 ${name}(${hash})`, info);
    }
    expectBundleHealthy(guard);
  });

  /** ④.2 离线通知页：礼貌开关的可访问语义（role=radio / aria-checked / 开关钮）。 */
  test('④.2 离线通知页礼貌开关语义', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, '#/settings/notify');
    await setTheme(page, 'dark');
    const m = await page.evaluate(() => ({
      radios: Array.from(document.querySelectorAll('[role=radio]')).map((r) => ({
        label: (r.textContent ?? '').trim().slice(0, 12),
        checked: r.getAttribute('aria-checked'),
      })),
      radiogroup: document.querySelectorAll('[role=radiogroup]').length,
      pressed: Array.from(document.querySelectorAll('[aria-pressed]')).map((b) => ({
        text: (b.textContent ?? '').trim().slice(0, 16),
        pressed: b.getAttribute('aria-pressed'),
      })),
      switchLike: document.querySelectorAll('[role=switch],[role=checkbox]').length,
      fields: Array.from(document.querySelectorAll('label.field')).map((l) =>
        (l.querySelector('.lab,.field-label')?.textContent ?? '').trim().slice(0, 20),
      ),
    }));
    log('④.2 离线通知页开关语义', m);
    await shot(page, 'r3-settings-notify');
  });

  /** ④.3 看板顶部道具栏：过滤槽的 aria-pressed / 计数徽章 / 第 3 槽不画数 / 「待处理 N」。 */
  test('④.3 看板道具栏', async ({ page }) => {
    await go(page, 1200, 1100);
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    const m = await page.evaluate(() => {
      const slots = Array.from(document.querySelectorAll('.slots button'));
      return {
        slotCount: slots.length,
        slots: slots.map((b) => ({
          text: (b.textContent ?? '').trim().slice(0, 10),
          pressed: b.getAttribute('aria-pressed'),
          hasCount: found(b, '.cb'),
        })),
        pendingChip: (document.querySelector('.chip.pending-count')?.textContent ?? '').trim(),
      };
      function found(b: Element, sel: string) {
        const e = b.querySelector(sel);
        return e ? (e.textContent ?? '').trim() : null;
      }
    });
    log('④.3 看板道具栏', m);
    await shot(page, 'r3-board-props-bar');
  });

  /** ④.4 wordmark 去链（决策 240）：铭牌是 `<span>` 不是 `<a href="#/">`。 */
  test('④.4 wordmark 是否仍是链接', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    const m = await page.evaluate(() => {
      const wm = document.querySelector('.wordmark');
      return {
        tag: wm?.tagName ?? null,
        isLink: wm?.tagName === 'A',
        href: (wm as HTMLAnchorElement | null)?.getAttribute?.('href') ?? null,
        text: (wm?.textContent ?? '').trim(),
        navItems: Array.from(document.querySelectorAll('.navbar a, nav.pages a, .pages a')).map((a) =>
          (a.textContent ?? '').trim().slice(0, 8),
        ),
      };
    });
    log('④.4 wordmark', m);
  });

  /** ④.5 窄档（430）底部页签栏与状态条：状态条退场、底栏在场（决策 243/300）。 */
  test('④.5 窄档底栏与状态条', async ({ page }) => {
    await go(page, 430, 932);
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    await page.waitForTimeout(500);
    const m = await page.evaluate(() => {
      const bar = document.querySelector('footer.statusline') as HTMLElement | null;
      const nav = document.querySelector('nav.navbar, nav.pages, .navbar') as HTMLElement | null;
      return {
        statusline: bar ? { display: getComputedStyle(bar).display, h: Math.round(bar.getBoundingClientRect().height) } : null,
        bottomNav: nav
          ? { display: getComputedStyle(nav).display, h: Math.round(nav.getBoundingClientRect().height), label: nav.getAttribute('aria-label') }
          : null,
        topbarH: getComputedStyle(document.documentElement).getPropertyValue('--topbar-h').trim(),
        sbarH: getComputedStyle(document.documentElement).getPropertyValue('--sbar-h').trim(),
      };
    });
    log('④.5 窄档底栏/状态条', m);
    await shot(page, 'r3-narrow-bottom-430');
  });
});

/* ─────────── ⑤ 复核：toast 键盘关闭路径（票 17 子项）+ 字面星号回归 ─────────── */

test.describe('⑤ 复核：toast 键盘关闭与字面星号', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: { ...fullPassScript('R3-TOAST'), ...foremanRounds },
      title: '实现用户登录接口',
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** toast：是否有关闭钮？是否有键盘关闭路径（Escape）？（票 17 未做子项） */
  test('⑤.1 toast 键盘关闭路径', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    // 触发一条 toast：用终态任务失败最容易，但这里退而求其次——读现有 toast 的存在与钮
    await page.waitForTimeout(2500);
    const m = await page.evaluate(() => {
      const toasts = Array.from(document.querySelectorAll('.toast'));
      const first = toasts[0] as HTMLElement | undefined;
      return {
        count: toasts.length,
        firstRole: first?.getAttribute('role') ?? null,
        closeBtns: toasts.reduce(
          (n, t) => n + t.querySelectorAll('button.close').length,
          0,
        ),
        closeLabel: toasts[0]?.querySelector('button.close')?.getAttribute('aria-label') ?? null,
      };
    });
    log('⑤.1 toast 结构', m);
    // Escape 是否能关掉（若有 toast）
    if (m.count > 0) {
      await page.locator('.toast').first().focus();
      await page.keyboard.press('Escape');
      await page.waitForTimeout(500);
      const afterEsc = await page.locator('.toast').count();
      log('⑤.1 Escape 后 toast 数', { before: m.count, afterEsc });
    }
  });

  /** 字面星号回归：模板正文里 `**…**` 原样渲染（第一轮已记，复核是否仍在）。 */
  test('⑤.2 模板里的字面星号', async ({ page }) => {
    await go(page, 1440, 1100);
    for (const [hash, name] of [
      ['#/settings/notify', '离线通知'],
      ['#/settings/tools', '命令执行'],
    ] as Array<[string, string]>) {
      await open(page, app, hash);
      await setTheme(page, 'dark');
      const found = await page.evaluate(() => {
        const t = document.body.innerText;
        // 字面 Markdown 粗体 `**x**` 才算缺陷；`***`（通知 token 的掩码）是有意设计，
        // `\*\*[^*]+\*\*` 对它不命中（评审订正：初稿 includes('**') 把掩码误报成缺陷）
        const mdBold = /\*\*[^*]+\*\*/;
        const hits = t
          .split('\n')
          .filter((l) => mdBold.test(l))
          .map((l) => l.trim().slice(0, 80));
        return { count: hits.length, hits };
      });
      log(`⑤.2 ${name} 字面星号`, found);
    }
    await shot(page, 'r3-literal-asterisks');
  });
});
