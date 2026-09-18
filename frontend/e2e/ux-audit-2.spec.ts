/**
 * UX 审计（第二轮）取证与**验证** spec（证据，非闸门）。
 *
 * 与第一轮 `ux-audit.spec.ts` 的分工：第一轮出的是「静息态画面」（每页一张 + 逐处特写）；
 * 这一轮针对的是第一轮**没覆盖的维度**——无障碍语义、键盘、失败态、窄档几何、横向溢出、
 * 双击重入——所以这里大部分用例不是截图，而是**量数**：读计算样式 / 布局盒 / DOM 语义，
 * 把「有没有问题」变成 stdout 上可引用的数字（截图只在需要肉眼确认的地方才出）。
 *
 * 默认 skip（`UX_AUDIT2` 未设时全跳），不进 `make check-e2e`。用法（仓库根）：
 *   bash scripts/e2e-artifacts.sh      # 先保证二进制内嵌的是当前工作树
 *   cd frontend && UX_AUDIT2=1 npx playwright test --project=chromium e2e/ux-audit-2.spec.ts
 *
 * 产物落 `.scratch/ux-audit-2/`（PNG 被 .gitignore 排除）。
 */

import { mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, type Page } from '@playwright/test';
import { startApp, waitForTaskById, watchBundle, type App } from './harness';
import { startGitRepo, type GitRepoFixture } from './gitRepo';
import {
  NODE,
  foremanScript,
  fullPassScript,
  readTask,
  siblingDesignRounds,
  siblingPassScript,
  text,
  tool,
} from './scripts';

test.skip(process.env.UX_AUDIT2 !== '1', '第二轮审计取证是证据不是门；设 UX_AUDIT2=1 才跑');

const here = dirname(fileURLToPath(import.meta.url));
const outDir = resolve(here, '..', '..', '.scratch', 'ux-audit-2');

/** 一个**停在 develop** 的任务：`sleep` 让它在取证期间一直是 running。 */
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
  console.log(`[r2] ${label} ${JSON.stringify(value)}`);
}

async function go(page: Page, width: number, height = 1000): Promise<void> {
  await page.setViewportSize({ width, height });
  await page.waitForTimeout(300);
}

/* ───────────────────────── ① 有数据：语义 / 键盘 / 失败态 / 几何 ───────────────────────── */

