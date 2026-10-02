/**
 * UX 审计截图（证据，非闸门）。
 *
 * 与 `screenshots.spec.ts` 的分工：那条只出「每个路由一张」的基线；这一条出**交互态**
 * ——空库、过滤、下拉、对话框、pending 档案盒、对讲台多急停、市场列表、手机访问两种
 * 绑定态，以及票 01 补拍的两组逐处证据（琥珀五处用途 / 七处空态 + 404，均深浅两套）。
 * 产物落 `.scratch/ux-audit/`，供人工审 UI/UX。
 *
 * **取证拿的是「改动前」的现状**：这里的页面来自被测二进制**编译期内嵌**的 `frontend/dist`
 * （决策 155 的 `crates/app/build.rs`：dist 逐文件 `include_bytes!` 进二进制；harness 的
 * `assertEmbeddedBundle` 就是这条的守卫）。所以磁盘上的 `frontend/dist` 被同轮并行改动
 * 重新构建得更晚，也不会改变本 spec 拿到的画面——这也是本文件不重建产物的原因。
 *
 * 默认 skip：审计不是闸门，不该拖慢 `make check-e2e`。用法（仓库根）：
 *   cd frontend && UX_AUDIT=1 AGENTPIPELINE_E2E_BIN=target/release/agent-pipeline \
 *     npx playwright test --project=chromium e2e/ux-audit.spec.ts
 */

import { mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, type Locator, type Page } from '@playwright/test';
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

test.skip(process.env.UX_AUDIT !== '1', '审计截图是证据不是门；设 UX_AUDIT=1 才跑');

const here = dirname(fileURLToPath(import.meta.url));
const outDir = resolve(here, '..', '..', '.scratch', 'ux-audit');

/** 任意合法形状的 id：只用来让值班长翻一次台账，回执行照样渲染。 */
const SOME_ID = '01JZZZZZZZZZZZZZZZZZZZZZZZ';

/** 一个**停在 develop** 的任务：`sleep` 让它在截图期间一直是 running。 */
const stalledScript = {
  ...siblingDesignRounds(),
  [NODE.developEx]: [[tool('run_command', { command: 'sleep 900' })]],
};

/** 值班长的回话：先翻两处台账（出「工位回执」），再给结论。 */
const foremanRounds = foremanScript([
  [readTask(SOME_ID), tool('read_conversation', { task_id: SOME_ID, run_id: null }), text('当前态势')],
  [text('三件货箱卡在合入前等拍板，两个工位还在跑。')],
  [text('合计 8 个工位：1 个在跑、3 个急停、其余空闲。')],
]);

async function shot(page: Page, name: string): Promise<void> {
  await page.screenshot({ path: resolve(outDir, `${name}.png`), fullPage: true });
}

/** 换主题后重渲染（与 screenshots.spec.ts 同姿态：直接改 dataset 更稳）。 */
async function setTheme(page: Page, theme: 'dark' | 'light'): Promise<void> {
  await page.evaluate((t) => {
    document.documentElement.dataset.theme = t;
    localStorage.setItem('agentpipeline.theme', t);
  }, theme);
  await page.waitForTimeout(350);
}

/** 进页面并等产物执行完（不能用 networkidle：SSE 常驻）。 */
async function open(page: Page, app: App, hash: string): Promise<void> {
  await page.goto(`${app.webBase}/${hash}`);
  await page.waitForLoadState('load');
  await page.waitForTimeout(700);
}

/** 逐处取证的「放大镜」：只截那一个元素，深浅两套并排比对才看得出一处是不是琥珀。 */
async function clip(page: Page, name: string, target: Locator): Promise<void> {
  const el = target.first();
  await el.scrollIntoViewIfNeeded();
  await page.waitForTimeout(200);
  await el.screenshot({ path: resolve(outDir, `${name}.png`) });
}

