/**
 * E2E：三个模态框的键盘与语义（票 02）+ 依赖任务 ID 的原生候选项列表（票 05）。
 *
 * 改动前的毛病（审计实测）：点开「新建任务」后按 Escape **关不掉**——Escape 挂在遮罩上，
 * 而焦点还在背后的按钮上；Tab 还能一路跑到弹窗背后的页面；版面上没有可被读屏播报的模态语义。
 * 三个对话框（新建任务 / 拆分任务 / 更换长上下文模型）是同一份复制粘贴的写法，一次改三处
 * （统一走 `components/ui/Modal.svelte`），所以这里也逐个模态框断言**用户做得到的事**：
 * 按 Escape 关得掉、打开后焦点在第一个输入框、Tab 一串不跑到背后、对话框被当作对话框播报。
 *
 * 断言口径（规格 Testing Decisions）：只测用户看得见 / 读屏读得到的东西，不测 class 名、
 * 不测组件内部状态；文案只对契约性字符串（按钮名、标题）做匹配。
 *
 * 怎么把「拆分任务 / 更换长上下文模型」这两个对话框弄到屏上：它们只在
 * `pending(context_overflow)` 这一档由后端下发（`crates/core/src/actions.rs`）。用例播种一个
 * `context_window = 1` 的 provider——压缩后仍超硬限，任务在第一个节点就挂这一档 pending。
 *
 * 只 Chromium（决策 144），真后端 + 内嵌真产物（与 `create-flow.spec.ts` 同一写法）。
 */

import { expect, test, type Page } from '@playwright/test';
import {
  expectBundleHealthy,
  pendingTypeOf,
  settleBundle,
  startApp,
  waitForTaskById,
  watchBundle,
  type App,
  type BundleGuard,
} from './harness';
import { fullPassScript } from './scripts';

