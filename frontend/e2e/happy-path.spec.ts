/**
 * E2E-① happy path（票 18 / 决策 151；主流程票 01 扩到真实产物 + 合入结果）：
 * 看板 → 任务详情 → 页签切换 → diff 审批合入 → **校验合入到 main 的代码符合任务目标**。
 *
 * 真 axum 后端 + mock LLM（FakeAgent 同一替换边界）+ 临时 home（决策 148）。
 * 页面加载的是**编译期内嵌的真实 bundle**（主流程票 01），不再经 Vite dev server。
 * 只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';
import { execFileSync } from 'node:child_process';

import {
  startApp,
  waitForTask,
  pendingTypeOf,
  watchBundle,
  settleBundle,
  expectBundleHealthy,
  expectGateReallyRan,
  type App,
} from './harness';
import { fullPassScript } from './scripts';

test.describe('前端 E2E ①：happy path（看板 → 详情 → 页签 → diff 审批合入）', () => {
  let app: App;
  const title = 'E2E happy path';

  test.beforeAll(async () => {
    // 任务 id 在播种后才知道：先以占位脚本启动，再用任务 id 无关的脚本站位——
    // develop 的 CodeChanges.branch_name 由后端从游标取，脚本里的 branch_name 只是元数据，
    // 因此这里用固定前缀即可（真 git 提交命令里的 task id 也不影响流水线推进）。
    app = await startApp({ script: fullPassScript('E2E'), title });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('看板展示任务 → 进详情 → 切页签 → Diff 审批合入到 done', async ({ page }) => {
    // 真实产物守卫（主流程票 01）：加载的是内嵌 bundle，不是 dev server 资源。
    const bundle = watchBundle(page);

    // ── 真实产物守卫（主流程票 01）：先确认内嵌 bundle 健康，再等业务断言 ──
    // 顺序很重要：若产物本身坏掉（资源 404 / JS 抛错），业务断言只会以 60s 超时收场，
    // 根因被淹没。这里先落地一次健康裁决，让「白屏类」失败立刻指名道姓。
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);

    // ── 看板：任务卡出现在看板上（真后端 seeding） ──
    // 卡片由 Svelte 从 API 数据渲染，它可见即证明内嵌 bundle 已执行——白屏在这一步必失败，
    // 而 `page.goto` 成功对白屏同样会绿。
    const card = page.locator('article.card', { hasText: title });
    await expect(card).toBeVisible({ timeout: 60_000 });

    // ── 详情：点卡进入 `#/task/{id}` ──
    await card.locator('a.card-link').click();
    await expect(page).toHaveURL(new RegExp(`#/task/${app.taskId}`));
    await expect(page.locator('h1.d-title')).toHaveText(title);

    // ── 页签切换：时间线 → 会话 → 命令与输出 → 产出文件 ──
    for (const label of ['[会话]', '[命令与输出', '[产出文件]']) {
      const tab = page.locator('nav.tabs button.tab', { hasText: label });
      await tab.click();
      await expect(tab).toHaveClass(/on/);
    }
    // 产出文件页签真的渲染了产物清单（真后端文件 API）
    await expect(page.locator('.fileview .list')).toContainText('design.md');

    // ── 等流水线推进到 pending(merge_approval)：dossier 琥珀面板出现 ──
    await waitForTask(app, (t) => pendingTypeOf(t) === 'merge_approval', 'merge_approval');
    const dossier = page.locator('aside.dossier');
    await expect(dossier).toBeVisible({ timeout: 60_000 });
    await expect(dossier.locator('.dtag')).toContainText('合并提案');

    // ── Diff 页签：切过去并点击「合入」 ──
    const diffTab = page.locator('nav.tabs button.tab', { hasText: '[Diff]' });
    await expect(diffTab).toBeEnabled({ timeout: 60_000 });
    await diffTab.click();
    // diff 面板在页签区与 dossier 各有一处（同一组件两处渲染），限定页签容器内那个
    await expect(page.locator('.pane .diffpanel').first()).toBeVisible();

    // 合入按钮（DiffReviewPanel 的 approve；决策 23：没有「拒绝」）
    const approve = dossier.locator('button.btn.solid', { hasText: '合入' });
    await expect(approve).toBeVisible();
    await approve.click();

    // ── 断言：任务到终态 done（合入真发生） ──
    await waitForTask(app, (t) => t.status === 'done', '任务 done', 120_000);
    await expect(page.locator('.terminal.done')).toBeVisible({ timeout: 60_000 });

    // ── 断言：合入的代码真的符合任务目标（主流程票 01 的核心）──
    // 前两步只证明「流程走完」；这一步证明「目标代码落到了主干上」。
    // 产物分两类：**代码**进 worktree → 合入 main；**设计文档**是任务产物（落任务目录，
    // 不进主干，见 tools.rs 的 task_dir 语义），因此分别取。
    const onMain = (path: string) =>
      execFileSync('git', ['-C', app.repoDir, 'show', `main:${path}`], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
      });

    // ① develop.execute 写的业务代码在 main 上，内容与脚本一致（不是空文件 / 未被覆盖）
    //    —— scripts.ts 的 implementationRounds 写的就是这两处
    const lib = onMain('src/lib.js');
    expect(lib).toContain('a + b');
    expect(lib).not.toContain('not implemented'); // 初始占位已被替换
    // ② 验收测试文件也在
    expect(onMain('tests/acceptance.js')).toContain('add(1, 2)');

    // ③ 设计文档记录了验收标准 AC-1 —— 「任务目标」的锚点，经任务产出文件 API 读取
    //    （与界面上「产出文件」页签同一条路径）
    const designRes = await fetch(`${app.apiBase}/tasks/${app.taskId}/files/design.md`);
    expect(designRes.ok, 'design.md 应可经任务产出文件 API 读取').toBe(true);
    expect(await designRes.text()).toContain('AC-1');

    // ④ 主干确已前进到该任务的提交（合入真发生，而非只改了状态字段）
    const subject = execFileSync('git', ['-C', app.repoDir, 'log', '--format=%s', '-1', 'main'], {
      encoding: 'utf8',
    }).trim();
    expect(subject).toContain('feat: task');
    // ⑤ 任务分支已删除（决策 3：done 清理 worktree 与分支）
    const branches = execFileSync('git', ['-C', app.repoDir, 'branch', '--list', 'kanban/*'], {
      encoding: 'utf8',
    }).trim();
    expect(branches).toBe('');

    // ⑥ 闸门**真的执行了**测试命令（主流程票 02）——「任务到 done」对闸门短路（命令为
    //    `true`）同样会绿，因此必须单独断言系统测试命令确实跑过。
    await expectGateReallyRan(app);
    //    且闸门命令在 fixture 仓库里能独立复现通过：证明合入的代码自身是自洽的，
    //    不是靠闸门空转混过去。
    const gateOut = execFileSync('npm', ['test', '--silent'], {
      cwd: app.repoDir,
      encoding: 'utf8',
    });
    expect(gateOut).toContain('PASS');

    // 终态后顶栏待办计数归零（面板消失由 pending 状态驱动）
    // 像素主题（决策 169）：计数徽章是纯数字且语义为「待处理」；`*` 字面量退役。
    const pendingCount = page.locator('.pending-count');
    await expect(pendingCount).toContainText('待处理', { timeout: 30_000 });
    await expect(pendingCount.locator('.c')).toHaveText('0');
    expectBundleHealthy(bundle);
  });
});