test.describe('① 有数据：无障碍语义、键盘、失败态、窄档几何', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: { ...fullPassScript('R2-MAIN'), ...foremanRounds },
      title: '实现用户登录接口',
      additionalTasks: [
        { title: '把登录失败原因写进审计日志', script: siblingPassScript('R2-EXTRA1') },
        { title: '给导出命令加一个 --since 参数', script: stalledScript },
      ],
    });
    // 等主任务挂到 merge_approval：档案盒（含移动款动作坞）只有 pending 才在
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

  /**
   * 语义与地标：`<main>`、`h1`、页签的 tablist、当前项 aria-current、动态错误的 live region、
   * 每个路由的 document.title。这一条不截图——它是 DOM 事实的枚举。
   */
  test('①.1 路由语义探针：main / h1 / tablist / aria-current / live region / title', async ({ page }) => {
    const guard = watchBundle(page);
    await go(page, 1440, 1100);
    const routes: Array<[string, string]> = [
      ['#/', '看板'],
      ['#/talk', '对讲台'],
      ['#/metrics', '指标'],
      ['#/settings', '设置落地页'],
      ['#/settings/projects', '设置·项目'],
      ['#/settings/providers', '设置·模型与密钥'],
      ['#/settings/stages', '设置·阶段配置'],
      ['#/settings/market', '设置·技能市场'],
      ['#/share', '手机访问'],
      ['#/nope', '404'],
      [`#/task/${app.taskIds[0]}`, '任务详情(pending)'],
    ];
    for (const [hash, name] of routes) {
      await open(page, app, hash);
      await setTheme(page, 'dark');
      const info = await page.evaluate(() => {
        const headings = Array.from(document.querySelectorAll('h1,h2,h3,h4,h5,h6')).map((h) => ({
          level: Number(h.tagName[1]),
          text: (h.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 36),
        }));
        const has = (sel: string) => document.querySelectorAll(sel).length;
        return {
          main: has('main'),
          header: has('header'),
          footer: has('footer'),
          h1: has('h1'),
          headings,
          tablist: has('[role=tablist]'),
          tabpanel: has('[role=tabpanel]'),
          ariaCurrent: has('[aria-current]'),
          ariaLive: has('[aria-live]'),
          roleAlert: has('[role=alert]'),
          navLabels: Array.from(document.querySelectorAll('nav[aria-label]')).map((n) =>
            n.getAttribute('aria-label'),
          ),
          title: document.title,
          hScroll: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
      log(`①.1 ${name}(${hash})`, info);
    }
    void guard;
  });

  /**
   * 待处理下拉：`role="menu"` 但没有键盘行为、也没有点外部关闭。
   * 断言形状：打开 → Escape → 点空白 → 是否仍开着；aria-expanded / aria-haspopup 是否有。
   */
  test('①.2 待处理下拉：role=menu 的键盘与关闭行为', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    const chip = page.locator('.pending-count');
    const menu = page.locator('[role=menu]');
    const before = await menu.count();
    await chip.click();
    await page.waitForTimeout(250);
    const opened = await menu.count();
    const attrs = await chip.evaluate((el) => ({
      expanded: el.getAttribute('aria-expanded'),
      haspopup: el.getAttribute('aria-haspopup'),
      controls: el.getAttribute('aria-controls'),
    }));
    await page.keyboard.press('Escape');
    await page.waitForTimeout(250);
    const afterEsc = await menu.count();
    // 点面板之外的空白处（看板主体）
    await page.mouse.click(20, 300);
    await page.waitForTimeout(250);
    const afterOutside = await menu.count();
    // 下拉项可否用方向键走到：焦点先落到触发钮，再按 ArrowDown
    await chip.focus();
    await page.keyboard.press('ArrowDown');
    await page.waitForTimeout(150);
    const focusedAfterArrow = await page.evaluate(
      () => `${document.activeElement?.tagName}.${(document.activeElement as HTMLElement | null)?.className ?? ''}`,
    );
    log('①.2 待处理下拉', { before, opened, afterEsc, afterOutside, attrs, focusedAfterArrow });
    await shot(page, 'r2-board-pending-dropdown-open');
    await page.keyboard.press('Escape');
  });

  /**
   * 详情页加载失败时是否**残留上一个任务**——在同一页里切到一个不存在的 id 最直接。
   */
  test('①.3 详情加载失败：是否残留上一个任务的内容', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, `#/task/${app.taskIds[0]}`);
    await page.waitForTimeout(1200);
    const beforeTitle = await page.locator('.d-title').first().innerText().catch(() => '');
    const beforeMain = await page.locator('.detail .main').innerText().catch(() => '');
    log('①.3 有效任务标题', { beforeTitle: beforeTitle.trim().slice(0, 40) });

    // 切到一个不存在的 id（同页 hash 变更，不重载 → store 是同一个）
    await open(page, app, '#/task/01JZZZZZZZZZZZZZZZZZZZZZZZ');
    await page.waitForTimeout(1200);
    const afterMain = await page.locator('.detail .main').innerText().catch(() => '');
    const banner = await page.locator('.banner.error').first().innerText().catch(() => '');
    log('①.3 失败后', {
      banner: banner.trim().slice(0, 80),
      // 旧任务标题是否仍在屏上
      stillShowsOldTitle: afterMain.includes(beforeTitle.trim()) && beforeTitle.trim().length > 0,
      bodyLen: afterMain.replace(/\s+/g, ' ').trim().length,
      hasActionButtons: await page.locator('.detail .main button').count(),
      // 加载/空分支是否可达
      showsRetry: await page.getByRole('button', { name: /重试|重新加载/ }).count(),
    });
    await shot(page, 'r2-detail-bogus-id');
  });

  /** 详情：宽度扫描——主栏 / 档案盒 / 页面横向溢出。 */
  test('①.4 详情宽度扫描：主栏被档案盒挤到多窄、页面是否横向滚', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, `#/task/${app.taskIds[0]}`);
    await setTheme(page, 'dark');
    for (const w of [1440, 1240, 1100, 1024, 900, 820, 768, 600, 520, 480, 430]) {
      await go(page, w, 1000);
      const m = await page.evaluate(() => {
        const split = document.querySelector('.detail');
        const main = document.querySelector('.detail .main') as HTMLElement | null;
        const dossier = document.querySelector('.dossier') as HTMLElement | null;
        const hero = document.querySelector('.rail.hero') as HTMLElement | null;
        return {
          cols: split ? getComputedStyle(split).gridTemplateColumns : null,
          main: main ? Math.round(main.getBoundingClientRect().width) : null,
          dossier: dossier ? Math.round(dossier.getBoundingClientRect().width) : null,
          heroScrollW: hero?.scrollWidth ?? null,
          heroClientW: hero?.clientWidth ?? null,
          planDocked: document.querySelector('.detail.docked') !== null,
          dockH: (document.querySelector('.dock') as HTMLElement | null)?.getBoundingClientRect().height ?? 0,
          pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
      log(`①.4 w=${w}`, m);
      if (w === 768) await shot(page, 'r2-detail-768');
      if (w === 520) await shot(page, 'r2-detail-520');
    }
  });

  /** 详情：sticky 档案盒滚到顶时，会不会被顶栏盖住「等你拍板」铭牌。 */
  test('①.5 档案盒 sticky 位置 vs 顶栏高度', async ({ page }) => {
    await go(page, 1280, 900);
    await open(page, app, `#/task/${app.taskIds[0]}`);
    await setTheme(page, 'dark');
    const topbarH = await page.evaluate(() => {
      const h = document.querySelector('header');
      return h ? Math.round(h.getBoundingClientRect().height) : null;
    });
    await page.evaluate(() => window.scrollTo(0, 500));
    await page.waitForTimeout(400);
    const m = await page.evaluate(() => {
      const d = document.querySelector('.dossier') as HTMLElement | null;
      const tag = document.querySelector('.dossier .dtag') as HTMLElement | null;
      const header = document.querySelector('header') as HTMLElement | null;
      return {
        dossierTop: d ? Math.round(d.getBoundingClientRect().top) : null,
        tagTop: tag ? Math.round(tag.getBoundingClientRect().top) : null,
        headerBottom: header ? Math.round(header.getBoundingClientRect().bottom) : null,
        stickyTopRule: d ? getComputedStyle(d).top : null,
      };
    });
    log('①.5 档案盒 sticky', { topbarH, ...m, 铭牌被盖: /* tagTop < headerBottom */ undefined });
    await shot(page, 'r2-detail-dossier-scrolled');
  });

  /** 移动款：底部动作坞与状态行是否重叠（同一 bottom:0 的两个 fixed 层）。 */
  test('①.6 移动款：动作坞 vs 状态行重叠', async ({ page }) => {
    await go(page, 430, 932);
    await open(page, app, `#/task/${app.taskIds[0]}`);
    await setTheme(page, 'dark');
    await page.waitForTimeout(500);
    const m = await page.evaluate(() => {
      const dock = document.querySelector('.dock') as HTMLElement | null;
      const bar = document.querySelector('footer.statusline') as HTMLElement | null;
      const r = (e: HTMLElement | null) => (e ? e.getBoundingClientRect() : null);
      const dr = r(dock);
      const br = r(bar);
      const overlap =
        dr && br ? Math.max(0, Math.min(dr.bottom, br.bottom) - Math.max(dr.top, br.top)) : 0;
      return {
        dock: dr ? { top: Math.round(dr.top), bottom: Math.round(dr.bottom), h: Math.round(dr.height), z: dock ? getComputedStyle(dock).zIndex : null } : null,
        status: br ? { top: Math.round(br.top), bottom: Math.round(br.bottom), h: Math.round(br.height), z: bar ? getComputedStyle(bar).zIndex : null } : null,
        overlapPx: Math.round(overlap),
        statusVisibleText: bar ? (bar.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 80) : null,
      };
    });
    log('①.6 移动款动作坞/状态行', m);
    await shot(page, 'r2-mobile-detail-dock-vs-status');

    // 坞上按钮是否被状态行/守护层挡住：用 elementFromPoint 打在坞按钮中心
    const hit = await page.evaluate(() => {
      const btn = document.querySelector('.dock button, .dock .btn') as HTMLElement | null;
      if (!btn) return null;
      const r = btn.getBoundingClientRect();
      const el = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return { btn: (btn.textContent ?? '').trim().slice(0, 20), hit: el?.className ?? el?.tagName ?? null };
    });
    log('①.6b 坞按钮命中测试', hit);
  });

  /** 对讲台：宽度扫描——对话列是否被右栏挤成一条。 */
  test('①.7 对讲台宽度扫描', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, '#/talk');
    await setTheme(page, 'dark');
    for (const w of [1440, 1240, 1024, 900, 820, 768, 600, 520, 480, 430]) {
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
      log(`①.7 w=${w}`, m);
      if (w === 768) await shot(page, 'r2-talk-768');
      if (w === 520) await shot(page, 'r2-talk-520');
    }
  });

  /** 状态行：宽度扫描——这一刻的时钟/主题钮是否被裁掉且无横滚。 */
  test('①.8 状态行裁剪扫描', async ({ page }) => {
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    for (const w of [1440, 1024, 900, 820, 768, 700, 600, 560, 520, 500, 480, 430]) {
      await go(page, w, 900);
      const m = await page.evaluate(() => {
        const bar = document.querySelector('footer.statusline') as HTMLElement | null;
        if (!bar) return null;
        const clock = bar.querySelector('.clock') as HTMLElement | null;
        const tog = bar.querySelector('.theme-tog') as HTMLElement | null;
        return {
          scrollW: bar.scrollWidth,
          clientW: bar.clientWidth,
          clipped: bar.scrollWidth - bar.clientWidth,
          clockRight: clock ? Math.round(clock.getBoundingClientRect().right) : null,
          viewport: window.innerWidth,
          togRight: tog ? Math.round(tog.getBoundingClientRect().right) : null,
        };
      });
      log(`①.8 w=${w}`, m);
    }
  });

  /** 字面 Markdown 星号：Talk 里那两处 `**…**` 在没有 Markdown 管道的模板里原样渲染。 */
  test('①.9 Talk 模板里的字面星号（源码事实 + 页面可见性）', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, '#/talk');
    const found = await page.evaluate(() => {
      const t = document.body.innerText;
      const hits = t.split('\n').filter((l) => l.includes('**')).map((l) => l.trim().slice(0, 90));
      return { asteriskLines: hits, count: hits.length };
    });
    log('①.9 Talk 字面星号', found);
  });
});

