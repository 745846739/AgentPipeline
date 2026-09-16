/**
 * 看板几何对齐守护（票 17 / 决策 196 裁决 ⑥）。
 *
 * 脊线坐标与列宽是**手工对齐**的：`SPINE_GEOM` 的 `x = 132 + 264i`、`Board.svelte` 里两段
 * 阵列的宽度、`PipelineRail.svelte` 的链节带长度，全是 264 的整数关系——**改一处不同步改
 * 另一处，今天没有任何测试会报警，只会让脊线静默错位**（决策 196 的起因之一）。
 *
 * 本用例是同一家族的另一条门（与 `theme/css-parity.test.ts` 同形：静态扫描 + 逐值比对，
 * **不新增接缝、不为测试改生产代码形状**）：
 *   ① 列宽从契约读，不重抄 264（含契约 ↔ `app.css` 的 `--rail-col-width` 镜像）；
 *   ② `SPINE_GEOM` 各列 `x === columnWidth/2 + columnWidth × 列号`，列位数 === `columnCount`
 *      （并行两站共用列 x）；
 *   ③ spine 段静态几何满足 `(left − columnWidth/2) % columnWidth === 0` 与
 *      `width % columnWidth === 0`（只扫 spine 变奏；hero / mini 另对齐冻结原型，与列宽无关）；
 *   ④ 主带端点 `left + width === columnWidth/2 + columnWidth × (columnCount − 1)`，
 *      且两段在主带上的**唯一断点**正好落在钉缝（同一把刀切列与脊线）；
 *   ⑤ `Board.svelte` 的 `1400px` === 契约 `GEOMETRY.boardPinBreakpoint`。
 *
 * **一条不作为断言的历史事实**（决策 196 如实记）：列 i 的内容中心（含列间 2px 框）与脊线
 * 站心每列差 2px（`done` 差 16px）——本门**不**把「站心 = 列内容中心」当断言，只保证
 * 「脊线坐标与列宽同源」这条关系。
 */
import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { Stage } from '../api/types';
import { GEOMETRY } from '../theme/contract';
import {
  BOARD_COLUMNS,
  PINNED_COLUMN_COUNT,
  RAIL_STAGES,
  SPINE_COUNT_LABEL,
  boardSegments,
  buildSpineStations,
  columnForStage,
  railGeometry,
  segmentStationX,
  spineBelts,
  spineBeltsAll,
  spineColumnIndex,
  spineColumnX,
  type MiniRailInput,
} from './pipeline';

// vitest 从 `frontend/` 运行（与 css-parity.test.ts 同一取法）。
const srcRoot = resolve(process.cwd(), 'src');
const read = (...parts: string[]) => readFileSync(join(srcRoot, ...parts), 'utf8');
/** 剥注释：注释里会写「站点 x=132+264i」这类说明，不剥会把说明当成实现。 */
const stripComments = (text: string) =>
  text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');

/** 列宽的唯一事实源（本文件其余断言一律由它推导）。 */
const colWidth = GEOMETRY.columnWidth;

