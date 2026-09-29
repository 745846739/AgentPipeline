/**
 * AgentPipeline 的 service worker（spec `.scratch/pwa-webpush/` 票 03）。
 *
 * **只做两件事**：收推送 → 显示通知；点通知 → 开窗 / 聚焦并导航。**零离线缓存**
 * （票面明写：PWA 离线壳是另一个功能的活，顺手加一层 fetch 缓存只会让「刷新后不是
 * 最新的」变成新的神秘故障）。
 *
 * 判据全在 `src/lib/pushPayload.ts` 里（纯函数、被 vitest 覆盖），这里只把它们接到
 * 浏览器事件上——这个文件跑不进 vitest（service worker 全局），所以它必须薄到
 * 「读一眼就知道没有第二处逻辑」。
 *
 * 构建：`vite.config.ts` 把它作为**第二个入口**打进 `dist/sw.js`（不许带 hash），
 * 由 axum 同源托管（`assets.rs`，`Cache-Control: no-cache`——地址固定、内容会变的那
 * 一类必须每次复验，决策 285 同一把尺）。注册在 `main.ts`（唯一的注册点）。
 *
 * 类型：这里**自带一份最小声明**而不是引 `lib.webworker`——后者与 tsconfig 里的
 * `lib.dom` 会撞（两套全局各自声明 `self` / `addEventListener`），而本文件只用得到
 * 五六个成员。`any` 出现在这里是有意的：事件对象的那几个成员没有 DOM 侧对应物。
 */

import { notificationFrom, notificationTarget } from './lib/pushPayload';

/* 事件对象的那几个成员没有 DOM 侧对应物，故这里用 `any` 而不是硬凑一个假形状。 */
/* eslint-disable @typescript-eslint/no-explicit-any */
interface SwEvent {
  waitUntil(promise: Promise<any>): void;
}
interface SwPushEvent extends SwEvent {
  data: { json(): unknown; text(): string } | null;
}
interface SwNotificationEvent extends SwEvent {
  notification: {
    close(): void;
    data: { url?: string } | null;
  };
}
interface SwWindowClient {
  url: string;
  focus(): Promise<SwWindowClient>;
  navigate(url: string): Promise<SwWindowClient>;
}
interface SwSelf {
  location: { origin: string };
  skipWaiting(): Promise<void>;
  addEventListener(type: 'install', handler: () => void): void;
  addEventListener(type: 'activate', handler: (event: SwEvent) => void): void;
  addEventListener(type: 'push', handler: (event: SwPushEvent) => void): void;
  addEventListener(type: 'notificationclick', handler: (event: SwNotificationEvent) => void): void;
  registration: {
    showNotification(title: string, options?: Record<string, unknown>): Promise<void>;
  };
  clients: {
    claim(): Promise<void>;
    matchAll(options: { type: string; includeUncontrolled: boolean }): Promise<SwWindowClient[]>;
    openWindow(url: string): Promise<unknown>;
  };
}

declare const self: SwSelf;

self.addEventListener('install', () => {
  // 立刻接管：本 SW 不缓存任何东西，故没有「等旧版本收工」的必要。
  void self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener('push', (event) => {
  // 解析可能抛（`json()` 对非 JSON 数据）。解析失败退到 `text()`——它几乎不会抛，
  // 而「推送到了却一条通知都不显示」是浏览器要罚的（Chrome 把它当成误用）。
  let raw: unknown = null;
  try {
    raw = event.data ? event.data.json() : null;
  } catch {
    try {
      raw = event.data ? event.data.text() : null;
    } catch {
      raw = null;
    }
  }
  const notice = notificationFrom(raw);
  event.waitUntil(
    self.registration.showNotification(notice.title, {
      body: notice.body,
      // 深链随通知一起挂在 data 上（点击时才用得到）。
      data: { url: notice.url },
      icon: '/icons/icon-192.png',
      badge: '/icons/icon-192.png',
    }),
  );
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const target = notificationTarget(event.notification.data?.url, self.location.origin);
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      // 已经有一个本应用的窗口 → 聚焦它并**导航到现场**（票面：点开直达，不是开新标签）。
      for (const client of windows) {
        if (new URL(client.url).origin !== self.location.origin) continue;
        await client.focus();
        if (client.url !== target) await client.navigate(target);
        return;
      }
      await self.clients.openWindow(target);
    })(),
  );
});
