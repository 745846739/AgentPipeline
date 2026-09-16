/**
 * 「行为 / 规则 → 实现位置」表的悬空引用检查（决策 199 / 票 22）。
 *
 * 索引最大的失败形态不是没建，是建完之后悄悄烂掉——这次的对比度规则就是这么没的（决策 195）。
 * 本用例逐行解析 `design/frontend-design.md` §12.3 的那张表，断言每条「实现位置」在磁盘上存在；
 * **指不到实现位置的条目变红**，失败信息说清「哪一行（行为列的文本）指向哪个不存在的路径」。
 *
 * 格式契约（规格 §12.3 写死，加行必须照此写）：
 *   反引号包裹、顿号分隔多条、路径相对仓库根、可选 `:行号`（行号只作定位、不参与断言）。
 *
 * 边界：纯静态扫描，属既有「静态扫描 + 逐值比对」家族（决策 169 的主题契约那一族），
 * 不新增接缝、不为它改生产代码形状。**跑在 node 环境**（vite.config.ts 默认 jsdom，
 * jsdom 下 `import.meta.url` 是 `http://` 形态、`fileURLToPath` 会抛；本用例不碰 DOM）。
 *
 * @vitest-environment node
 */
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

/** 规格文件（`frontend/src/lib` 上溯三级 = 仓库根）。 */
const SPEC_PATH = fileURLToPath(new URL('../../../design/frontend-design.md', import.meta.url));
const SPEC_LABEL = 'design/frontend-design.md';
/** 仓库根：表里的路径一律相对它解析。 */
const REPO_ROOT = fileURLToPath(new URL('../../../', import.meta.url));

/** 表头（定稿）；解析器只认这一行——改表头等于改契约。 */
const HEADER = '| 行为 / 规则 | 实现位置 | 备注 |';

/**
 * 本 effort 内正在新建、因此允许「此刻还不在磁盘上」的路径（规格 §12.3 第 5 条）。
 * 它们是**已排期的落地物，不是长期豁免**：文件一旦落地就从这里删掉。下面有一条断言逼着这份
 * 名单兑现——名单里的每一条都必须真的被表引用，且名单只准有这五条（用「往名单里加一条」
 * 绕过悬空检查会被挡下）。
 *
 * **已清空（2026-09-16）**：最初的五条（`SettingsLanding.svelte` / `SettingsStages.svelte` /
 * `Modal.svelte` / `contrast.ts` / `copy-discipline.test.ts`）在本轮全部落地，按「落地后逐条删」
 * 清空——现在表里每一条实现位置都必须真实存在于磁盘。
 */
const PENDING: string[] = [];

interface BehaviorRow {
  /** 行为列的文本（失败信息里点名它）。 */
  behavior: string;
  /** 实现位置列里反引号包裹的条目（原样，可能带 `:行号`）。 */
  locations: string[];
  /** 实现位置列的原文（用来查「反引号之外还写了路径」）。 */
  locationsCell: string;
  /** 备注列（编号的归宿）。 */
  note: string;
  /** 规格文件里的行号（1 起），失败信息里定位。 */
  line: number;
}

