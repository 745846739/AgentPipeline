#!/usr/bin/env node
// AgentPipeline 应用图标（像素机房「夜班流水线」，规格 theme-6-pixel.md §2.5）
//
// 单份 32×32 像素母版 → 各尺寸 PNG → iconset → icon.icns。
// 为什么不是直接改 PNG：像素画的可审查形式是坐标表而非位图——与本仓 sprite 表
// （规格 §2.3 的 SPRITES）同一种做法：**改图改这里，重跑生成**。
// 取色一律来自主题 token，图标因此与界面同源，且可被机器核对（--check）。
//
// 无第三方依赖（PNG 编码用 node 内置 zlib），输出确定性：同一份坐标表恒得同一批字节。
//
// 用法：
//   node scripts/make-icon.mjs           生成 crates/desktop/icons/{icon.png,icon.icns}
//                                        与 frontend/public/icons/ 的 Web/PWA 各档（决策 282 ④）
//   node scripts/make-icon.mjs --check   校验取色表与规格 §2.1 逐字一致（不写文件）
//   node scripts/make-icon.mjs --dump    把 16px 真实位图打成字符网格（核像素用）
//   node scripts/make-icon.mjs --preview 写 .scratch/shots/icon-preview.png（放大对照图）
import { deflateSync } from 'node:zlib';
import { writeFileSync, readFileSync, mkdirSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const ICONS_DIR = join(ROOT, 'crates/desktop/icons');

/* ── 取色表：与规格 §2.1 深色「夜班靛」逐字对齐 ──
   新增/改动颜色必须回到规格修订；--check 拿本表与规格正文比对，不一致即报错。 */
const C = {
  bg: '#1B1D2C', // 夜班靛，页面底色 → 图标底板
  panel: '#232639', // 货箱面（界面原色）
  wash: '#2B2F47', // 顶盖带 / 箱面提亮档
  pane: '#3E4363', // 2px 描边 / 顶盖提亮档
  ink: '#12131E', // 硬投影墨色
  paper: '#F1ECDC', // 暖纸白（= --text-hi）
  text2: '#918E9F', // 次文本（= --text-2），货单标签
  go: '#55D97C', // 信号灯绿「执行中」
  belt: '#4E5478', // 传送带已通过段 / 货箱描边（= --belt-lit）
};
const TOKEN_NAME = {
  bg: '--bg', panel: '--panel', wash: '--wash', pane: '--pane', ink: '--ink',
  paper: '--text-hi', text2: '--text-2', go: '--go', belt: '--belt-lit',
};

/* ── 母版：32×32，**所有矩形 (x0,y0,w,h) 皆为偶数** ──
   偶数不是审美偏好而是正确性前提：16px（及各尺寸）由本母版整数倍推导得到，
   21 / 17 这类奇数落在 2×2 箱的中缝上 → 降采样出中间色（既脏又难解释）。
   校验见 assertEven()，违反即报错退出。 */
const N = 32;
/** 底板内缩 2px、台阶圆角半径 6px（均偶数）。圆角比例 6/28 ≈ 21%，参照 macOS 图标格 22.5%。 */
const PLATE = { r: 6, inset: 2 };
const CRATE = { x0: 6, y0: 6, w: 20, h: 16 }; // 20×16 货箱，居中于底板内区（2..29）
/** 6×6 信号灯，**与货箱右沿齐平**（x 20..25 = 箱右沿）且上半悬出箱顶 2px ——
 *  齐平才读作「装在这只箱的右上角」；缩进 2px 会读成浮在箱面里的一块绿。 */
const LAMP = { x0: 20, y0: 4, w: 6, h: 6 };
/** 传送带链节：4px 链 + 4px 隙 ×3 节，总宽 20px 与货箱等宽对齐（前 2 节已通过）。
 *  用 3 节而非 4 节：16px 下 4 节是 4 条 2px 短线，读成噪点；3 节才读成「一段链」。 */
const BELT = { x0: 6, y0: 22, w: 4, gap: 4, n: 3, h: 4, lit: 2 };
const LABEL = { x0: 10, y0: 14, w: 8, h: 2 }; // 货单标签一行（让箱子读作「有单的货箱」而非圆角盒）

/* ── 明度分配：本图标唯一的真难点 ──
   界面的货箱面是 `--panel`，与底板 `--bg` 只差一档明度（对比 1.12:1）——
   32px 下尚可，16px 下整只箱子会糊成底板的一部分。故**整体提亮一档**，
   但严格保住界面「箱沿 > 顶盖 > 箱面」的明度序（界面：pane > wash > panel）：

     界面        frame=pane  lid=wash  face=panel
     图标        frame=belt  lid=pane  face=wash      ← 同序，整体 +1 档

   为什么不用暖纸白做箱面：试过（箱面 paper），16px 下箱面变成一大块白，
   反而把绿灯的对比吃掉了——boldness 只能花在一处，那一处是灯。 */
const FRAME = 'belt';
const LID = 'pane';
const FACE = 'wash';

/** 偶数校验：坐标与尺寸出现奇数，16px 降采样必出中间色。
 *  链节**数量**（n / lit）是计数不是坐标，不参与校验；要核的是它扫出的总宽度。 */
function assertEven() {
  const beltW = BELT.w * BELT.n + BELT.gap * (BELT.n - 1);
  const rects = {
    plate: [PLATE.r, PLATE.inset],
    crate: [CRATE.x0, CRATE.y0, CRATE.w, CRATE.h],
    lamp: [LAMP.x0, LAMP.y0, LAMP.w, LAMP.h],
    belt: [BELT.x0, BELT.y0, BELT.w, BELT.gap, BELT.h, beltW],
    label: [LABEL.x0, LABEL.y0, LABEL.w, LABEL.h],
  };
  for (const [k, nums] of Object.entries(rects))
    for (const n of nums)
      if (n % 2 !== 0) throw new Error(`${k} 含奇数 ${n}：16px 降采样会出中间色`);
  if (beltW !== CRATE.w) throw new Error(`传送带总宽 ${beltW} 与货箱宽 ${CRATE.w} 不等：两者须对齐`);
}

/* ── 极小 RGBA 画布 ── */
const hex = (h) => [parseInt(h.slice(1, 3), 16), parseInt(h.slice(3, 5), 16), parseInt(h.slice(5, 7), 16)];

class Grid {
  constructor(w, h) {
    this.w = w;
    this.h = h;
    this.d = new Uint8Array(w * h * 4);
  }
  /** 源重叠混合：像素画多为实心覆盖，混合是为投影叠在传送带上这类地方留的路 */
  put(x, y, c, a = 255) {
    if (x < 0 || y < 0 || x >= this.w || y >= this.h || a <= 0) return;
    const [r, g, b] = hex(c);
    const i = (y * this.w + x) * 4;
    const sa = a / 255;
    const da = this.d[i + 3] / 255;
    const oa = sa + da * (1 - sa);
    if (oa === 0) return;
    this.d[i] = Math.round((r * sa + this.d[i] * da * (1 - sa)) / oa);
    this.d[i + 1] = Math.round((g * sa + this.d[i + 1] * da * (1 - sa)) / oa);
    this.d[i + 2] = Math.round((b * sa + this.d[i + 2] * da * (1 - sa)) / oa);
    this.d[i + 3] = Math.round(oa * 255);
  }
  /** 闭区间矩形 */
  rect(x0, y0, x1, y1, c) {
    for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) this.put(x, y, c);
  }
}

