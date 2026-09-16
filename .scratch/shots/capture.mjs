// 给四款终端原型逐个视图截图：桌面 3 视图 / 移动 4 视图。
// 视口高度先按内容撑满再截，避免 position:fixed 的底栏被钉在视口中部。
import { fileURLToPath } from 'node:url';

const port = process.argv[2] || '9222';
// 路径从脚本自身位置推导（脚本在 `<repo>/.scratch/shots/`），不再写死本机绝对路径。
const here = new URL('.', import.meta.url);
const root = fileURLToPath(new URL('../../design/', here));
const out = fileURLToPath(here);

const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = list.find((t) => t.type === 'page');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });

let id = 0;
const pending = new Map();
const events = [];
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    const p = pending.get(m.id); pending.delete(m.id);
    m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result);
  } else events.push(m.method);
};
const send = (method, params = {}) => new Promise((resolve, reject) => {
  pending.set(++id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params }));
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

await send('Page.enable');
await send('Emulation.setEmulatedMedia', {
  features: [{ name: 'prefers-reduced-motion', value: 'reduce' }],
});

const evaluate = async (expression) => {
  const { result, exceptionDetails } = await send('Runtime.evaluate', { expression, returnByValue: true });
  if (exceptionDetails) throw new Error(JSON.stringify(exceptionDetails));
  return JSON.parse(result.value);
};
const metrics = (w, h) => send('Emulation.setDeviceMetricsOverride', {
  width: w, height: h, deviceScaleFactor: 2, mobile: false,
});

const variants = [
  { tag: '01-dark-desktop', url: `file://${root}/prototype-terminal.html`, width: 2176,
    views: [['v-board', 'board'], ['v-run', 'detail-run'], ['v-approve', 'detail-approve']] },
  { tag: '02-light-desktop', url: `file://${root}/prototype-terminal-light.html`, width: 2176,
    views: [['v-board', 'board'], ['v-run', 'detail-run'], ['v-approve', 'detail-approve']] },
  { tag: '03-dark-mobile', url: `file://${root}/prototype-terminal-mobile.html`, width: 430,
    views: [['v-board', 'board'], ['v-wait', 'wait'], ['v-run', 'detail-run'], ['v-approve', 'detail-approve']] },
  { tag: '04-light-mobile', url: `file://${root}/prototype-terminal-mobile-light.html`, width: 430,
    views: [['v-board', 'board'], ['v-wait', 'wait'], ['v-run', 'detail-run'], ['v-approve', 'detail-approve']] },
];

const fs = await import('node:fs');
const shoot = async (path) => {
  const shot = await send('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path, Buffer.from(shot.data, 'base64'));
};

for (const v of variants) {
  events.length = 0;
  await metrics(v.width, 900);
  await send('Page.navigate', { url: v.url });
  while (!events.includes('Page.loadEventFired')) await sleep(50);
  await sleep(500);

  for (const [viewId, label] of v.views) {
    // 先复位视口再测量：撑高过的视口会让 scrollHeight 退化成视口高度。
    await metrics(v.width, 400);
    await evaluate(`(document.querySelector('.demo button[data-v="${viewId}"],.demo-menu button[data-v="${viewId}"]')||{click(){}}).click(),1`);
    await sleep(600);

    let dims = await evaluate(`JSON.stringify({w:document.documentElement.scrollWidth,h:document.documentElement.scrollHeight})`);
    await metrics(v.width, dims.h);
    await sleep(400);
    const grown = await evaluate(`JSON.stringify({w:document.documentElement.scrollWidth,h:document.documentElement.scrollHeight})`);
    if (grown.h > dims.h) { await metrics(v.width, grown.h); await sleep(400); dims = grown; }

    const file = `${v.tag}-${label}.png`;
    await shoot(`${out}/${file}`);
    console.log(`${file}: ${dims.w}x${dims.h}`);
  }
}
ws.close();
