/**
 * 技能市场的**离线 registry fixture**（E2E ⑬，决策 177 / 187）。
 *
 * 给 `market-install.spec.ts` 起一个真的会应答的 registry：`GET /index.json` 回索引、
 * `GET /skills/{name}-{version}.zip` 回包。摘要由**本模块对包字节现算**，因此索引与内容
 * 天然一致（诚实传输），走的正是生产 `HttpMarketClient` → `parse_index` → 校验 →
 * `SkillPackage::from_zip` → `install` 那条完整通路——端到端只有「来源地址」这一处是替身。
 *
 * ## 为什么不是 skillhub.tencent.com
 *
 * 诉求是「用 skillhub.tencent.com 做技能市场」。**它今天当不了本系统的市场来源**，四道都过不去
 * （2026-09-16 实测，逐条可复现）：
 *
 * 1. **没有索引**。本系统的契约是 `GET {origin}/index.json`（决策 172⑤ 定义的格式，含
 *    `sha256`）；skillhub 的索引在自己的 JSON API 上（`https://api.skillhub.tencent.com/api/v1/search`），
 *    `skillhub.tencent.com/index.json` → **301**（跳 `skillhub.cn`），`skillhub.cn/index.json` →
 *    **200 text/html**（SPA 的兜底页），`api.skillhub.tencent.com/index.json` → **405**。
 *    更要紧的是**它任何地方都不给包的 `sha256`**——而摘要校验（决策 177②的core）正是拿索引里
 *    钉住的摘要去比对下载字节；没有权威摘要，这一步就退化成空操作。
 * 2. **下载地址是明文 http 且跨源**：界面上的「Zip包安装」指向
 *    `http://lightmake.site/api/v1/download?slug=…`，非回环明文 http 被决策 177③ 明确拒绝。
 * 3. **下载靠 302 跳转**：上面那个地址回 302 → `skillhub-…cos.accelerate.myqcloud.com/…zip`，
 *    而决策 177② 显式 `Policy::none()`——不跟随重定向，因为白名单判定看的是**请求** URL。
 * 4. **包布局不同**：skillhub 的包是根级 `SKILL.md` + `_meta.json`，本系统要求单层
 *    `{name}/SKILL.md`（`SkillPackage::from_zip` 的单根校验）。
 *
 * 也就是说，要用真 skillhub 得先给它写一个**适配器**，并修订决策 177 的②③与「先摘要后落盘」——
 * 那是产品与安全口径的变更，不是一条用例能覆盖的事。故本 fixture 是**替身**：它钉的是
 * 「技能市场页能把一个技能装上、预览三项、落到技能根」这条界面通路的**产品行为**，
 * 与来源是谁无关；来源侧的真实性由 `crates/core/tests/market.rs` + `crates/app/tests/api_contract.rs`
 * 的 `FakeMarket` 契约测试承担。
 *
 * ## 为什么是回环 http
 *
 * 决策 177③ 放行回环的明文 http（本机起 registry 做开发与测试），故 fixture 用
 * `http://127.0.0.1:<port>`——它同时也验证了那条放行规则真的生效。
 *
 * ## zip 自己拼（store 方式，不压缩）
 *
 * 不引入 zip 依赖、也不要求系统有 `zip` 命令：技能包是一个几行的 markdown，store 方式够用，
 * 而且字节确定（固定时间戳），摘要可复现。结构：local file header + 数据 + central directory + EOCD。
 */

import { createHash } from 'node:crypto';
import { createServer, type Server } from 'node:http';

/** 索引里的一条技能（`SKILL.md` 的 frontmatter 由本模块按 `name` / `description` 生成）。 */
export interface RegistrySkill {
  name: string;
  version: string;
  description: string;
  /** `SKILL.md` 的正文（frontmatter 之后的部分）。逐行写，行号即可预判。 */
  body: string[];
}

export interface Registry {
  /** registry 的 origin（`http://127.0.0.1:<port>`）——填进「技能市场」页的来源白名单。 */
  origin: string;
  /** 后端打过来的路径流水（断言「搜索真的问了 registry」而不是拿缓存答的）。 */
  requests: string[];
  close(): Promise<void>;
}

// ─────────────────────────────── zip（store） ───────────────────────────────

/** CRC-32（IEEE 802.3）。表只建一次——每个包要用到一次，而表本身与内容无关。 */
const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let i = 0; i < 256; i += 1) {
    let c = i;
    for (let k = 0; k < 8; k += 1) c = (c & 1) !== 0 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[i] = c;
  }
  return table;
})();

function crc32(buf: Buffer): number {
  let c = -1;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff]! ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}

/** 1980-01-01 00:00（DOS 时间戳的最小值）——固定值让同样的输入产出同样的字节。 */
const DOS_TIME = 0;
const DOS_DATE = 0x21;

