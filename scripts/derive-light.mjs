// 由深色款生成浅色款：像素主题的浅色变体只差 token 块、头部注释、wordmark 投影、
// 工头头像肤色四处（规格 theme-6-pixel.md §2.4）。手抄两份文件必然漂移，故用同一份
// 变换脚本生成；正确性由「用 git 里的旧深色款跑一遍，能否逐字节还原旧浅色款」证明。
//
// 用法：node derive-light.mjs <dark.html> <out.html> <desktop|mobile>
import { readFileSync, writeFileSync } from 'node:fs';

const [, , darkPath, outPath, kind] = process.argv;
if (!darkPath || !outPath || !['desktop', 'mobile'].includes(kind)) {
  console.error('用法: node derive-light.mjs <dark.html> <out.html> <desktop|mobile>');
  process.exit(1);
}

let html = readFileSync(darkPath, 'utf8');

/** 每处替换都必须命中，否则宁可报错也不要产出半成品。 */
function sub(from, to, label) {
  if (!html.includes(from)) throw new Error(`未命中：${label}`);
  html = html.replace(from, to);
}

/* ── ① 头部：color-scheme / 标题 / 注释 ── */
if (kind === 'desktop') {
  sub('<meta name="color-scheme" content="dark">', '<meta name="color-scheme" content="light">', 'color-scheme');
  sub(
    '<title>AgentPipeline · 看板原型 · 像素机房「夜班流水线」</title>',
    '<title>AgentPipeline · 看板原型 · 像素机房「掌机背光」浅色</title>',
    'title',
  );
  sub(
    '<!-- 主题六「像素机房 · 夜班流水线」提案：与 prototype-terminal.html 同一套页面元素与内容。',
    '<!-- 主题六「像素机房」浅色款「掌机背光」：与 prototype-pixel.html 同一套页面元素与内容，仅换 token（规格 theme-6-pixel.md §2.4）。',
    '注释',
  );
} else {
  sub('<meta name="color-scheme" content="dark">', '<meta name="color-scheme" content="light">', 'color-scheme');
  sub(
    '<title>AgentPipeline · 移动原型 · 像素机房「夜班流水线」</title>',
    '<title>AgentPipeline · 移动原型 · 像素机房「掌机背光」浅色</title>',
    'title',
  );
  sub(
    '<!-- 主题六「像素机房 · 夜班流水线」移动版（深色款）：与 prototype-pixel.html 同一套 token 与内容',
    '<!-- 主题六「像素机房」移动版浅色款「掌机背光」：与 prototype-pixel-mobile.html 同一套 token 与内容',
    '注释',
  );
}

/* ── ② 色彩 token（theme-6-pixel.md §2.4） ── */
const DARK_TOKENS = `  --bg:#1B1D2C; --panel:#232639; --wash:#2B2F47; --pane:#3E4363; --ink:#12131E;
  --t-hi:#F1ECDC; --t1:#C7C3B4; --t2:#918E9F; --t3:#6E6C82; --t4:#55536B;
  --go:#55D97C; --go-hi:#6FE693; --go-ink:#0B2314;
  --pending:#FFB545; --stop:#FF6157; --done:#6E6C82;
  --dev:#59A7FF; --tst:#C08BFF;
  --diff-a:#57D97C; --diff-a-bg:#16301F; --diff-d:#FF7B6E; --diff-d-bg:#361A20;
  --belt-lit:#4E5478;`;
const LIGHT_TOKENS = `    --bg:#E8E6DC; --panel:#F6F4EA; --wash:#DEDBCE; --pane:#9A97A8; --ink:#2A2B3A;
  --t-hi:#14151F; --t1:#2F3040; --t2:#55566A; --t3:#7A7B8E; --t4:#9A9BAC;
  --go:#1F7A3C; --go-hi:#175F2F; --go-ink:#F6F4EA;
  --pending:#8F5B00; --stop:#B3271E; --done:#7A7B8E;
  --dev:#1D5FA8; --tst:#6F3BB8;
  --diff-a:#1D6B3C; --diff-a-bg:#DDEBDD; --diff-d:#A02C22; --diff-d-bg:#F4DEDA;
  --belt-lit:#7E8094;`;
sub(DARK_TOKENS, LIGHT_TOKENS, 'token 块');

/* 移动款追加 --hairline（浅色下弱分隔不能靠透明度） */
if (kind === 'mobile') {
  sub(
    '  --belt-lit:#7E8094;\n  --px:',
    '  --belt-lit:#7E8094;\n  --hairline:#D8D5C8;\n  --px:',
    '移动 --hairline',
  );
}

/* ── ③ wordmark 去投影（浅底高对比下 24px 紧排像素字会成重影） ── */
if (kind === 'desktop') {
  sub('\n  text-shadow:3px 3px 0 var(--ink)}', '\n}', 'wordmark 投影');
} else {
  sub('\n  text-shadow:2px 2px 0 var(--ink)}', '\n}', 'wordmark 投影');
}

/* ── ④ 工头头像肤色固定（浅色下 var(--t-hi) 变墨块） ── */
sub(
  `'<rect x="4" y="4" width="8" height="5" fill="var(--t-hi)"/>'`,
  `'<rect x="4" y="4" width="8" height="5" fill="#E3C7A6"/>'`,
  '工头肤色',
);

writeFileSync(outPath, html);
console.log(`已生成 ${outPath}`);
