/**
 * 前端 E2E：设置各页的空态与文案纪律（UX 审计票 04 / 07 / 09 / 12 / 13 / 23）。
 *
 * 断言口径（整备规格 Testing Decisions）：只测**用户看得见的东西**——
 *   · 四个页面的空态都有「状态 → 下一步」，文字落在可读的灰字档（计算样式 + 对比度）；
 *   · 空态提到的另一个页面真的可点，点了能到；
 *   · 删除被拒的理由出现在**动手之后**（动手之前页面上没有那条报错），说的是下一步；
 *   · 破坏性动作与中性动作同一档量级（描边 / 硬投影 / 尺寸，计算样式）；
 *   · 项目页据 `#/settings/projects?project=<id>&analyze=1` 自动就位并触发分析，读不到 query 时不变；
 *   · 面向用户的正文里不再出现内部决策编号（兜底机器门在 `src/lib/copy-discipline.test.ts`，
 *     这里在真应用上再核一遍**屏幕上真的读得到什么**）。
 *
 * 文案不做精确匹配（只对结构性片段做包含 / 正则判断）——只有 `title` 这类契约性字符串例外。
 * 真 axum 后端 + 内嵌产物 + mock LLM；只 Chromium（决策 144）。
 */

import { expect, test, type Locator, type Page } from '@playwright/test';

import {
  expectBundleHealthy,
  settleBundle,
  startApp,
  watchBundle,
  type App,
} from './harness';
import { foremanScript, fullPassScript, submit } from './scripts';

/* ── 计算样式与对比度（照 pixel-theme.spec 的口径：读浏览器**算出**的值） ── */