describe('看板几何对齐（票 17 / 决策 196）', () => {
  it('① 列宽与列数从契约读：列定义、样式表镜像都是同一个数', () => {
    expect(BOARD_COLUMNS).toHaveLength(GEOMETRY.columnCount);
    // `app.css` 的 --rail-col-width 是契约的手工镜像（css-parity 只比色彩 token），
    // Board.svelte 的宽度 calc 走这个镜像——它漂了，阵列与脊线一起漂。
    const css = stripComments(read('app.css'));
    expect(css.match(/--rail-col-width:\s*(\d+)px/)?.[1]).toBe(String(colWidth));
  });

  it('② 脊线各列站心 = columnWidth/2 + columnWidth × 列号（并行两站共用列 x）', () => {
    const spine = railGeometry('spine');
    const columns = new Set<number>();
    for (const stage of RAIL_STAGES) {
      const col = BOARD_COLUMNS.findIndex((d) => d.key === columnForStage(stage));
      expect(col, `阶段 ${stage} 找不到看板列`).toBeGreaterThanOrEqual(0);
      expect(spine[stage].x, `${stage} 的站心`).toBe(spineColumnX(col));
      // 段内切分（PipelineRail 把站点按段归位）用的就是这条关系，故一并钉住
      expect(spineColumnIndex(spine[stage].x), `${stage} 的列号反查`).toBe(col);
      columns.add(col);
    }
    expect(columns.size).toBe(GEOMETRY.columnCount);
    // 9 站落在 8 列上：并行两站共用一列 x（这是「列位数 === columnCount」的读法）
    expect(Object.keys(spine)).toHaveLength(RAIL_STAGES.length);
  });

  it('③ spine 段的带都是列宽的整数关系，且两段是同一条带（只差一个平移）', () => {
    const { scroll, pinned } = boardSegments();
    /** 正模：段内 left 可为负（段起点之后的带被裁掉），负数的取模得归一化后比。 */
    const norm = (v: number) => ((v % colWidth) + colWidth) % colWidth;
    for (const seg of [scroll, pinned]) {
      const belts = spineBelts(seg);
      expect(belts.length, `段 ${JSON.stringify(seg)} 没有任何带`).toBeGreaterThan(0);
      for (const belt of belts) {
        expect(norm(belt.left - colWidth / 2), `${belt.key} 的 left=${belt.left}`).toBe(0);
        if (belt.kind === 'vt') continue; // 纵向接头宽度由 CSS 定 6px，不参与列宽关系
        expect(norm(belt.width), `${belt.key} 的 width=${belt.width}`).toBe(0);
        expect(belt.width, `${belt.key} 应至少跨一格`).toBeGreaterThan(0);
      }
    }
    // 两段拼接即整条看板：钉右段正好是 merge / done
    expect(scroll.base).toBe(0);
    expect(pinned.span).toBe(PINNED_COLUMN_COUNT);
    expect(pinned.base + pinned.span).toBe(GEOMETRY.columnCount);
    expect(BOARD_COLUMNS.slice(pinned.base).map((d) => d.key)).toEqual(['merge', 'done']);
  });

  it('④ 主带端点在首末列站心；两段的带是同一套几何各自平移到段起点（同一把刀）', () => {
    const main = spineBeltsAll().find((b) => b.kind === 'main');
    if (!main) throw new Error('整条看板的主链节带缺失');
    expect(main.left).toBe(spineColumnX(0));
    expect(main.left + main.width).toBe(spineColumnX(GEOMETRY.columnCount - 1));

    const { scroll, pinned } = boardSegments();
    const backToBoard = (seg: { base: number }, belt: { kind: string; left: number; width: number; top: number }) => ({
      kind: belt.kind,
      left: belt.left + seg.base * colWidth,
      width: belt.width,
      top: belt.top,
    });
    // 「同一把刀」：回到整条看板口径，两段的带逐条相同——所以两段在钉缝处严丝合缝
    expect(spineBelts(pinned).map((b) => backToBoard(pinned, b))).toEqual(
      spineBelts(scroll).map((b) => backToBoard(scroll, b)),
    );
    // 站心按段平移（段内 j 从 0 计）：段首列落在 spineColumnX(0)，反查也回到段首列
    for (const seg of [scroll, pinned]) {
      const firstStage = BOARD_COLUMNS[seg.base].stages[0];
      const firstX = railGeometry('spine')[firstStage].x;
      expect(segmentStationX(firstX, seg.base)).toBe(spineColumnX(0));
      expect(spineColumnIndex(firstX)).toBe(seg.base);
    }
  });

  it('⑤ Board.svelte 的断点字面量与契约相等（CSS 不认自定义属性，靠这条扫）', () => {
    const board = stripComments(read('routes', 'Board.svelte'));
    expect(
      [...board.matchAll(/@media\s*\(min-width:\s*(\d+)px\)/g)].map((m) => Number(m[1])),
    ).toEqual([GEOMETRY.boardPinBreakpoint]);
    // 移动款断点一字不动：479 = 契约 480 的下沿
    expect(
      [...board.matchAll(/@media\s*\(max-width:\s*(\d+)px\)/g)].map((m) => Number(m[1])),
    ).toEqual([GEOMETRY.mobileBreakpoint - 1]);
  });

  it('脊线几何不重抄列宽：pipeline.ts 与 spine 段里都没有裸 264 / 132 字面量', () => {
    // 实现按决策 196 裁决 ⑥ 的推荐做法：`SPINE_GEOM` 的 x 与 spine 段每条带全部由
    // `columnWidth` 推导（评论里的说明不算——注释已剥），故 ③④ 退化成这两条扫描。
    expect(stripComments(read('lib', 'pipeline.ts'))).not.toMatch(/\b(264|132)\b/);

    const rail = stripComments(read('components', 'pipeline', 'PipelineRail.svelte'));
    // 只扫 spine 变奏：hero / mini 另对齐冻结原型，与列宽无关。
    const start = rail.indexOf("{#if variant === 'spine'}");
    const spineBlock = rail.slice(start, rail.indexOf('{:else}', start));
    expect(start).toBeGreaterThanOrEqual(0);
    expect(spineBlock.length).toBeGreaterThan(0);
    expect(spineBlock).not.toMatch(/\b(264|132)\b/);
    expect(
      [...rail.matchAll(/@media\s*\(max-width:\s*(\d+)px\)/g)].map((m) => Number(m[1])),
    ).toEqual([GEOMETRY.mobileBreakpoint - 1]);

    const board = stripComments(read('routes', 'Board.svelte'));
    expect(board).not.toMatch(/\b264\b/);
    // 宽度都按段列数写（改列数只改 pipeline.ts，这条会让另一边的字面量变红）：
    // 阵列 = 整条看板的列数、A 段 = 可横滚段列数、B 段 = 钉住段列数
    for (const span of [GEOMETRY.columnCount, ...Object.values(boardSegments()).map((s) => s.span)]) {
      expect(board, `Board.svelte 缺少按 ${span} 列算的宽度`).toMatch(
        new RegExp(`calc\\(var\\(--rail-col-width\\) \\* ${span}(?!\\d)`),
      );
    }
    // 三个宽度常量也是手工对齐的：横滚内容 = 左内缩 16 + 阵列（左框 2 + 八列 + 右框 2 + 余量 14）
    // + 右内缩 16；阵列 = 左框 2 + 八列 + 18 的框；A 段宽 = 16 + 2 + 六列。
    // 它们与上面「脊线带按段平移 + overflow 裁切」是一套的：改内缩 / 框线必须一起改。
    const { scroll } = boardSegments();
    expect(board).toContain(`calc(var(--rail-col-width) * ${GEOMETRY.columnCount} + 50px)`);
    expect(board).toContain(`calc(var(--rail-col-width) * ${GEOMETRY.columnCount} + 18px)`);
    expect(board).toContain(`calc(var(--rail-col-width) * ${scroll.span} + 18px)`);
  });

  /**
   * `{#each belts as b (b.key)}` 的键必须两两不同。
   *
   * 这条守护是**真实回归**换来的：并行的上下两条 `br` 同 `col=1`，只按 `kind+col` 拼键会撞，
   * Svelte 在浏览器里当场抛 `each_key_duplicate` 并让整块看板不渲染——纯函数的其它断言全绿、
   * jsdom 又不跑真实的 Svelte 运行时，所以只有真应用 e2e 才会红。放在这里，撞键在单测层就断。
   */
  it('段内每条带的 key 两两不同（撞键＝浏览器里 each_key_duplicate）', () => {
    for (const seg of [boardSegments().pinned, boardSegments().scroll]) {
      const keys = spineBelts(seg).map((b) => b.key);
      expect(new Set(keys).size, `段内带键撞了：${JSON.stringify(keys)}`).toBe(keys.length);
    }
    const all = spineBeltsAll().map((b) => b.key);
    expect(new Set(all).size, `整条看板的带键撞了：${JSON.stringify(all)}`).toBe(all.length);
  });
});