/** 写操作要带配对头（`x-agentpipeline`），与 harness 的播种同姿态。 */
async function post<T>(base: string, route: string, body: unknown): Promise<T> {
  const res = await fetch(`${base}${route}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-agentpipeline': '1' },
    body: JSON.stringify(body),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`POST ${route} -> ${res.status}: ${text}`);
  return JSON.parse(text) as T;
}

/** 逐个 Tab / Shift+Tab，断言焦点始终没离开对话框（用户做得到的事：一路填完不用摸鼠标）。 */
async function expectFocusStaysInside(page: Page, dialog: ReturnType<Page['getByRole']>, presses: number) {
  for (const key of ['Tab', 'Shift+Tab'] as const) {
    for (let i = 0; i < presses; i++) {
      await page.keyboard.press(key);
      const inside = await dialog.evaluate((el) => el.contains(document.activeElement));
      expect(inside, `第 ${i + 1} 次 ${key} 之后焦点跑到了对话框外`).toBe(true);
    }
  }
}

/** 焦点从没进过对话框时按 Escape 也要关得掉——改动前正是这一条不成立。 */
async function closeByEscapeWithoutFocus(page: Page) {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
}

test.describe('前端 E2E：模态框的键盘与语义（票 02 / 05）', () => {
  let app: App;
  /** 停在 `pending(context_overflow)` 的任务：拆分任务 / 更换长上下文模型唯一的入口。 */
  let overflowTaskId = '';
  const overflowTitle = '模态框考据任务';

  test.beforeAll(async () => {
    app = await startApp({ script: fullPassScript('MODAL'), seedless: true });

    // 播种三件套（providers / projects / tasks 走 API，用例只考界面行为）。
    // `context_window = 1`：容量小到「压缩后仍超硬限」，任务第一个节点就挂 context_overflow。
    await post(app.apiBase, '/providers', {
      vendor: 'openai',
      model: 'mock',
      context_window: 1,
      base_url: app.mockUrl,
      api_key: 'sk-e2e-modal',
      enabled: true,
    });
    const project = await post<{ project: { id: string } }>(app.apiBase, '/projects', {
      name: 'e2e-modal',
      local_path: app.repoDir,
    });
    const task = await post<{ task: { id: string } }>(app.apiBase, '/tasks', {
      project_id: project.project.id,
      title: overflowTitle,
    });
    overflowTaskId = task.task.id;
    await waitForTaskById(
      overflowTaskId,
      app,
      (t) => pendingTypeOf(t) === 'context_overflow',
      'pending(context_overflow)',
    );
  });

  test.afterAll(async () => {
    await app?.stop();
  });

  /** 进看板（「新建任务」按钮在这里，桌面档）。 */
  async function openBoard(page: Page, bundle: BundleGuard) {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(`${app.webBase}/#/`);
    await settleBundle(page, bundle);
  }

  /** 进那个 pending 任务的详情页，等动作行出现。 */
  async function openDetail(page: Page, bundle: BundleGuard) {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(`${app.webBase}/#/task/${overflowTaskId}`);
    await settleBundle(page, bundle);
    await expect(page.getByRole('button', { name: '拆分任务' }).first()).toBeVisible({
      timeout: 60_000,
    });
  }

  test('① 新建任务：语义 / 焦点进框 / Escape / Tab 不逃逸 / 鼠标仍能关', async ({ page }) => {
    const bundle = watchBundle(page);
    await openBoard(page, bundle);
    const dialog = page.getByRole('dialog');

    await page.getByRole('button', { name: '新建任务' }).click();
    await expect(dialog).toBeVisible();

    // 读屏语义：它被当作一个模态对话框播报，名字就是标题（不是一段无名的排版）
    await expect(dialog).toHaveAttribute('aria-modal', 'true');
    await expect(dialog).toHaveAccessibleName('新建任务');

    // 打开后焦点就在第一个输入框里：不需要先点一下
    await expect(dialog.locator('input').first()).toBeFocused();

    // Escape：焦点不在框内时也关得掉
    await closeByEscapeWithoutFocus(page);

    // Tab / Shift+Tab 在框内循环：不会跑到背后的页面上
    await page.getByRole('button', { name: '新建任务' }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('input').first()).toBeFocused();
    await expectFocusStaysInside(page, dialog, 14);

    // 鼠标路径不变：取消关得掉
    // 收在对话框内 + 精确名：页面上还有别的名字**含**「取消」的钮
    //（状态过滤槽「已结束（失败·取消）」、后端下发的「取消任务」动作），不限定会撞严格模式。
    await dialog.getByRole('button', { name: '取消', exact: true }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);

    // 鼠标路径不变：点遮罩关得掉
    await page.getByRole('button', { name: '新建任务' }).click();
    await expect(dialog).toBeVisible();
    await page.mouse.click(8, 8);
    await expect(page.getByRole('dialog')).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test('② 新建任务：依赖任务 ID 有候选项列表，选了它就真的建出依赖（票 05）', async ({ page }) => {
    const bundle = watchBundle(page);
    await openBoard(page, bundle);
    await page.getByRole('button', { name: '新建任务' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();

    // 输入框上挂着原生候选项列表（`list` 指向一个 datalist），不是另做一套选择器
    const input = dialog.locator('input[list]');
    await expect(input).toBeVisible();
    const listId = await input.getAttribute('list');
    expect(listId, '依赖任务 ID 的输入框没有挂候选项列表').toBeTruthy();
    await expect(dialog.locator(`datalist#${listId}`)).toHaveCount(1);

    // 候选 = 当前项目已有的任务，且能靠标题区分
    const option = dialog.locator(`datalist#${listId} option[value="${overflowTaskId}"]`);
    await expect(option).toHaveCount(1);
    expect(await option.textContent()).toContain(overflowTitle);

    // 从候选里选中它（= 填好该任务的 ID）→ 提交 → 后端建出依赖关系
    await dialog.locator('input').first().fill('E2E 依赖候选任务');
    await input.fill(overflowTaskId);
    await dialog.getByRole('button', { name: '创建并启动' }).click();
    await expect(page).toHaveURL(/#\/task\//);

    const created = page.url().split('/task/')[1] ?? '';
    expect(created).not.toBe('');
    expect(created).not.toBe(overflowTaskId);
    const body = (await (await fetch(`${app.apiBase}/tasks/${created}`)).json()) as {
      depends_on: string[];
    };
    expect(body.depends_on).toContain(overflowTaskId);

    expectBundleHealthy(bundle);
  });

  test('③ 拆分任务：语义 / 焦点进框 / Escape / Tab 不逃逸', async ({ page }) => {
    const bundle = watchBundle(page);
    await openDetail(page, bundle);
    const dialog = page.getByRole('dialog');

    await page.getByRole('button', { name: '拆分任务' }).first().click();
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute('aria-modal', 'true');
    await expect(dialog).toHaveAccessibleName('拆分任务');
    await expect(dialog.locator('textarea').first()).toBeFocused();

    await expectFocusStaysInside(page, dialog, 8);
    await closeByEscapeWithoutFocus(page);

    // 鼠标路径不变：取消关得掉
    await page.getByRole('button', { name: '拆分任务' }).first().click();
    await expect(dialog).toBeVisible();
    // 收在对话框内 + 精确名：页面上还有别的名字**含**「取消」的钮
    //（状态过滤槽「已结束（失败·取消）」、后端下发的「取消任务」动作），不限定会撞严格模式。
    await dialog.getByRole('button', { name: '取消', exact: true }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);

    expectBundleHealthy(bundle);
  });

  test('④ 更换长上下文模型：语义 / 焦点进框 / Escape / Tab 不逃逸', async ({ page }) => {
    const bundle = watchBundle(page);
    await openDetail(page, bundle);
    const dialog = page.getByRole('dialog');

    await page.getByRole('button', { name: '更换长上下文模型' }).first().click();
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute('aria-modal', 'true');
    await expect(dialog).toHaveAccessibleName('更换长上下文模型');
    // 这个框里没有可打字的输入框（只有一个下拉），焦点落在框内第一个控件上
    await expect(dialog.locator('select').first()).toBeFocused();

    await expectFocusStaysInside(page, dialog, 8);
    await closeByEscapeWithoutFocus(page);

    await page.getByRole('button', { name: '更换长上下文模型' }).first().click();
    await expect(dialog).toBeVisible();
    // 收在对话框内 + 精确名：页面上还有别的名字**含**「取消」的钮
    //（状态过滤槽「已结束（失败·取消）」、后端下发的「取消任务」动作），不限定会撞严格模式。
    await dialog.getByRole('button', { name: '取消', exact: true }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);

    expectBundleHealthy(bundle);
  });
});
