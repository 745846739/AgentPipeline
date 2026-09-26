/**
 * 前端 E2E：值守台账（票 foreman-unbounded 04 / 决策 286）。
 *
 * 值守轮在这之前对前端是隐形的：播报混在人的时间线里（票 01 已在后端分家）。本文件
 * 用**真的值守轮**（serve 的调度循环 + 真待办）取证三件事：
 *
 * ① **入口**：人的对讲台页头有一条「值守台账」的去处，点进去是自己的路由（`/talk/watch`）；
 * ② **只读账**：值守账上没有输入坞、没有急停区、没有值班板——看、翻，不出「说话 / 动手」的口；
 * ③ **转去对话**：播报条目上唯一的一颗钮，把摘录预填进**人的对讲台**的输入坞并聚焦——
 *    「把这件事带进人的时间线」的收口。
 *
 * 真值守轮的编排：`archBlockerRounds()` 让任务停进 pending（当场落一条 `task_pending`
 * 待办，`AttentionKind::wakes()`），值守循环（10s 一趟）过开关门 → 人在跑门 → 退避门 →
 * 去抖（配置压到 1s）→ 播报。播报的回话由 mock 的 foreman 槽按「轮」投喂（与 core
 * 集成测试的 `Script::for_foreman()` 同一形状）；同任务冷却保持默认 30 分钟——用例期间
 * 正好只有第一趟醒来说话，不与后到的待办竞速。
 */

import { expect, test } from '@playwright/test';

import { startApp, settleBundle, watchBundle, expectBundleHealthy, type App } from './harness';
import { archBlockerRounds, foremanScript, text } from './scripts';

/** 播报的内容（mock 投喂给值守轮的那一句；断言按它认行）。 */
const BROADCAST = '「E2E 值守台账」到了等你拍板的一步：需要人确认。';

test.describe('前端 E2E：值守台账的入口与只读账', () => {
  let app: App;

  test.beforeAll(async () => {
    app = await startApp({
      script: {
        ...archBlockerRounds(),
        // 值守轮每醒一次吃一轮；铺三轮给重试留余量
        ...foremanScript(Array.from({ length: 3 }, () => [text(BROADCAST)])),
      },
      pipelineConfig: ['watch_debounce_sec = 1'],
      title: 'E2E 值守台账',
    });
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** 等值守轮真的醒一次：`?kind=watch` 的台账里有值班长的播报行。返回那一账的 id。 */
  async function waitForWatchBroadcast(): Promise<string> {
    const deadline = Date.now() + 120_000;
    while (Date.now() < deadline) {
      const res = await fetch(`${app.apiBase}/foreman/session?kind=watch`, {
        headers: { 'x-agentpipeline': '1' },
      });
      if (res.ok) {
        const payload = (await res.json()) as {
          session: { id: string } | null;
          messages: Array<{ kind: string }> | null;
        };
        if (payload.session && (payload.messages ?? []).some((m) => m.kind === 'fm')) {
          return payload.session.id;
        }
      }
      await new Promise((r) => setTimeout(r, 1_000));
    }
    throw new Error('值守轮 120s 内没有播报（task_pending 待办没被消费？）');
  }

  test('入口 → 只读账 → 转去对话带草稿回人的时间线并聚焦', async ({ page }) => {
    const bundle = watchBundle(page);

    // ── 人的对讲台：页头元信息行有「值守台账」的去处 ──
    await page.goto(`${app.webBase}/#/talk`);
    await settleBundle(page, bundle);
    const entry = page.getByRole('link', { name: '值守台账' });
    await expect(entry).toBeVisible();

    // 值守轮先醒一次（后台等 API，进 UI 之后不再竞速）
    await waitForWatchBroadcast();

    // ── 进值守账：自己的路由，账上是那条播报 ──
    await entry.click();
    await expect(page).toHaveURL(/\/talk\/watch/);
    await expect(page.locator('.timeline .turn.fm', { hasText: BROADCAST }).first()).toBeVisible();

    // ── 只读：没有输入坞、没有急停区、没有值班板；出口是页头那条「回对讲台」 ──
    await expect(page.locator('.typer')).toHaveCount(0);
    await expect(page.locator('.zone-status')).toHaveCount(0);
    await expect(page.locator('.talk-side')).toHaveCount(0);
    await expect(page.getByRole('link', { name: '回对讲台' })).toBeVisible();

    // ── 转去对话：摘录预填进人的输入坞并聚焦 ──
    await page.getByRole('button', { name: '转去对话' }).first().click();
    await expect(page).toHaveURL(/#\/talk(\?|$)/);
    const input = page.locator('.typer textarea');
    // 预填的是**台账那一行的正文**（决策 252：界面不解析正文哨兵）——后端落库的播报行
    // 自带【值守播报】标记，草稿原样带来；人要改的正是这份草稿。
    expect(await input.inputValue()).toContain(BROADCAST);
    await expect(input).toBeFocused();

    expectBundleHealthy(bundle);
  });
});
