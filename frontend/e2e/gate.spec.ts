/**
 * E2E-③ 闸门真跑 + 闸门失败分流（主流程票 02）。
 *
 * 覆盖用户主流程里必然经过、而此前被短路的环节：
 *   fixture 是无语言标记仓库时闸门命令退化为 `true`，闸门成了空操作。
 *   本文件用**真实 Node 工程**（`npm test`）让闸门真跑，并验证失败后的分流与恢复。
 *
 * 替换边界不变（决策 148）：只换 LLM 响应流，命令 / git / 闸门全部真跑。
 * 页面加载编译期内嵌的真实 bundle（票 01）；只 Chromium（决策 144）。
 */

import { expect, test } from '@playwright/test';

import {
  startApp,
  waitForTask,
  pendingTypeOf,
  watchBundle,
  settleBundle,
  expectBundleHealthy,
  expectGateReallyRan,
  findFailedGateCommand,
  fetchCommands,
  type App,
} from './harness';
import { failingGateRounds } from './scripts';

test.describe('前端 E2E ③：闸门真跑与失败分流', () => {
  let app: App;
  const title = 'E2E gate failure';

  test.beforeAll(async () => {
    app = await startApp({ script: failingGateRounds('E2E'), title });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  test('闸门真跑测试命令；失败被观测到；修好后恢复并推进到合并提案', async ({ page }) => {
    const bundle = watchBundle(page);
    await page.goto(`${app.webBase}/#/task/${app.taskId}`);
    await settleBundle(page, bundle);

    // ── 断言 1：闸门**真的执行了**测试命令（不是 `true` 空转） ──
    //    与「任务到 done」无关——空转时同样会 done，这正是此前被掩盖的点。
    await waitForTask(app, (t) => t.status === 'done' || pendingTypeOf(t) !== null, '首次 pending/完成', 120_000);
    await expectGateReallyRan(app);

    // ── 断言 2：闸门**真失败过**——命令记录里存在退出码非 0 的系统测试命令 ──
    //    脚本第 1 轮让 `add` 返回错值，闸门 `npm test` 必然失败。
    const failed = await findFailedGateCommand(app);
    expect(failed, `应有退出码非 0 的闸门命令；全部命令=${JSON.stringify(
      (await fetchCommands(app)).map((c) => `${c.command}#${c.exit_code}`),
    )}`).not.toBeNull();
    expect(failed!.exit_code).not.toBe(0);

    // ── 断言 3：失败后系统走了恢复路径，最终到达 merge_approval 且闸门转绿 ──
    await waitForTask(
      app,
      (t) => pendingTypeOf(t) === 'merge_approval',
      '闸门恢复后推进到 merge_approval',
      180_000,
    );

    // 恢复后再断言一次：存在**退出码 0** 的闸门命令（修好后的那次真跑）
    const commands = await fetchCommands(app);
    const okGate = commands.find(
      (c) => c.source === 'system' && c.exit_code === 0 && /\bnpm\b|run-tests|node\b/.test(c.command),
    );
    expect(okGate, `修复后闸门应有成功记录；全部=${JSON.stringify(
      commands.map((c) => `${c.command}#${c.exit_code}`),
    )}`).toBeDefined();

    expectBundleHealthy(bundle);
  });
});
