/**
 * E2E-⑧ 并发第二任务（主流程票 09）。
 *
 * 单任务 happy path 之外，用户最常见的真实用法是**同时跑多个任务**。Rust 层覆盖充分
 * （并行互不阻塞 / 并发准入 / 基准前移），浏览器层此前零覆盖——而并发恰恰是
 * 「状态投影到界面」最容易出错的地方：哪张卡在跑、哪张卡等审批、待办计数是否正确累加、
 * 多游标动作是否落到**正确的分支**（决策 91）。
 *
 * 三条用例：
 *   ① 互不阻塞 + 看板多卡归位 + 双 pending 时的顶栏待办计数；
 *   ② 并行区间内一个任务同时有两个 pending 分支 → 按分支分组渲染 + resume 带对 cursor_id；
 *   ③ 基准前移（决策 96）：另一任务合入使基准前进 → 对停在 merge_approval 的任务点「合入」
 *      → approval 被重置、重走阶段 A，**不得**直接显示 done。
 *
 * 每条用例独占一套 app（同 home 内两个任务，mock 按任务标题路由脚本）。只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';
import { execFileSync } from 'node:child_process';

import {
  expectBundleHealthy,
  pendingTypeOf,
  settleBundle,
  startApp,
  watchBundle,
  type App,
} from './harness';
import { archBlockerRounds, fullPassScript, parallelBlockerRounds, siblingPassScript } from './scripts';

interface CursorView {
  cursor_id: string;
  branch: string;
  stage: string;
  node: string;
  status: string;
}

async function fetchTaskBody(
  app: App,
  taskId: string,
): Promise<{ task: Record<string, unknown>; cursors: CursorView[] }> {
  const res = await fetch(`${app.apiBase}/tasks/${taskId}`);
  if (!res.ok) throw new Error(`GET /tasks/${taskId} -> ${res.status}`);
  return (await res.json()) as { task: Record<string, unknown>; cursors: CursorView[] };
}

/** 读任务产出文件（与界面上「产出文件」页签同一条路径）。 */
async function readTaskFile(app: App, taskId: string, path: string): Promise<string> {
  const res = await fetch(`${app.apiBase}/tasks/${taskId}/files/${path}`);
  if (!res.ok) throw new Error(`GET files/${path} -> ${res.status}`);
  return res.text();
}

/** 提案 diff 里出现变更的**文件头**列表（`diff --git a/<path>`）——不能按裸串判断：
 *  `tests/acceptance.js` 的正文里就有 `require('../src/lib.js')` 这样的字符串。 */
function diffFiles(diff: string): string[] {
  return [...diff.matchAll(/^diff --git a\/(\S+)/gm)].map((m) => m[1]);
}

/** 只取 diff 的**文件头行**（去掉 `+`/`-` 变更正文），用于「某文件不再被改动」这类断言。 */
function diffHeaders(diff: string): string {
  return diff
    .split('\n')
    .filter((line) => /^(diff --git|---|\+\+\+|index |new file|deleted file)/.test(line))
    .join('\n');
}