/**
 * 逐处取证的「量尺」：读**计算样式**（用户可见真值）并打到 stdout。
 *
 * 为什么必须有：审计的结论不能靠 class 名或印象——`--pending` 与 `--text-hi` 在深色款下
 * 都是暖色，肉眼在小字号上会看错（本轮就纠正了一处）。这里把「这一处到底取哪枚 token」
 * 变成可引用的输出，顺带把根上的 token 取值一起打出来。
 */
async function probeColor(page: Page, label: string, target: Locator): Promise<void> {
  const info = await target.first().evaluate((node) => {
    const cs = getComputedStyle(node);
    const root = getComputedStyle(document.documentElement);
    const tok = (name: string) => root.getPropertyValue(name).trim();
    return {
      text: (node.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 32),
      color: cs.color,
      borderTop: cs.borderTopColor,
      borderLeft: cs.borderLeftColor,
      background: cs.backgroundColor,
      tokens: {
        '--pending': tok('--pending'),
        '--text-hi': tok('--text-hi'),
        '--text-3': tok('--text-3'),
        '--text-4': tok('--text-4'),
      },
    };
  });
  console.log(`[audit] ${label} ${JSON.stringify(info)}`);
}

/**
 * 阶段配置的取证入口（决策 198：设置的信息架构本轮改过——阶段配置从「模型与密钥」页
 * 搬出、独立成 `#/settings/stages`）。
 *
 * **新路由优先、旧产物回落**：本轮证据取自已构建的 release 二进制（编译期内嵌 dist，
 * 早于该 IA 改动），新路由在它上面还不存在；两条路都留着，下一轮取证自动走新路由。
 */
async function openStageConfig(
  page: Page,
  app: App,
): Promise<'settings-stages' | 'settings-providers'> {
  await open(page, app, '#/settings/stages');
  if (await page.getByRole('button', { name: /新增阶段配置/ }).count()) {
    console.log('[audit] 阶段配置取证走新路由 #/settings/stages（决策 198）');
    return 'settings-stages';
  }
  console.log('[audit] 阶段配置取证走旧路由 #/settings/providers（内嵌产物早于决策 198）');
  await open(page, app, '#/settings/providers');
  return 'settings-providers';
}