/* ───────────────────────── ② 动作失败：重试/归档是否静默 ───────────────────────── */

test.describe('② 动作失败无反馈（失败 provider 任务）', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: fullPassScript('R2-FAIL'),
      title: '注定失败的任务',
      badProvider: { status: 500, body: 'boom' },
    });
    // 等它落到终态（失败）
    await waitForTaskById(app.taskId, app, (t) => t.status === 'failed', 'failed', 120_000).catch(
      () => undefined,
    );
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('②.1 重试失败：是否有可见反馈', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, `#/task/${app.taskId}`);
    await setTheme(page, 'dark');
    const status = await page.locator('.dmeta').first().innerText().catch(() => '');
    const retryBtn = page.getByRole('button', { name: /重试/ });
    const count = await retryBtn.count();
    log('②.1 前置', { status: status.replace(/\s+/g, ' ').trim().slice(0, 60), retryButtons: count });
    if (!count) {
      log('②.1 跳过：页面上没有重试钮（任务未到终态）', {});
      return;
    }
    // 让这个请求必然失败
    await page.route('**/retry**', (route) =>
      route.fulfill({ status: 500, contentType: 'text/plain', body: 'injected failure' }),
    );
    await retryBtn.first().click();
    await page.waitForTimeout(2500);
    const after = await page.evaluate(() => ({
      bannerErrors: Array.from(document.querySelectorAll('.banner.error')).map((e) =>
        (e.textContent ?? '').trim().slice(0, 80),
      ),
      liveRegions: document.querySelectorAll('[aria-live],[role=alert]').length,
    }));
    log('②.1 点击后', after);
    await shot(page, 'r2-retry-failure');
  });
});