/** 解析规格文本里的那张表：表头之后到第一个非表行之间全是表行。 */
function parseBehaviorMap(md: string): BehaviorRow[] {
  const lines = md.split('\n');
  const start = lines.findIndex((line) => line.trim() === HEADER);
  if (start < 0) return [];
  const rows: BehaviorRow[] = [];
  for (let i = start + 1; i < lines.length; i++) {
    const line = lines[i];
    if (!line.startsWith('|')) break;
    const cells = line
      .split('|')
      .slice(1, -1)
      .map((cell) => cell.trim());
    // 分隔行（|---|）跳过；列数不对的行留给形状用例报错，不进逐行用例。
    if (cells.every((cell) => /^-+$/.test(cell.replace(/:/g, '')))) continue;
    if (cells.length !== 3) continue;
    rows.push({
      behavior: cells[0],
      locationsCell: cells[1],
      locations: [...cells[1].matchAll(/`([^`]+)`/g)].map((m) => m[1].trim()),
      note: cells[2],
      line: i + 1,
    });
  }
  return rows;
}

/** 路径条目 → 磁盘判据用的相对路径（剥掉可选的 `:行号`）。 */
function pathOf(entry: string): string {
  return entry.replace(/:\d+$/, '');
}

const specExists = existsSync(SPEC_PATH);
const md = specExists ? readFileSync(SPEC_PATH, 'utf8') : '';
const rows = parseBehaviorMap(md);

describe('行为 / 规则 → 实现位置：悬空引用检查', () => {
  it('规格文件与那张表都在（表头逐字为 `| 行为 / 规则 | 实现位置 | 备注 |`）', () => {
    expect(specExists, `读不到 ${SPEC_LABEL}（按 ${SPEC_PATH} 解析）`).toBe(true);
    expect(md.includes(HEADER), `${SPEC_LABEL} 里找不到表头：${HEADER}`).toBe(true);
    expect(rows.length, '表行数少于 20：表被删或被改散了').toBeGreaterThanOrEqual(20);
  });

  it('每一行都有行为、实现位置与备注，且备注承载编号（可追溯性就在这一列）', () => {
    for (const row of rows) {
      expect(row.behavior.length, `${SPEC_LABEL}:${row.line} 行为列为空`).toBeGreaterThan(0);
      expect(
        row.locations.length,
        `${SPEC_LABEL}:${row.line} 行为「${row.behavior}」的实现位置列没有反引号包裹的路径`,
      ).toBeGreaterThan(0);
      expect(row.note.length, `${SPEC_LABEL}:${row.line} 行为「${row.behavior}」的备注列为空`).toBeGreaterThan(0);
      expect(
        /决策\s?\d+|票\s?\d+/.test(row.note),
        `${SPEC_LABEL}:${row.line} 行为「${row.behavior}」的备注列没带编号（决策 N / 票 N）：「${row.note}」`,
      ).toBe(true);
    }
  });

  it('实现位置列的格式合规（相对仓库根、无反引号外的路径、无 glob / 锚点）', () => {
    for (const row of rows) {
      const outside = row.locationsCell.replace(/`[^`]*`/g, '');
      expect(
        /[A-Za-z0-9_-]+\/[A-Za-z0-9_./-]+/.test(outside),
        `${SPEC_LABEL}:${row.line} 行为「${row.behavior}」的实现位置列有反引号之外的路径：「${outside.trim()}」`,
      ).toBe(false);
      for (const entry of row.locations) {
        const why = `${SPEC_LABEL}:${row.line} 行为「${row.behavior}」的实现位置条目不合格式：\`${entry}\``;
        expect(entry.startsWith('/'), `${why}（不得用绝对路径）`).toBe(false);
        expect(/^[A-Za-z0-9._/-]+(:\d+)?$/.test(entry), `${why}（只允许相对路径，可带 :行号）`).toBe(true);
        expect(/[*?#]/.test(entry), `${why}（不得写 glob / query / 锚点）`).toBe(false);
        expect(/\s/.test(entry), `${why}（不得含空白）`).toBe(false);
      }
    }
  });

  it('「允许尚未落地」名单被表真实引用，且规模不超过规格允许的五个', () => {
    const referenced = new Set(rows.flatMap((row) => row.locations.map(pathOf)));
    for (const pending of PENDING) {
      expect(
        referenced.has(pending),
        `名单里的 \`${pending}\` 没有任何表行引用它：落地后请从名单删掉`,
      ).toBe(true);
    }
    expect(PENDING.length, '允许尚未落地的路径上限是 5（规格 §12.3 第 5 条）').toBeLessThanOrEqual(5);
  });

  for (const row of rows) {
    it(`行为「${row.behavior}」的实现位置都存在`, () => {
      const missing: string[] = [];
      for (const entry of row.locations) {
        const rel = pathOf(entry);
        if (PENDING.includes(rel)) continue;
        if (!existsSync(resolve(REPO_ROOT, rel))) missing.push(entry);
      }
      expect(
        missing,
        missing.length > 0
          ? `${SPEC_LABEL}:${row.line} 行为「${row.behavior}」指向不存在的路径：` +
              missing.map((m) => `\`${m}\``).join('、') +
              `（按仓库根 ${REPO_ROOT} 解析）`
          : '',
      ).toEqual([]);
    });
  }
});