function zipStore(files: Array<{ name: string; data: Buffer }>): Buffer {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;

  for (const file of files) {
    const nameBytes = Buffer.from(file.name, 'utf8');
    const crc = crc32(file.data);

    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0); // 签名
    local.writeUInt16LE(20, 4); // 解压所需版本（2.0）
    local.writeUInt16LE(0x0800, 6); // 名字是 UTF-8
    local.writeUInt16LE(0, 8); // store（不压缩）
    local.writeUInt16LE(DOS_TIME, 10);
    local.writeUInt16LE(DOS_DATE, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(file.data.length, 18); // 压缩后大小 = 原始大小
    local.writeUInt32LE(file.data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    local.writeUInt16LE(0, 28); // extra 长度
    locals.push(local, nameBytes, file.data);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4); // 制作版本
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt16LE(DOS_TIME, 12);
    central.writeUInt16LE(DOS_DATE, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(file.data.length, 20);
    central.writeUInt32LE(file.data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt16LE(0, 30); // extra
    central.writeUInt16LE(0, 32); // comment
    central.writeUInt16LE(0, 34); // 起始磁盘
    central.writeUInt16LE(0, 36); // 内部属性
    central.writeUInt32LE(0, 38); // 外部属性
    central.writeUInt32LE(offset, 42); // 本地头偏移
    centrals.push(central, nameBytes);

    offset += local.length + nameBytes.length + file.data.length;
  }

  const centralDir = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4); // 本磁盘号
  eocd.writeUInt16LE(0, 6); // 中央目录起始磁盘
  eocd.writeUInt16LE(files.length, 8);
  eocd.writeUInt16LE(files.length, 10);
  eocd.writeUInt32LE(centralDir.length, 12);
  eocd.writeUInt32LE(offset, 16);
  eocd.writeUInt16LE(0, 20); // 注释长度
  return Buffer.concat([...locals, centralDir, eocd]);
}

/** 按本系统的单根布局打一个包：`{name}/SKILL.md` + `{name}/_meta.json`。
 *
 * 带 `_meta.json` 兄弟文件是刻意的：真 skillhub 的包就是 `SKILL.md` + `_meta.json` 两件，
 * 这里只把布局改成本系统要求的单层目录——顺带让 `sibling_count` 不为 0（装完的读数里可见）。 */
function packageOf(skill: RegistrySkill): Buffer {
  const skillMd = [
    '---',
    `name: ${skill.name}`,
    `description: ${skill.description}`,
    '---',
    '',
    ...skill.body,
    '',
  ].join('\n');
  return zipStore([
    { name: `${skill.name}/SKILL.md`, data: Buffer.from(skillMd, 'utf8') },
    {
      name: `${skill.name}/_meta.json`,
      data: Buffer.from(`${JSON.stringify({ version: skill.version }, null, 2)}\n`, 'utf8'),
    },
  ]);
}

// ─────────────────────────────── registry ───────────────────────────────

/**
 * 起一个 registry，返回它的 origin。
 *
 * 索引在 `listen` **之后**才组装：条目的 `source` 与 `url` 都要带上真实端口，而端口是
 * `listen(0)` 之后才知道的（与 harness 回读后端就绪行端口同一姿态）。
 */
export async function startRegistry(skills: RegistrySkill[]): Promise<Registry> {
  const packages = new Map<string, Buffer>();
  for (const skill of skills) {
    packages.set(`/skills/${skill.name}-${skill.version}.zip`, packageOf(skill));
  }

  const requests: string[] = [];
  /** `listen` 之后填：索引里要写进真实 origin。 */
  let indexBody = '{}';

  const server: Server = createServer((req, res) => {
    const url = req.url ?? '/';
    requests.push(url);
    const path = url.split('?')[0] ?? url;

    if (path === '/index.json') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(indexBody);
      return;
    }
    const pkg = packages.get(path);
    if (pkg) {
      res.writeHead(200, { 'content-type': 'application/zip' });
      res.end(pkg);
      return;
    }
    res.writeHead(404, { 'content-type': 'text/plain' });
    res.end('not found');
  });

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : 0;
  const origin = `http://127.0.0.1:${port}`;

  indexBody = JSON.stringify({
    skills: skills.map((skill) => {
      const bytes = packages.get(`/skills/${skill.name}-${skill.version}.zip`)!;
      return {
        name: skill.name,
        version: skill.version,
        // 摘要现算：索引与内容必然一致（诚实传输）。摘要**不符**那条路径不在本用例的射程里
        // ——它是 core 契约测试的活儿（`crates/core/tests/market.rs`）。
        sha256: createHash('sha256').update(bytes).digest('hex'),
        source: origin,
        description: skill.description,
        url: `${origin}/skills/${skill.name}-${skill.version}.zip`,
      };
    }),
  });

  return {
    origin,
    requests,
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
      }),
  };
}