/* ───────────────────────── ③ 表单：不合法输入是否被拦 ───────────────────────── */

test.describe('③ 表单校验（只播 provider）', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({ script: foremanScript([[text('空班。')]]), providerOnly: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('③.1 provider：base_url 填一个非法值', async ({ page }) => {
    await go(page, 1440, 1100);
    await open(page, app, '#/settings/providers');
    await setTheme(page, 'dark');
    await page.getByRole('button', { name: /新增 provider/ }).click();
    await page.waitForTimeout(400);
    // ProviderForm 是**页内表单**（`form.prov-form`），不是模态框；输入框没有 name 属性
    const form = page.locator('form.prov-form');
    const inputs = form.locator('input');
    const fields = await inputs.evaluateAll((els) =>
      els.map((e) => ({
        type: (e as HTMLInputElement).type,
        placeholder: (e as HTMLInputElement).placeholder,
      })),
    );
    log('③.1 表单字段', fields);
    await shot(page, 'r2-provider-form');
    // 文档序：0 vendor / 1 model / 2 context_window / 3 base_url / 4 api_key / 5 enabled
    await inputs.nth(0).fill('openai');
    await inputs.nth(1).fill('gpt-4o-mini');
    await inputs.nth(2).fill('128000');
    await inputs.nth(3).fill('not a url');
    await inputs.nth(4).fill('sk-test-key');
    const submit = form.getByRole('button', { name: '创建' });
    log('③.1 提交钮', { count: await submit.count(), enabled: await submit.first().isEnabled() });
    await submit.first().click();
    await page.waitForTimeout(1500);
    const after = await page.evaluate(() => {
      const rows = Array.from(document.querySelectorAll('.reg-row, .row')).map((r) =>
        (r.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 110),
      );
      return {
        dialogStillOpen: document.querySelector('.dialog') !== null,
        fieldErrors: Array.from(document.querySelectorAll('.err, .error, [aria-invalid]')).map((e) =>
          (e.textContent ?? '').trim().slice(0, 80),
        ),
        rowsWithBadUrl: rows.filter((r) => r.includes('not a url')),
        rowCount: rows.length,
      };
    });
    log('③.1 提交后', after);
    await shot(page, 'r2-provider-bad-url-created');
  });
});

/* ───────────────────────── ④ 市场：刷新失败是否清空可见列表 ───────────────────────── */

test.describe('④ 技能市场：离线刷新', () => {
  let app: App;
  let remote: GitRepoFixture;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    remote = await startGitRepo({
      owner: 'acme',
      repo: 'skills',
      skills: [
        { dir: 'skills/grilling', description: '拷问设计树', body: ['拷问规则：事实自己查。'] },
        { dir: 'skills/tdd', description: '测试先行', body: ['红绿重构。'] },
      ],
    });
    app = await startApp({
      script: foremanScript([[text('空班。')]]),
      providerOnly: true,
      market: { owner: 'acme', repo: 'skills', gitBase: remote.base },
    });
  });

  test.afterAll(async () => {
    await app?.stop();
    await remote?.close();
  });

  test('④.1 列出技能后断网刷新：列表与控制是否消失', async ({ page, context }) => {
    await go(page, 1440, 1100);
    await open(page, app, '#/settings/market');
    await setTheme(page, 'dark');
    const view = page.getByRole('button', { name: /查看技能/ });
    if (!(await view.count())) {
      log('④.1 跳过：没有「查看技能」钮', {});
      return;
    }
    await view.first().click();
    await page.waitForTimeout(4500);
    const listed = await page.evaluate(() => ({
      body: document.body.innerText.replace(/\s+/g, ' ').trim().slice(0, 200),
      refreshBtns: Array.from(document.querySelectorAll('button')).filter((b) =>
        /刷新/.test(b.textContent ?? ''),
      ).length,
    }));
    log('④.1 列出后', listed);
    await shot(page, 'r2-market-listed');

    await context.setOffline(true);
    const refresh = page.getByRole('button', { name: /刷新/ });
    if (await refresh.count()) {
      await refresh.first().click();
      await page.waitForTimeout(4000);
    } else {
      log('④.1 没有刷新钮可点', {});
    }
    const after = await page.evaluate(() => ({
      body: document.body.innerText.replace(/\s+/g, ' ').trim().slice(0, 260),
      refreshBtns: Array.from(document.querySelectorAll('button')).filter((b) =>
        /刷新/.test(b.textContent ?? ''),
      ).length,
      viewBtns: Array.from(document.querySelectorAll('button')).filter((b) =>
        /查看技能/.test(b.textContent ?? ''),
      ).length,
      skillRows: document.querySelectorAll('.skill, .srow, .skill-row').length,
    }));
    log('④.1 断网刷新后', after);
    await shot(page, 'r2-market-refresh-offline');
    await context.setOffline(false);
  });
});