/** 像素台阶圆角板：四角各自向最近的角心量距，圆内实心、圆外透明 → 阶梯边。
 *  内缩 2px 使图标不顶到画布边，与 Dock 里邻近的原生图标更齐。 */
function plate(g, r, inset) {
  for (let y = inset; y < g.h - inset; y++)
    for (let x = inset; x < g.w - inset; x++) {
      const cx = Math.min(Math.max(x, inset + r), g.w - 1 - inset - r);
      const cy = Math.min(Math.max(y, inset + r), g.h - 1 - inset - r);
      const dx = x - cx;
      const dy = y - cy;
      if (dx * dx + dy * dy <= r * r + 0.5) g.put(x, y, C.bg);
    }
}

/** 母版：一次画完 32×32 的所有层（书写顺序即遮挡关系）。
 *  `fullBleed`（决策 282 ④）：底板不再内缩圆角、夜班靛铺满整幅——maskable 与
 *  apple-touch-icon 的目标会按自己的形状裁切（安卓圆形遮罩 / iOS 圆角），透明四角
 *  会被裁成黑角或白角；满幅底 + 图形全部落在中央 80% 安全区内，两边就都吃得住。
 *  桌面 Dock 仍用内缩圆角的那份（与邻近原生图标对齐是桌面格的事）。 */