test.describe('① 有数据的库：看板 / 详情 / 对讲台 / 台账', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({
      script: { ...fullPassScript('UX-MAIN'), ...foremanRounds },
      title: '实现用户登录接口',
      additionalTasks: [
        { title: '把登录失败原因写进审计日志', script: siblingPassScript('UX-EXTRA1') },
        { title: '给导出命令加一个 --since 参数', script: stalledScript },
      ],
    });
    // 琥珀用途取证（票 01 第 4 处）：`.warnnote`（§3.2「行内降级」的琥珀标）只在
    // **不受支持厂商**那一行渲染，而 harness 只播 `openai`（受支持）。
    // `SUPPORTED_ADAPTERS = openai / deepseek / anthropic`（`lib/providers.ts:16`），
    // 故补一个 `gemini`；`enabled: false` 让它不参与任何阶段解析，主任务不受影响。
    const res = await fetch(`${app.apiBase}/providers`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
      body: JSON.stringify({
        vendor: 'gemini',
        model: 'gemini-2.5-pro',
        context_window: 1000000,
        base_url: 'https://example.invalid',
        api_key: 'sk-test',
        enabled: false,
      }),
    });
    if (!res.ok) throw new Error(`播种「不受支持厂商」provider 失败：${res.status}`);
    // 让看板先有「跑到一半」的样子：三件事分散在不同列
    await new Promise((r) => setTimeout(r, 7000));
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('看板：列 / 卡片 / 过滤 / 下拉 / 新建对话框', async ({ page }) => {
    const guard = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    await shot(page, 'board-dark');

    await setTheme(page, 'light');
    await shot(page, 'board-light');
    await setTheme(page, 'dark');

    // 过滤：已完成（第 6 格；顺序见 TopBar 的 FILTERS）
    await page.locator('.slots .slot').nth(5).click();
    await page.waitForTimeout(300);
    await shot(page, 'board-filter-done');
    await page.locator('.slots .slot').nth(0).click();
    await page.waitForTimeout(300);

    // 待处理下拉
    await page.locator('.pending-count').click();
    await page.waitForTimeout(300);
    await shot(page, 'board-pending-dropdown');
    await page.locator('.pending-count').click();
    await page.waitForTimeout(300);

    // 新建任务对话框
    await page.locator('.btn-new').click();
    await page.waitForTimeout(400);
    await shot(page, 'board-newtask-dialog');
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);

    // 窄屏
    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, '#/');
    await setTheme(page, 'dark');
    await shot(page, 'mobile-board');
    void guard;
  });

  test('详情：pending（档案盒）与五个页签', async ({ page }) => {
    const guard = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    // 等主任务挂到 merge_approval —— 那才是档案盒最全的样子
    await waitForTaskById(
      app.taskIds[0],
      app,
      (t) => {
        const reason = t.pending_reason as { type?: string } | null | undefined;
        return reason?.type === 'merge_approval';
      },
      'merge_approval',
      180_000,
    ).catch(() => undefined);

    await open(page, app, `#/task/${app.taskIds[0]}`);
    await setTheme(page, 'dark');
    await shot(page, 'detail-pending-dossier');
    await setTheme(page, 'light');
    await shot(page, 'detail-pending-dossier-light');
    await setTheme(page, 'dark');

    for (const [slug, label] of [
      ['timeline', '时间线'],
      ['scene', '现场'],
      ['files', '产出文件'],
      ['diff', 'Diff'],
    ] as const) {
      const tab = page.getByRole('button', { name: new RegExp(label) });
      if (await tab.count()) {
        await tab.first().click();
        await page.waitForTimeout(1000);
        await shot(page, `detail-tab-${slug}`);
      }
    }
    void guard;
  });

  test('详情：在跑的任务（桌面 + 窄屏）', async ({ page }) => {
    const guard = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, `#/task/${app.taskIds[2]}`);
    await setTheme(page, 'dark');
    await shot(page, 'detail-running');

    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, `#/task/${app.taskIds[0]}`);
    await setTheme(page, 'dark');
    await shot(page, 'mobile-detail-pending');

    await open(page, app, `#/task/${app.taskIds[2]}`);
    await shot(page, 'mobile-detail-running');
    void guard;
  });

  test('对讲台：多急停 + 一轮对话', async ({ page }) => {
    const guard = watchBundle(page);
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/talk');
    await setTheme(page, 'dark');
    await shot(page, 'talk-stops');

    // 展开第一张急停（多张时默认全折叠）
    const expander = page.getByRole('button', { name: /展开恢复动作/ });
    if (await expander.count()) {
      await expander.first().click();
      await page.waitForTimeout(600);
      await shot(page, 'talk-stop-expanded');
    }

    // 发一句话，看时间线里的几种「轮」
    await page.locator('textarea.input').fill('现在卡在哪几个工位？');
    await page.getByRole('button', { name: /发送/ }).click();
    await page.waitForTimeout(3000);
    await shot(page, 'talk-conversation');

    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, '#/talk');
    await setTheme(page, 'dark');
    await shot(page, 'mobile-talk');
    void guard;
  });

  test('指标：有数据', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/metrics');
    await setTheme(page, 'dark');
    await shot(page, 'metrics');
    await setTheme(page, 'light');
    await shot(page, 'metrics-light');
    await setTheme(page, 'dark');

    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, '#/metrics');
    await shot(page, 'mobile-metrics');
  });

  /**
   * 票 01 的第 1–3 项：琥珀五处用途的**特写 + 计算样式**，深浅两套。
   *
   * 结论落 `.scratch/ux-audit/README.md`「核查后的更正」第 6 条。这一条只取证、不判定，
   * 判定要引规格原文，故不在 spec 里写断言（文案与色值都在动，硬断言会与并行改动打架）。
   */
  test('琥珀用途：五处逐处特写与计算样式（深浅两套）', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });

    // ① 指标页「各阶段重试率」：每根轨道上的 12px 描边方块标记（值全是 0.0% 也照样琥珀）
    await open(page, app, '#/metrics');
    for (const theme of ['dark', 'light'] as const) {
      await setTheme(page, theme);
      const retry = page.locator('.chart').filter({ hasText: '各阶段重试率' });
      await clip(page, `amber-metrics-retry-${theme}`, retry);
      await probeColor(page, `${theme} ①指标·重试率轨道标记 .mdot`, retry.locator('.mdot'));
      await probeColor(page, `${theme} ①指标·重试率横条 .fill`, retry.locator('.fill'));
      await probeColor(page, `${theme} ①指标·重试率数值 .val`, retry.locator('.val'));
      // 同一屏上的对照组：平均耗时那张固定用 dev 档。两相对照才看得出「灯色是按图固定的」
      const dur = page.locator('.chart').filter({ hasText: '各阶段平均耗时' });
      await probeColor(page, `${theme} ①对照·耗时轨道标记 .mdot`, dur.locator('.mdot'));
    }

    // ② 推荐技能的「未安装」标签（决策 198 起推荐面板随阶段配置在 #/settings/stages——
    // 第一轮取证时它还在模型与密钥页，路由已迁、选择器跟着改；截图文件名保持原样，
    // 第一轮 README 引的就是它）
    await open(page, app, '#/settings/stages');
    for (const theme of ['dark', 'light'] as const) {
      await setTheme(page, theme);
      const rec = page.locator('.rec');
      await clip(page, `amber-providers-uninstalled-${theme}`, rec);
      await probeColor(page, `${theme} ②推荐技能·未安装 .state`, rec.locator('.state'));
    }

    // ③ 阶段配置区标题（以及展开表单后那块「整条替换」题注——两者不是同一处）。
    // 同页（决策 198）：区标题是 `.stage-head`（第一轮时是 providers 页的 `.sub-head`）
    for (const theme of ['dark', 'light'] as const) {
      await setTheme(page, theme);
      const head = page.locator('.stage-head');
      await clip(page, `amber-stageconfig-title-${theme}`, head);
      await probeColor(page, `${theme} ③阶段配置区标题 .stage-head h2`, head.locator('h2'));
      await page.getByRole('button', { name: /新增阶段配置/ }).click();
      await page.waitForTimeout(400);
      const note = page.locator('.replace-note');
      if (await note.count()) {
        await clip(page, `amber-stageconfig-replace-note-${theme}`, note);
        await probeColor(page, `${theme} ③b阶段配置表单·整条替换题注 .replace-note`, note);
      }
      await page.getByRole('button', { name: /取消/ }).first().click();
      await page.waitForTimeout(300);
    }

    // ④ 告警注记 `.warnnote`：只在「不受支持厂商」那一行（beforeAll 播的 gemini）
    await open(page, app, '#/settings/providers');
    for (const theme of ['dark', 'light'] as const) {
      await setTheme(page, theme);
      const row = page.locator('.reg-row.dead').first();
      if (await row.count()) {
        await clip(page, `amber-warnnote-${theme}`, row);
        await probeColor(page, `${theme} ④告警注记 .warnnote`, row.locator('.warnnote').first());
      } else {
        console.log(`[audit] ${theme} ④告警注记 .warnnote 未渲染（该行不是不受支持厂商？）`);
      }
    }

    // ⑤ 手机访问页的入口闸标题（回环态默认就是闸；外加同页的风险提示作对照）。
    // 票 13 之后闸标题在多数分支换成 EmptyState 的 `.es`（`.gate-head` 只剩「读不到令牌」
    // 那一分支还在）——回环态走到的是 EmptyState，故两个选择器并取、取到哪个算哪个。
    await open(page, app, '#/share');
    for (const theme of ['dark', 'light'] as const) {
      await setTheme(page, theme);
      const gate = page.locator('.gate');
      await clip(page, `amber-share-gate-${theme}`, gate);
      await probeColor(
        page,
        `${theme} ⑤手机访问·入口闸标题（.gate-head / .es）`,
        page.locator('.gate .gate-head, .gate .es').first(),
      );
      await probeColor(page, `${theme} ⑤b手机访问·风险提示 .warn`, page.locator('.gate .warn'));
    }
  });

  /**
   * 看板空**列**：`design/frontend-design.md` §5.3（HEAD:181-183）是**唯一**写了空态形状的
   * 地方（「空列不放插画，一句话」），所以它的实现偏离要单独留一张特写——
   * 实现是 `.col-empty`（`BoardColumn.svelte:169`，`--text-4` 装饰档）。
   */
  test('空列：规格 §5.3 写过的那一处（深浅两套）', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/');
    for (const theme of ['dark', 'light'] as const) {
      await setTheme(page, theme);
      const col = page.locator('.col').filter({ has: page.locator('.col-empty') }).first();
      if (await col.count()) {
        await clip(page, `empty-board-column-${theme}`, col);
        await clip(page, `empty-board-column-text-${theme}`, col.locator('.col-empty'));
        await probeColor(page, `${theme} 空列·引导句 .col-empty`, col.locator('.col-empty'));
      } else {
        console.log(`[audit] ${theme} 空列未出现（每列都占了？）`);
      }
    }
  });

  test('项目 / 模型与密钥 / 阶段配置：列表与表单', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });

    await open(page, app, '#/settings/providers');
    await setTheme(page, 'dark');
    await shot(page, 'providers-list');
    await page.getByRole('button', { name: /新增 provider/ }).click();
    await page.waitForTimeout(400);
    await shot(page, 'providers-form');
    await page.getByRole('button', { name: /取消/ }).first().click();
    await page.waitForTimeout(300);

    // 阶段配置：决策 198 起它有自己的路由（`#/settings/stages`）。这一步走 openStageConfig
    // ——新路由优先、旧产物回落，下一轮取证不必再改这一条。
    const stagesRoute = await openStageConfig(page, app);
    await setTheme(page, 'dark');
    await shot(page, `stageconfig-page-${stagesRoute}`);
    await page.getByRole('button', { name: /新增阶段配置/ }).click();
    await page.waitForTimeout(400);
    await shot(page, `stageconfig-form-${stagesRoute}`);
    await page.getByRole('button', { name: /取消/ }).first().click();
    await page.waitForTimeout(300);

    // 项目页放最后：三个任务都在飞，删除钮是**禁用**的（决策 101），
    // 这一张截的就是「按钮禁掉 + 旁边写出原因」的样子。
    await open(page, app, '#/settings/projects');
    await setTheme(page, 'dark');
    await shot(page, 'projects-list');
    await page.getByRole('button', { name: /新建项目/ }).click();
    await page.waitForTimeout(400);
    await shot(page, 'projects-form');

    await page.setViewportSize({ width: 430, height: 932 });
    await open(page, app, '#/settings/projects');
    await shot(page, 'mobile-projects');
    await open(page, app, '#/settings/providers');
    await shot(page, 'mobile-providers');
  });

  test('手机访问：回环态 → 改绑后的二维码', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/share');
    await setTheme(page, 'dark');
    await shot(page, 'share-loopback');

    // 真按那颗钮：改绑全网卡 → 二维码态
    const toggle = page.getByRole('button', { name: /绑定全网卡/ });
    if (await toggle.count()) {
      await toggle.click();
      await page.waitForTimeout(5000);
      await shot(page, 'share-lan-qr');
      await setTheme(page, 'light');
      await shot(page, 'share-lan-qr-light');
      await setTheme(page, 'dark');

      await page.setViewportSize({ width: 430, height: 932 });
      await open(page, app, '#/share');
      await shot(page, 'mobile-share-lan');
    }
  });

  test('404：不存在路由（深浅两套）', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/nope');
    for (const theme of ['dark', 'light'] as const) {
      await setTheme(page, theme);
      await shot(page, theme === 'dark' ? 'notfound' : 'notfound-light');
    }
    // 取证用：404 页面上「有没有一条能走的路」是字面意义上的可测项
    const links = await page.locator('main a, .page a, body a').count();
    const text = await page.locator('body').innerText();
    console.log(
      `[audit] 404 链接数=${links} 正文=${JSON.stringify(text.replace(/\s+/g, ' ').trim().slice(0, 120))}`,
    );
  });
});

