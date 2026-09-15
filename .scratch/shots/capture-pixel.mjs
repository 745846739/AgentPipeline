// 给像素主题原型逐个视图截图：桌面 7 视图 × 深浅 + 移动 8 视图 × 深浅。
// 与 capture.mjs（终端主题，走 CDP）同一目的，但这里用 Playwright：需要按视图逐个
// 撑高视口再截，避免 position:fixed 的底栏/动作坞被钉在视口中部。
//
// 用法（仓库根）：node .scratch/shots/capture-pixel.mjs [tag 子串]
//   例：node .scratch/shots/capture-pixel.mjs dark-desktop   # 只跑桌面深色
// Playwright 从 frontend/node_modules 解析（前端 devDependency），无需另装。
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..', '..');
const require = createRequire(resolve(repo, 'frontend', 'package.json'));
const { chromium } = require('playwright');

const root = `${repo}/design`;
const out = `${repo}/.scratch/shots`;

const variants = [
  {
    tag: 'pixel-01-dark-desktop',
    url: `file://${root}/prototype-pixel.html`,
    width: 2176,
    views: [
      ['v-board', 'board'],
      ['v-run', 'detail-run'],
      ['v-approve', 'detail-approve'],
      ['v-talk', 'talk'],
      ['v-projects', 'projects'],
      ['v-providers', 'providers'],
      ['v-metrics', 'metrics'],
      ['v-share', 'share'],
    ],
  },
  {
    tag: 'pixel-02-light-desktop',
    url: `file://${root}/prototype-pixel-light.html`,
    width: 2176,
    views: [
      ['v-board', 'board'],
      ['v-run', 'detail-run'],
      ['v-approve', 'detail-approve'],
      ['v-talk', 'talk'],
      ['v-projects', 'projects'],
      ['v-providers', 'providers'],
      ['v-metrics', 'metrics'],
      ['v-share', 'share'],
    ],
  },
  {
    tag: 'pixel-03-dark-mobile',
    url: `file://${root}/prototype-pixel-mobile.html`,
    width: 430,
    views: [
      ['v-board', 'board'],
      ['v-wait', 'wait'],
      ['v-run', 'detail-run'],
      ['v-approve', 'detail-approve'],
      ['v-talk', 'talk'],
      ['v-projects', 'projects'],
      ['v-providers', 'providers'],
      ['v-metrics', 'metrics'],
      ['v-share', 'share'],
    ],
  },
  {
    tag: 'pixel-04-light-mobile',
    url: `file://${root}/prototype-pixel-mobile-light.html`,
    width: 430,
    views: [
      ['v-board', 'board'],
      ['v-wait', 'wait'],
      ['v-run', 'detail-run'],
      ['v-approve', 'detail-approve'],
      ['v-talk', 'talk'],
      ['v-projects', 'projects'],
      ['v-providers', 'providers'],
      ['v-metrics', 'metrics'],
      ['v-share', 'share'],
    ],
  },
];

const only = process.argv[2];
const browser = await chromium.launch();
// 与 capture.mjs 同scale：2x（评审时看清 2px 描边与像素字）
const page = await browser.newPage({ deviceScaleFactor: 2 });
await page.emulateMedia({ reducedMotion: 'reduce' });

const shoot = async (path) => page.screenshot({ path });

for (const v of variants) {
  if (only && !v.tag.includes(only)) continue;
  for (const [viewId, label] of v.views) {
    // 每个视图独立导航：hash 直达。先回 about:blank 强制真实重载——
    // 同一文档只改 hash 不会重新执行脚本，会截到上一个视图。
    await page.setViewportSize({ width: v.width, height: 400 });
    await page.goto('about:blank');
    await page.goto(`${v.url}#${viewId}`, { waitUntil: 'load' });
    await page.waitForTimeout(700);

    const dims = () =>
      page.evaluate(() => ({
        w: document.documentElement.scrollWidth,
        h: document.documentElement.scrollHeight,
      }));

    let d = await dims();
    await page.setViewportSize({ width: v.width, height: d.h });
    await page.waitForTimeout(400);
    const grown = await dims();
    if (grown.h > d.h) {
      await page.setViewportSize({ width: v.width, height: grown.h });
      await page.waitForTimeout(400);
      d = grown;
    }

    const file = `${v.tag}-${label}.png`;
    await shoot(`${out}/${file}`);
    console.log(`${file}: ${d.w}x${d.h}`);
  }
}

await browser.close();
