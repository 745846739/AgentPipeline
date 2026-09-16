/**
 * 页面面板的定位助手（E2E ⑫⑬ 共用）。
 *
 * **为什么按 `<h2>` 标题而不是 `hasText`**：`hasText` 扫的是面板的**全部文字**，而技能列表面板
 * 在没选仓时那句空白文案是「点上面**仓名单**里的「查看技能」」——于是
 * `.filter({ hasText: '仓名单' })` 会同时命中两个面板，Playwright 的 strict mode 当场报
 * 「resolved to 2 elements」（本批实测）。按标题匹配则一一对应，且它正是用户看到的那一行字。
 */

import type { Page } from '@playwright/test';

/** 按标题定位一个 `.panel.blk` 面板（标题允许含动态部分，如「刚装上：grilling」）。 */
export function panel(page: Page, title: string | RegExp) {
  return page
    .locator('.panel.blk')
    .filter({ has: page.getByRole('heading', { name: title, exact: typeof title === 'string' }) });
}