function master(fullBleed = false) {
  assertEven();

  const g = new Grid(N, N);
  if (fullBleed) g.rect(0, 0, N - 1, N - 1, C.bg);
  else plate(g, PLATE.r, PLATE.inset);

  // ① 传送带：前 lit 节已通过（亮），其余未点亮
  for (let i = 0; i < BELT.n; i++) {
    const x = BELT.x0 + i * (BELT.w + BELT.gap);
    g.rect(x, BELT.y0, x + BELT.w - 1, BELT.y0 + BELT.h - 1, C[i < BELT.lit ? 'belt' : 'pane']);
  }

  // ② 货箱：2px 箱沿 → 箱面 → 顶盖带
  const c = CRATE;
  const c1 = { x: c.x0 + c.w - 1, y: c.y0 + c.h - 1 };
  g.rect(c.x0, c.y0, c1.x, c1.y, C[FRAME]);
  g.rect(c.x0 + 2, c.y0 + 2, c1.x - 2, c1.y - 2, C[FACE]);
  g.rect(c.x0 + 2, c.y0 + 2, c1.x - 2, c.y0 + 1 + 4, C[LID]); // 顶盖带 4px

  // ③ 货单标签
  g.rect(LABEL.x0, LABEL.y0, LABEL.x0 + LABEL.w - 1, LABEL.y0 + LABEL.h - 1, C.text2);

  // ④ 信号灯：全图唯一的绿，也是最亮点——boldness 花在这一处
  g.rect(LAMP.x0, LAMP.y0, LAMP.x0 + LAMP.w - 1, LAMP.y0 + LAMP.h - 1, C.go);
  return g;
}

/* ── 尺寸派生 ── */
/** 整数最近邻放大（≥32 各档；像素画的脆靠它保住） */
function scaleInt(src, f) {
  const w = src.w * f;
  const h = src.h * f;
  const out = new Grid(w, h);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const si = (((y / f) | 0) * src.w + ((x / f) | 0)) * 4;
      out.d.set(src.d.subarray(si, si + 4), (y * w + x) * 4);
    }
  return out;
}
/** 2×2 箱式降采样（母版 → 16px）。因母版全偶数，箱恒不跨图形边缘，故不产生中间色 */
function half(src) {
  const w = src.w >> 1;
  const h = src.h >> 1;
  const out = new Grid(w, h);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let dy = 0; dy < 2; dy++)
        for (let dx = 0; dx < 2; dx++) {
          const si = ((y * 2 + dy) * src.w + (x * 2 + dx)) * 4;
          const sa = src.d[si + 3] / 255;
          r += src.d[si] * sa;
          g += src.d[si + 1] * sa;
          b += src.d[si + 2] * sa;
          a += sa;
        }
      const di = (y * w + x) * 4;
      if (a > 0) {
        out.d[di] = Math.round(r / a);
        out.d[di + 1] = Math.round(g / a);
        out.d[di + 2] = Math.round(b / a);
        out.d[di + 3] = Math.round((a / 4) * 255);
      }
    }
  return out;
}
/** 各档尺寸（键即像素边长） */
const derive = (m) => ({
  16: half(m), 32: m, 64: scaleInt(m, 2), 128: scaleInt(m, 4),
  256: scaleInt(m, 8), 512: scaleInt(m, 16), 1024: scaleInt(m, 32),
});