/** 根元素上的 token 取值。 */
async function rootToken(page: Page, name: string): Promise<string> {
  return page.evaluate(
    (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim().toLowerCase(),
    name,
  );
}

/** `#rgb` / `#rrggbb` / `rgb()` / `rgba()` → `[r, g, b]`；其它形态直接报错，不猜。 */
function toRgb(value: string): [number, number, number] {
  const v = value.trim().toLowerCase();
  if (v.startsWith('#')) {
    const h = v.slice(1);
    const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h.slice(0, 6);
    const n = Number.parseInt(full, 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  const m = /rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)/.exec(v);
  if (!m) throw new Error(`读不出颜色：${value}`);
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

/** 与浏览器归一化后的计算值同形（`rgb(r, g, b)`），便于直接比较。 */
function rgbString(value: string): string {
  const [r, g, b] = toRgb(value);
  return `rgb(${r}, ${g}, ${b})`;
}

function relativeLuminance([r, g, b]: [number, number, number]): number {
  const f = (c: number) => {
    const x = c / 255;
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}

/** WCAG 相对对比度。 */
function contrastRatio(a: string, b: string): number {
  const [hi, lo] = [relativeLuminance(toRgb(a)), relativeLuminance(toRgb(b))].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** 元素的计算文字色。 */
function colorOf(locator: Locator): Promise<string> {
  return locator.evaluate((el) => getComputedStyle(el).color);
}

/** 元素背后**真正**的承载底色（自下而上找第一个不透明底——透明元素自己的背景读不出东西）。 */
function backingColor(locator: Locator): Promise<string> {
  return locator.evaluate((el) => {
    let node: HTMLElement | null = el as HTMLElement;
    while (node) {
      const c = getComputedStyle(node).backgroundColor;
      if (c && c !== 'transparent' && !/rgba\(0,\s*0,\s*0,\s*0\)/.test(c)) return c;
      node = node.parentElement;
    }
    return getComputedStyle(document.body).backgroundColor;
  });
}

/**
 * 打开一个 hash 路由并等它真的画出来。
 *
 * 全是同文档的片段导航（hash 路由），故 `load` 事件不会再来一次——用页面标题当锚，
 * 免得断言落在上一页还没换掉的 DOM 上。
 */
async function open(page: Page, app: App, hash: string, heading: string): Promise<void> {
  const bundle = watchBundle(page);
  await page.goto(`${app.webBase}/${hash}`);
  await settleBundle(page, bundle);
  await expect(page.locator('.page h1.p-title')).toContainText(heading);
  expectBundleHealthy(bundle);
}

/** 空态的形状来自共享组件 `components/ui/EmptyState.svelte`。这里**只按用户看得见的文字**
 *  定位，不按它的 class——spec 的 Testing Decisions 明说「断言落在用户看到什么与计算样式上，
 *  不落在 class 名、组件内部状态或文件结构上」。`getByText` 认的是最小的那个含文本的元素
 *  （即那个 `<p>`），与按 class 取到的是同一个节点。 */
const atText = (page: Page, pattern: RegExp) => page.getByText(pattern).first();

/* ───────────────────────── ① 空 home：四个页面的空态 ───────────────────────── */

test.describe('前端 E2E：设置各页的空态（票 13 / 12 / 09 / 23）', () => {
  let app: App;

  test.beforeAll(async () => {
    // 首启（空 home）：项目 / 模型与密钥 / 技能市场 / 手机访问四页都处在空态或入口闸上。
    app = await startApp({ script: foremanScript([[]]), seedless: true });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('四个页面的空态都说清「现在是什么 + 下一步做什么」', async ({ page }) => {
    const pages = [
      { hash: '#/settings/projects', heading: '设置 · 项目', state: /还没有项目/, next: /加进来之后才能创建任务/ },
      { hash: '#/settings/providers', heading: '设置 · 模型与密钥', state: /还没有 provider/, next: /阶段模型才会被解析/ },
      { hash: '#/settings/market', heading: '设置 · 技能市场', state: /仓名单是空的/, next: /加进来并保存/ },
      { hash: '#/share', heading: '手机访问', state: /手机现在连不上这台机器/, next: /不在同一个地址空间/ },
    ] as const;

    for (const p of pages) {
      await open(page, app, p.hash, p.heading);
      // 状态那半句：说清「现在是空的、缺什么」
      await expect(atText(page, p.state)).toBeVisible();
      // 下一步那半句：说清「做什么」——两半都在，形状才算成立
      await expect(atText(page, p.next)).toBeVisible();
    }
  });

  test('空态文字用的是可读档（--text-3）而不是装饰档，且过 4.5:1', async ({ page }) => {
    const pages = [
      { hash: '#/settings/projects', heading: '设置 · 项目', next: /加进来之后才能创建任务/ },
      { hash: '#/settings/providers', heading: '设置 · 模型与密钥', next: /阶段模型才会被解析/ },
      { hash: '#/settings/market', heading: '设置 · 技能市场', next: /加进来并保存/ },
      { hash: '#/share', heading: '手机访问', next: /不在同一个地址空间/ },
    ] as const;

    for (const p of pages) {
      await open(page, app, p.hash, p.heading);
      const next = atText(page, p.next);
      await expect(next).toBeVisible();

      const color = await colorOf(next);
      // 档位：次级必读（--text-3），不是纯装饰的 --text-4
      expect(color).toBe(rgbString(await rootToken(page, '--text-3')));
      expect(color).not.toBe(rgbString(await rootToken(page, '--text-4')));
      // 可读性：对这一处**真正的承载底色**（页面底或面板底）过 AA 的 4.5:1
      const ratio = contrastRatio(color, await backingColor(next));
      expect(
        ratio,
        `${p.heading} 的空态文字 ${color} 对底色 ${await backingColor(next)} 只有 ${ratio.toFixed(2)}:1`,
      ).toBeGreaterThanOrEqual(4.5);
    }
  });

  test('空态提到的另一个页面可点，点了能到那一页', async ({ page }) => {
    await open(page, app, '#/settings/providers', '设置 · 模型与密钥');

    const link = page.getByRole('link', { name: /设置 · 项目/ }).first();
    await expect(link).toBeVisible();
    await expect(link).toHaveAttribute('href', '#/settings/projects');
    await link.click();

    // 真的到了「设置 · 项目」那一页（不是空跳转、不是留在原地）
    await expect(page.locator('.page h1.p-title')).toContainText('设置 · 项目');
    await expect(atText(page, /加进来之后才能创建任务/)).toBeVisible();
  });

  test('设置页的小节标题明确小于页面标题，且不引入阶外字号（票 09）', async ({ page }) => {
    for (const [hash, heading] of [
      ['#/settings/market', '设置 · 技能市场'],
      // 推荐技能面板随阶段配置整块搬来了这一页（决策 198 裁决③），「模型与密钥」页自此
      // 没有小节标题（只剩 h1 与台账盒的 div 头）——小节标题的第二处取证落点跟着搬。
      ['#/settings/stages', '设置 · 阶段配置'],
    ] as const) {
      await open(page, app, hash, heading);

      const titleSize = await page
        .locator('.page h1.p-title')
        .evaluate((el) => getComputedStyle(el).fontSize);
      expect(titleSize).toBe('24px');

      // 小节标题：市场的「仓名单 / 技能列表」是 h2，阶段配置的「阶段配置」是 h2、「推荐技能」是区块头
      const sizes = await page
        .locator('h2, .rec-head')
        .evaluateAll((els) => els.map((el) => getComputedStyle(el).fontSize));
      expect(sizes.length, `${heading} 上一个小节标题都没找到`).toBeGreaterThan(0);
      for (const size of sizes) {
        expect(Number.parseFloat(size)).toBeLessThan(Number.parseFloat(titleSize));
        // 像素字号阶：12 的整数倍
        expect(Number.parseFloat(size) % 12).toBe(0);
      }
    }
  });

  test('琥珀只留在有东西要你处理的地方（票 12）：入口闸标题与「未安装」回中性档', async ({
    page,
  }) => {
    // ① 手机访问的入口闸标题：这一块是「现在拿不到什么 + 下一步按哪颗钮」，不是告警
    //（token 要在页面装载之后读：`--pending` 是应用样式表里的，about:blank 上读不到）
    await open(page, app, '#/share', '手机访问');
    const pending = rgbString(await rootToken(page, '--pending'));
    const gateTitle = atText(page, /手机现在连不上这台机器/);
    await expect(gateTitle).toBeVisible();
    const gateColor = await colorOf(gateTitle);
    expect(gateColor).toBe(rgbString(await rootToken(page, '--text-hi')));
    expect(gateColor).not.toBe(pending);

    // ② 推荐技能的「未安装」：只有文字编码、要人读 → 次级必读档（决策 195）。
    // 面板已随阶段配置搬到 `#/settings/stages`（决策 198 裁决③），取证跟着它走。
    await open(page, app, '#/settings/stages', '设置 · 阶段配置');
    const state = page.locator('.item .state').first();
    await expect(state).toHaveText(/未安装/);
    const stateColor = await colorOf(state);
    expect(stateColor).toBe(rgbString(await rootToken(page, '--text-3')));
    expect(stateColor).not.toBe(pending);
  });

  test('四页正文里不再出现内部决策编号（票 23）', async ({ page }) => {
    for (const [hash, heading] of [
      ['#/settings/projects', '设置 · 项目'],
      ['#/settings/providers', '设置 · 模型与密钥'],
      ['#/settings/market', '设置 · 技能市场'],
      ['#/share', '手机访问'],
    ] as const) {
      await open(page, app, hash, heading);
      // 屏幕上的正文：「决策 NN」一个都不该有（代码注释里的编号照旧，屏幕上读不到）
      await expect(page.locator('.page')).not.toContainText(/决策\s*\d/);
    }
  });
});

/* ───────────────── ② 有项目与活跃任务：动手时的理由 + query 就位 ───────────────── */

test.describe('前端 E2E：项目页动手时的理由与 query 就位（票 04 / 07 / 12 / 23）', () => {
  let app: App;
  const title = 'E2E 设置台账';

  test.beforeAll(async () => {
    app = await startApp({
      script: {
        ...fullPassScript('E2E'),
        // 项目分析伪阶段（票 07 的消费端）：给出可解析的元数据，分析才有终态
        'pseudo:project_analysis': [[submit({ summary: 'e2e 项目分析', suspicious: [] })]],
      },
      title,
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('删除被拒的理由只在动手之后出现，说的是一步动作（票 04 / 23）', async ({ page }) => {
    await open(page, app, '#/settings/projects', '设置 · 项目');

    const row = page.locator('li.reg-row', { hasText: 'e2e' }).first();
    await expect(row).toBeVisible();

    // ① 动手之前：这一行上没有那条常驻的红字解释
    await expect(row.locator('.reg-err')).toHaveCount(0);

    const remove = row.getByRole('button', { name: '删除' });
    // 「先禁掉再在旁边常驻解释」不是这条票要的形态：按钮得按得动，理由在动手时给
    await expect(remove).toBeEnabled();
    // 追溯性收在 title 里（理由，不夹内部编号）
    const hint = (await remove.getAttribute('title')) ?? '';
    expect(hint).toContain('不能删除');
    expect(hint).not.toContain('决策');

    // ② 动手：点「删除」→ 被拒 → 出现的是下一步（不是「不能删」的原因）
    await remove.click();
    const reason = row.locator('.reg-err');
    await expect(reason).toBeVisible();
    await expect(reason).toContainText(/先去处理那 \d+ 个任务，然后再删除这个项目/);
    await expect(reason).not.toContainText(/决策/);

    // 被拒了就不该进入确认态（不给人一个注定失败的确认）
    await expect(row.locator('.reg-sub', { hasText: '确认删除' })).toHaveCount(0);
  });

  test('破坏性动作在视觉上不比中性动作轻（票 04）', async ({ page }) => {
    await open(page, app, '#/settings/projects', '设置 · 项目');

    const row = page.locator('li.reg-row').first();
    const remove = row.getByRole('button', { name: '删除' });
    const edit = row.getByRole('button', { name: '编辑' });
    await expect(edit).toBeVisible();

    // 同一档量级：描边宽度不更少、硬投影不少一层、尺寸不更小
    for (const prop of ['border-top-width', 'border-left-width']) {
      const [delWidth, editWidth] = await Promise.all([
        remove.evaluate((el, p) => getComputedStyle(el).getPropertyValue(p), prop),
        edit.evaluate((el, p) => getComputedStyle(el).getPropertyValue(p), prop),
      ]);
      expect(delWidth, `删除比编辑少了一层描边（${prop}）`).toBe(editWidth);
    }

    const delShadow = await remove.evaluate((el) => getComputedStyle(el).boxShadow);
    const editShadow = await edit.evaluate((el) => getComputedStyle(el).boxShadow);
    expect(delShadow).not.toBe('none');
    expect(delShadow, '破坏性动作比中性动作少一层硬投影').toBe(editShadow);

    const [delBox, editBox] = await Promise.all([remove.boundingBox(), edit.boundingBox()]);
    expect(delBox?.height ?? 0).toBeGreaterThanOrEqual(editBox?.height ?? 0);

    // 仍然是「破坏性」的样子（失败红 vs 中性文字档），不是被染成中性色藏起来
    expect(await colorOf(remove)).not.toBe(await colorOf(edit));
  });

  test('项目页据 query 自动就位并触发分析（票 07）', async ({ page }) => {
    const body = (await (await fetch(`${app.apiBase}/projects`)).json()) as {
      projects: Array<{ id: string; name: string }>;
    };
    const target = body.projects[0];
    expect(target, '播种的项目没读到').toBeTruthy();

    // 任务侧给的就是这个入口（跨流接口：`#/settings/projects?project=<id>&analyze=1`）
    await open(
      page,
      app,
      `#/settings/projects?project=${encodeURIComponent(target.id)}&analyze=1`,
      '设置 · 项目',
    );

    // ① 就位：目标那一行被标出来（不需要用户再选一次）
    const spot = page.locator('li.reg-row.current');
    await expect(spot).toBeVisible();
    await expect(spot).toContainText(target.name);

    // ② 触发分析：分析块出现，并说清看的是哪个项目
    const checklist = page.locator('section.checklist');
    await expect(checklist).toBeVisible({ timeout: 60_000 });
    await expect(checklist).toContainText(target.name);
  });

  test('读不到 query 时行为不变（票 07）', async ({ page }) => {
    await open(page, app, '#/settings/projects', '设置 · 项目');

    await expect(page.locator('li.reg-row').first()).toBeVisible();
    // 不标行、也不自己触发分析
    await expect(page.locator('li.reg-row.current')).toHaveCount(0);
    await expect(page.locator('section.checklist')).toHaveCount(0);
  });

  test('不受支持的厂商行：正文没有编号，琥珀标记**保留**（票 12 / 23 / 决策 203）', async ({ page }) => {
    // 播种一行「不在支持列表里」的厂商（升级后被移除 / 手工改库的情形，界面允许它存在）
    const created = await fetch(`${app.apiBase}/providers`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
      body: JSON.stringify({ vendor: 'ollama', model: 'llama', context_window: 8000, enabled: true }),
    });
    expect(created.ok, `播种不受支持的 provider 失败：${created.status}`).toBe(true);

    await open(page, app, '#/settings/providers', '设置 · 模型与密钥');

    const marker = page.locator('.warnnote.inline').first();
    await expect(marker).toContainText(/不支持这个厂商，该行已停用/);
    await expect(marker).not.toContainText(/决策/);

    // 编号的归宿是 title（放的是理由，不夹编号）
    const titleAttr = (await marker.getAttribute('title')) ?? '';
    expect(titleAttr).toContain('支持列表');
    expect(titleAttr).not.toContain('决策');

    // 琥珀收敛的**保留**一边：这一行是规格明文允许的琥珀用法（决策 203 裁决③），
    // 「该行已停用」意味着用户得动手处理（换厂商或删掉它）——收敛掉的不是它。
    const color = await colorOf(marker);
    expect(color).toBe(rgbString(await rootToken(page, '--pending')));
    expect(color).not.toBe(rgbString(await rootToken(page, '--text-3')));
  });
});