/**
 * 脊线数字的口径（票 19 / 决策 197）。
 *
 * 同屏有**两个**长得一样的数字：脊线的是「累计到过这一站」（流量），列头的是
 * 「此刻停在这一列」（存量）——实测能差一倍。纯函数这一层把口径钉住；界面上读不读得出来
 * 由 `e2e/board-overflow.spec.ts` 断言（框里带 `累计` 词）。
 */
describe('脊线数字的口径（票 19 / 决策 197）', () => {
  const at = (stage: Stage, extra: Partial<MiniRailInput> = {}): MiniRailInput => ({
    status: 'running',
    current_stage: stage,
    stalled: false,
    ...extra,
  });

  it('数的是「累计到过这一站」（漏斗），不是「此刻在这一列」（存量）', () => {
    const counts = Object.fromEntries(
      buildSpineStations([at('develop')]).map((s) => [s.stage, s.count]),
    );
    // 一个走到 develop 的任务：走过的四站都算 1（存量口径下它们全是 0）……
    expect(counts.init).toBe(1);
    expect(counts['architect-design']).toBe(1);
    expect(counts.develop).toBe(1);
    // ……还没到的站是 0
    expect(counts.review).toBe(0);
    expect(counts.test).toBe(0);
    expect(counts.merge).toBe(0);
    expect(counts.done).toBe(0);
    // 两个任务 → 累计翻倍（流量能回答「这条链上跑过多少」）
    expect(buildSpineStations([at('develop'), at('review')])[0].count).toBe(2);
  });

  it('并行两个侧站没有数字：整块不渲染，不把「没有」画成 0', () => {
    const stations = buildSpineStations([at('develop-design')]);
    const dev = stations.find((s) => s.stage === 'develop-design');
    const tst = stations.find((s) => s.stage === 'test-design');
    expect(dev?.parallel).toBe('dev');
    expect(tst?.parallel).toBe('test');
    expect(dev?.count).toBeUndefined();
    expect(tst?.count).toBeUndefined();
  });

  it('口径词定稿是 `累计`（脊线数字只准带这一个词）', () => {
    expect(SPINE_COUNT_LABEL).toBe('累计');
    expect(SPINE_COUNT_LABEL).not.toMatch(/\s/);
  });
});