/* ── PNG 编码（内置 zlib，无依赖）── */
const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();
const crc32 = (buf) => {
  let c = -1;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
};
function encodePNG(g) {
  const stride = g.w * 4;
  const raw = Buffer.alloc((stride + 1) * g.h);
  for (let y = 0; y < g.h; y++) {
    raw[y * (stride + 1)] = 0; // filter: none
    Buffer.from(g.d.buffer, g.d.byteOffset + y * stride, stride).copy(raw, y * (stride + 1) + 1);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(g.w, 0);
  ihdr.writeUInt32BE(g.h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type: RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/* ── 命令行 ── */
const args = process.argv.slice(2);

if (args.includes('--check')) {
  const spec = readFileSync(join(ROOT, 'design/theme-6-pixel.md'), 'utf8');
  const bad = Object.entries(C).filter(([k, v]) => !spec.includes(`\`${TOKEN_NAME[k]}\` | \`${v}\``));
  if (bad.length) {
    console.error('取色表与规格 §2.1 不一致：', bad.map(([k, v]) => `${TOKEN_NAME[k]}=${v}`).join(' '));
    process.exit(1);
  }
  console.log(`取色表与规格一致（${Object.keys(C).length} 项）`);
  process.exit(0);
}

const m = master();

if (args.includes('--dump')) {
  // 预览图的缩放会骗眼，像素必须逐格核：把 16px 真实位图打出来
  const g16 = half(m);
  const legend = new Map();
  console.log('16px 真实位图（母版 2×2 降采样；此图即 Dock 里所见的像素）');
  for (let y = 0; y < g16.h; y++) {
    let line = '';
    for (let x = 0; x < g16.w; x++) {
      const i = (y * g16.w + x) * 4;
      if (g16.d[i + 3] === 0) line += '.';
      else {
        const k = `${g16.d[i]},${g16.d[i + 1]},${g16.d[i + 2]}`;
        if (!legend.has(k)) legend.set(k, 'abcdefghijklmnop'[legend.size] ?? '?');
        line += legend.get(k);
      }
    }
    console.log(line);
  }
  console.log('\n图例：');
  for (const [k, ch] of legend) {
    const [r, g2, b] = k.split(',').map(Number);
    const hx = '#' + [r, g2, b].map((n) => n.toString(16).padStart(2, '0')).join('');
    const named = Object.entries(C).find(([, val]) => val.toLowerCase() === hx);
    console.log(`  ${ch} = ${hx}${named ? ` (${TOKEN_NAME[named[0]]})` : ' ← 中间色（非 token，说明有矩形落了奇数）'}`);
  }
  process.exit(0);
}

if (args.includes('--preview')) {
  const blit = (dst, src, ox, oy) => {
    for (let y = 0; y < src.h; y++)
      for (let x = 0; x < src.w; x++) {
        const si = (y * src.w + x) * 4;
        dst.put(ox + x, oy + y, '#' + [0, 1, 2].map((k) => src.d[si + k].toString(16).padStart(2, '0')).join(''), src.d[si + 3]);
      }
  };
  const shots = join(ROOT, '.scratch/shots');
  mkdirSync(shots, { recursive: true });
  const sizes = derive(m);
  // 每格 = 某尺寸的最近邻放大（放大后即为眼睛在该尺寸所见）；16/32/256 三档在深浅两底各一行
  const MAG = 128;
  const gap = 14;
  const row = (tiles, bg) => {
    const g = new Grid(gap + tiles.length * (MAG + gap), MAG + gap * 2);
    g.rect(0, 0, g.w - 1, g.h - 1, bg);
    tiles.forEach((t, i) => blit(g, scaleInt(t, Math.max(1, Math.floor(MAG / t.w))), gap + i * (MAG + gap), gap));
    return g;
  };
  const tiles = [sizes[16], sizes[32], sizes[256]];
  const rows = [row(tiles, '#1B1D2C'), row(tiles, '#E8E6DC')];
  const W = Math.max(...rows.map((r) => r.w));
  const H = rows.reduce((a, r) => a + r.h, 0) + gap * (rows.length + 1);
  const out = new Grid(W, H);
  out.rect(0, 0, W - 1, H - 1, '#7A7A7A');
  rows.forEach((r, i) => blit(out, r, 0, gap + i * (r.h + gap)));
  writeFileSync(join(shots, 'icon-preview.png'), encodePNG(out));
  console.log('已写 .scratch/shots/icon-preview.png（列=16/32/256px 的最近邻放大；上夜靛底、下浅底）');
  process.exit(0);
}

/* ── 出图 ── */
const sizes = derive(m);
mkdirSync(ICONS_DIR, { recursive: true });
writeFileSync(join(ICONS_DIR, 'icon.png'), encodePNG(sizes[1024]));

// iconset 是中间产物，落临时目录，不污染仓库（入库的是 .png / .icns 成品）
const set = join(tmpdir(), `AgentPipeline-${process.pid}.iconset`);
rmSync(set, { recursive: true, force: true });
mkdirSync(set, { recursive: true });
const pairs = [
  ['icon_16x16', 16], ['icon_16x16@2x', 32], ['icon_32x32', 32], ['icon_32x32@2x', 64],
  ['icon_128x128', 128], ['icon_128x128@2x', 256], ['icon_256x256', 256], ['icon_256x256@2x', 512],
  ['icon_512x512', 512], ['icon_512x512@2x', 1024],
];
for (const [base, n] of pairs) writeFileSync(join(set, `${base}.png`), encodePNG(sizes[n]));

if (process.platform === 'darwin') {
  execFileSync('iconutil', ['-c', 'icns', '-o', join(ICONS_DIR, 'icon.icns'), set]);
  rmSync(set, { recursive: true, force: true });
  console.log('已写 crates/desktop/icons/{icon.png (1024), icon.icns (10 档)}');
} else {
  console.log(`非 macOS：iconset 留在 ${set}，未生成 .icns`);
}

/* ── Web/PWA 出图（决策 282 ④）：主屏幕图标与桌面壳**同一份母版**，不另造第二套图形 ──
   192 = 32×6、512 = 32×16，全整数倍（非整数的 180 不出档：apple-touch-icon 直接
   用 192 那份，iOS 自己缩）。两个 purpose：
   - any        → 桌面内缩圆角版（桌面 Chrome 铺图标时不裁切，圆角是图形自己的）；
   - maskable   → 满幅底版（安卓圆形/圆角遮罩裁的是它；iOS 的 apple-touch-icon
                  也用它——iOS 不认透明，桌面版的透明四角会露黑）。 */
const WEB_ICONS_DIR = join(ROOT, 'frontend', 'public', 'icons');
mkdirSync(WEB_ICONS_DIR, { recursive: true });
const maskable = master(true);
for (const n of [192, 512]) {
  writeFileSync(join(WEB_ICONS_DIR, `icon-${n}.png`), encodePNG(scaleInt(m, n / 32)));
  writeFileSync(join(WEB_ICONS_DIR, `icon-maskable-${n}.png`), encodePNG(scaleInt(maskable, n / 32)));
}
console.log('已写 frontend/public/icons/{icon-192,icon-512,icon-maskable-192,icon-maskable-512}.png');