test.describe('② 空库：首启空态', () => {
  let app: App;

  test.beforeAll(async () => {
    mkdirSync(outDir, { recursive: true });
    app = await startApp({ script: foremanScript([[]]), seedless: true });
    await new Promise((r) => setTimeout(r, 1500));
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('对话框的键盘可达性：Escape / 焦点', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/');
    await page.locator('.btn-new').click();
    await page.waitForTimeout(400);
    const before = await page.locator('.dialog').count();
    const focusedAfterOpen = await page.evaluate(
      () => `${document.activeElement?.tagName}.${document.activeElement?.className ?? ''}`,
    );
    await page.keyboard.press('Escape');
    await page.waitForTimeout(400);
    const afterEsc = await page.locator('.dialog').count();
    // 票 02 之后：打开即焦点进第一个可填控件、Escape 挂在 window 上——**第一下就关**。
    // 第一轮取证时这里依赖「焦点在框外 Escape 接不到」的旧行为，故先按新契约取数。
    // 重开一颗，把焦点显式放进输入框再 Escape：焦点在框内时同样关得掉（两条路都取证）
    await page.locator('.btn-new').click();
    await page.waitForTimeout(400);
    await page.locator('.dialog input').first().focus();
    await page.keyboard.press('Escape');
    await page.waitForTimeout(400);
    const afterEscFocused = await page.locator('.dialog').count();
    console.log(
      `[audit] dialog before=${before} afterEsc=${afterEsc} afterEscFocused=${afterEscFocused} focus=${focusedAfterOpen}`,
    );
    await shot(page, 'dialog-escape-check');
  });

  test('空看板 / 空对讲台 / 空指标 / 空台账：七处空态深浅两套', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    for (const [slug, hash] of [
      ['empty-board', '#/'],
      ['empty-talk', '#/talk'],
      ['empty-metrics', '#/metrics'],
      ['empty-projects', '#/settings/projects'],
      ['empty-providers', '#/settings/providers'],
      ['empty-market', '#/settings/market'],
      ['empty-share', '#/share'],
    ] as const) {
      await open(page, app, hash);
      for (const theme of ['dark', 'light'] as const) {
        await setTheme(page, theme);
        // 深色款沿用审计原稿的文件名（README 里逐条引的是它），浅色款补 `-light`
        await shot(page, theme === 'dark' ? slug : `${slug}-light`);
      }
      // 取证用：这一页此刻的正文——空态「说不说清下一步」「提到别处是不是可点」都在这行字里
      const text = await page.locator('body').innerText();
      const links = await page.locator('body a[href]').evaluateAll((els) =>
        els.map((e) => `${(e.textContent ?? '').trim()}=${e.getAttribute('href')}`),
      );
      const emptyBlocks = await page
        .locator('.board-empty, .empty, .col-empty, .gate-head')
        .evaluateAll((els) =>
          els.map((e) => (e.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 150)),
        );
      console.log(
        `[audit] ${slug} 正文=${JSON.stringify(text.replace(/\s+/g, ' ').trim().slice(0, 420))}`,
      );
      console.log(`[audit] ${slug} 正文内的链接=${JSON.stringify(links)}`);
      console.log(`[audit] ${slug} 空态块=${JSON.stringify(emptyBlocks)}`);
    }
  });
});

test.describe('③ 技能市场：带离线仓的列表', () => {
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

  test('仓名单 + 技能列表', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await open(page, app, '#/settings/market');
    await setTheme(page, 'dark');
    await shot(page, 'market-repo-list');

    const view = page.getByRole('button', { name: /查看技能/ });
    if (await view.count()) {
      await view.first().click();
      await page.waitForTimeout(4000);
      await shot(page, 'market-listing');
      await setTheme(page, 'light');
      await shot(page, 'market-listing-light');
    }
  });
});