/** 读 **主干** 上的文件（合入产物在仓库里，不在任务目录，见 happy-path 的同款断言）。 */
function gitShow(app: App, path: string): string {
  return execFileSync('git', ['-C', app.repoDir, 'show', `main:${path}`], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

/** 轮询任务直到满足谓词（多任务场景：显式指定任务 id）。 */
async function waitFor(
  app: App,
  taskId: string,
  predicate: (t: Record<string, unknown>, cursors: CursorView[]) => boolean,
  description: string,
  timeoutMs = 120_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let last = '';
  while (Date.now() < deadline) {
    const { task, cursors } = await fetchTaskBody(app, taskId);
    last = JSON.stringify(task);
    if (predicate(task, cursors)) return;
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`等待「${description}」超时：${last}`);
}

test.describe('前端 E2E ⑧：并发第二任务', () => {
  test('① 双任务互不阻塞：看板两张卡状态独立 + 顶栏待办计数正确累加', async ({ page }) => {
    const bundle = watchBundle(page);
    const jia = '并发甲·入口阻塞';
    const yi = '并发乙·全流程';
    const app = await startApp({
      script: archBlockerRounds(),
      title: jia,
      additionalTasks: [{ title: yi, script: fullPassScript('E2E-乙') }],
    });
    try {
      const [jiaId, yiId] = app.taskIds;
      expect(jiaId && yiId, '应播种出两个任务').toBeTruthy();

      // ── 甲挂在流水线第一步等用户决策；乙照常推进到合并提案（互不阻塞） ──
      await waitFor(
        app,
        jiaId,
        (t) => pendingTypeOf(t) === 'info_insufficient',
        '甲 pending(info_insufficient)',
      );
      await waitFor(
        app,
        yiId,
        (t) => pendingTypeOf(t) === 'merge_approval',
        '乙 pending(merge_approval)',
      );
      // 甲仍停在原处：乙的推进没有把甲从 pending 挤走（决策 82 / 89 的浏览器侧证据）
      const jiaAfter = await fetchTaskBody(app, jiaId);
      expect(pendingTypeOf(jiaAfter.task), '甲应仍停在 info_insufficient').toBe(
        'info_insufficient',
      );

      // ── 看板：两张卡各自归位，状态文案独立正确 ──
      await page.goto(`${app.webBase}/#/`);
      await settleBundle(page, bundle);

      const jiaCard = page.locator('article.card', { hasText: jia });
      const yiCard = page.locator('article.card', { hasText: yi });
      await expect(jiaCard).toBeVisible({ timeout: 60_000 });
      await expect(yiCard).toBeVisible({ timeout: 60_000 });
      await expect(jiaCard.locator('.reason')).toContainText('信息不足');
      await expect(yiCard.locator('.reason')).toContainText('合并提案');
      // 各卡可下发自己的动作（不是只渲染计数）
      await expect(jiaCard.getByRole('button', { name: '补充信息并继续' })).toBeVisible();
      await expect(yiCard.getByRole('button', { name: '合入' })).toBeVisible();

      // ── 顶栏待办计数 = 2（多任务 pending 累加，而非单任务时的 0/1） ──
      // 像素主题（决策 169）：计数徽章是纯数字，`*` 字面量退役；断言同一行为。
      const pendingCount = page.locator('.pending-count');
      await expect(pendingCount).toContainText('待处理', { timeout: 60_000 });
      await expect(pendingCount.locator('.c')).toHaveText('2');

      expectBundleHealthy(bundle);
    } finally {
      await app.stop();
    }
  });

  test('② 并行区间双 pending 分支：按分支分组渲染，resume 带正确的 cursor_id', async ({ page }) => {
    const bundle = watchBundle(page);
    const title = '并发甲·双分支阻塞';
    const app = await startApp({ script: parallelBlockerRounds(), title });
    try {
      const taskId = app.taskId;
      // ── 两个设计分支各自 pending（architect 通过后分裂，决策 90） ──
      await waitFor(
        app,
        taskId,
        (_t, cursors) =>
          cursors.filter((c) => c.status === 'pending').length === 2 &&
          cursors.some((c) => c.branch === 'develop-design') &&
          cursors.some((c) => c.branch === 'test-design'),
        '并行双分支同时 pending',
      );
      const { cursors } = await fetchTaskBody(app, taskId);
      const devCursor = cursors.find((c) => c.branch === 'develop-design')!;
      const testCursor = cursors.find((c) => c.branch === 'test-design')!;

      // ── 看板卡：动作按分支分组，组头是 [dev] / [test]（决策 84：用字标不用色相） ──
      await page.goto(`${app.webBase}/#/`);
      await settleBundle(page, bundle);
      const card = page.locator('article.card', { hasText: title });
      await expect(card).toBeVisible({ timeout: 60_000 });
      await expect(card.locator('.head-label')).toHaveText(['[dev]', '[test]']);
      // 后端下发的动作**总是**带 cursor_id，故决策 91 的「多游标选择器」兜底不渲染——
      // 这是安全方向（用户不可能把分支选错），此处一并钉住。
      await expect(card.locator('.picker')).toHaveCount(0);

      // ── 点 [dev] 组的恢复动作：resume 必须带 develop-design 那条游标的 cursor_id ──
      const devGroup = card.locator('.group').filter({ has: page.locator('.head-label', { hasText: '[dev]' }) });
      const resumeResp = page.waitForResponse(
        (r) => /\/tasks\/[^/]+\/resume$/.test(r.url()) && r.request().method() === 'POST',
      );
      await devGroup.getByRole('button', { name: '跳过当前阶段' }).click();
      const body = (await (await resumeResp).request().postDataJSON()) as { cursor_id?: string };
      expect(body.cursor_id, 'resume 应带被点那组分支的 cursor_id（决策 91）').toBe(
        devCursor.cursor_id,
      );

      // ── 只有被 resume 的分支离开 pending：dev → waiting_join，test 原地不动 ──
      await waitFor(
        app,
        taskId,
        (_t, cs) =>
          cs.find((c) => c.cursor_id === devCursor.cursor_id)?.status === 'waiting_join',
        'develop-design 分支离开 pending',
      );
      const after = await fetchTaskBody(app, taskId);
      expect(
        after.cursors.find((c) => c.cursor_id === testCursor.cursor_id)?.status,
        '另一分支的 pending 不受影响（决策 82）',
      ).toBe('pending');

      expectBundleHealthy(bundle);
    } finally {
      await app.stop();
    }
  });

  test('③ 基准前移：对停在 merge_approval 的任务点「合入」→ approval 重置重走阶段 A，不直接 done', async ({
    page,
  }) => {
    const bundle = watchBundle(page);
    const jia = '并发甲·基准前移';
    const yi = '并发乙·先合入';
    const app = await startApp({
      script: fullPassScript('E2E-甲'),
      title: jia,
      additionalTasks: [{ title: yi, script: siblingPassScript('E2E-乙') }],
    });
    try {
      const [jiaId, yiId] = app.taskIds;
      await waitFor(app, jiaId, (t) => pendingTypeOf(t) === 'merge_approval', '甲 merge_approval');
      await waitFor(app, yiId, (t) => pendingTypeOf(t) === 'merge_approval', '乙 merge_approval');

      // 甲此刻的提案 diff 覆盖它自己的两处变更（稍后据此断言「重走阶段 A」）
      const diffBefore = await readTaskFile(app, jiaId, 'merge-proposal.diff');
      expect(diffFiles(diffBefore).sort()).toEqual(['src/lib.js', 'tests/acceptance.js']);

      // ── 乙先合入：基准前移 ──
      await page.goto(`${app.webBase}/#/task/${yiId}`);
      await settleBundle(page, bundle);
      const yiDossier = page.locator('aside.dossier');
      await expect(yiDossier.locator('.dtag')).toContainText('合并提案', { timeout: 60_000 });
      await yiDossier.getByRole('button', { name: /合入/ }).click();
      await waitFor(app, yiId, (t) => t.status === 'done', '乙合入到 done');

      // ── 对甲点「合入」：基准已前移，approval 必须被重置并重走阶段 A（决策 96） ──
      await page.goto(`${app.webBase}/#/task/${jiaId}`);
      await settleBundle(page, bundle);
      const jiaDossier = page.locator('aside.dossier');
      await expect(jiaDossier.locator('.dtag')).toContainText('合并提案', { timeout: 60_000 });
      const approveResp = page.waitForResponse(
        (r) => r.url().endsWith('/merge/decision') && r.request().method() === 'POST',
      );
      await jiaDossier.getByRole('button', { name: /合入/ }).click();
      expect((await approveResp).status()).toBe(200);

      // ── 收敛判定：done（缺陷）或「重走阶段 A 后再次等审批」（决策 96 的正确行为） ──
      //
      // 为什么不用「等到 pending(merge_approval) 就断言」：点「合入」前它**就是**
      // merge_approval，那种等待会在旧状态上立刻返回、把 bug 放行成绿。这里等到
      // done 或「pending 且提案 diff 已按新基准重算」这个**终局**信号再裁定。
      const deadline = Date.now() + 120_000;
      let observed = { status: '', pending: null as string | null, diff: '' };
      while (Date.now() < deadline) {
        const body = await fetchTaskBody(app, jiaId);
        const diff = await readTaskFile(app, jiaId, 'merge-proposal.diff');
        observed = { status: String(body.task.status), pending: pendingTypeOf(body.task), diff };
        if (observed.status === 'done') break;
        // 终局信号：重算后的提案里不再有 src/lib.js 这个**变更文件**
        if (observed.pending === 'merge_approval' && !diffHeaders(diff).includes('src/lib.js')) break;
        await new Promise((r) => setTimeout(r, 300));
      }
      expect(
        observed.status,
        'approval 被重置后应回到阶段 A 等二次审批，界面不得直接显示 done（决策 96）',
      ).toBe('pending');
      expect(observed.pending).toBe('merge_approval');
      // 「重走阶段 A」的硬证据：rebase 到新基准后提案收窄——自己独有的变更还在，
      // 已在基准里的 src/lib.js **不再被改动**（approval 未被重置时读到的是旧基准下的
      // 提案，这条必红）。按文件头判断，避免撞上 acceptance.js 正文里的 require 字符串。
      expect(diffFiles(observed.diff), '提案应基于新基准重算').toEqual(['tests/acceptance.js']);

      // ── 二次审批：这次基准一致 → 合入 → done ──
      await page.reload();
      await settleBundle(page, bundle);
      const jiaDossier2 = page.locator('aside.dossier');
      await expect(jiaDossier2.locator('.dtag')).toContainText('合并提案', { timeout: 60_000 });
      await jiaDossier2.getByRole('button', { name: /合入/ }).click();
      await waitFor(app, jiaId, (t) => t.status === 'done', '甲合入到 done');

      // 两个任务的产物都在主干上（合入真发生，而非只改了状态字段）
      expect(gitShow(app, 'src/extra.js')).toContain('sibling');
      expect(gitShow(app, 'tests/acceptance.js')).toContain('add(1, 2)');

      expectBundleHealthy(bundle);
    } finally {
      await app.stop();
    }
  });
});
