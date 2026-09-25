import type { BranchCursor, Stage, TaskStatus } from '../api/types';
import { GEOMETRY, STATE_STYLES, type CrateState, type SpriteName } from '../theme/contract';

/**
 * 流水线静态拓扑（决策 107：sync-check 全站不展示）。
 *
 * 10 阶段去掉 sync-check = 9 站；并行区间 develop-design ∥ test-design 直接分岔，
 * 在 develop 站前合流，不设汇合站点。汇合由双轨直接表达的语义在此收敛。
 */

/** 迷你轨 / hero 的 9 个刻度（决策 107）。 */
export const RAIL_STAGES: Stage[] = [
  'init',
  'architect-design',
  'develop-design',
  'test-design',
  'develop',
  'review',
  'test',
  'merge',
  'done',
];

const STAGE_INDEX: Record<Stage, number> = {
  init: 0,
  'architect-design': 1,
  'develop-design': 2,
  'test-design': 3,
  'sync-check': 3, // 不展示；投影到双轨合流边界（任务永不"停在"这里）
  develop: 4,
  review: 5,
  test: 6,
  merge: 7,
  done: 8,
};

export function railIndex(stage: Stage): number {
  return STAGE_INDEX[stage];
}

/* ─────────────────────────────── 看板列 ─────────────────────────────── */

export type ColumnKey =
  | 'init'
  | 'architect-design'
  | 'design'
  | 'develop'
  | 'review'
  | 'test'
  | 'merge'
  | 'done';

export interface BoardColumnDef {
  key: ColumnKey;
  label: string;
  /** 归属该列的阶段（design 列 = 双轨合并列，决策 92）。 */
  stages: Stage[];
}

/** 8 列（决策 92 的"独立槽位"）。 */
export const BOARD_COLUMNS: BoardColumnDef[] = [
  { key: 'init', label: 'init', stages: ['init'] },
  { key: 'architect-design', label: 'architect-design', stages: ['architect-design'] },
  { key: 'design', label: 'develop-design ∥ test-design', stages: ['develop-design', 'test-design'] },
  { key: 'develop', label: 'develop', stages: ['develop'] },
  { key: 'review', label: 'review', stages: ['review'] },
  { key: 'test', label: 'test', stages: ['test'] },
  { key: 'merge', label: 'merge', stages: ['merge'] },
  { key: 'done', label: 'done', stages: ['done'] },
];

export function columnForStage(stage: Stage): ColumnKey {
  switch (stage) {
    case 'init':
      return 'init';
    case 'architect-design':
      return 'architect-design';
    case 'develop-design':
    case 'test-design':
    case 'sync-check':
      return 'design';
    case 'develop':
      return 'develop';
    case 'review':
      return 'review';
    case 'test':
      return 'test';
    case 'merge':
      return 'merge';
    case 'done':
      return 'done';
  }
}

/** 终态任务停留在其终态游标所在的列（决策 131）；done 恒在 done 列。 */
export function columnForTask(task: { status: TaskStatus; current_stage: Stage }): ColumnKey {
  return columnForStage(task.current_stage);
}

/* ─────────────────────────────── 分支语义 ─────────────────────────────── */

export type BranchKind = 'main' | 'dev' | 'test';

export function branchKind(branch: string): BranchKind {
  if (branch === 'develop-design') return 'dev';
  if (branch === 'test-design') return 'test';
  return 'main';
}

export function branchShort(branch: string): string {
  const kind = branchKind(branch);
  return kind === 'main' ? 'main' : kind;
}

/* ───────────────────── 像素主题：货箱 / 工位（决策 169） ───────────────────── */

/** 货箱状态 → 契约 `STATE_STYLES` 的键（灯 / 描边 / 小人节奏的唯一入口）。 */
export function crateState(task: { status: TaskStatus; stalled?: boolean }): CrateState {
  switch (task.status) {
    case 'running':
      return 'running';
    case 'pending':
      return 'pending';
    case 'failed':
    case 'cancelled':
      return 'failed';
    case 'done':
      return 'done';
    case 'queued':
      return 'queued';
    default:
      return 'waiting';
  }
}

/**
 * 列 → 工位 sprite（theme-6-pixel.md §3 与冻结原型列头逐列一致）。
 * 图元只来自契约 sprite 表；改这里等于改规格，须回 theme-6 修订。
 */
export const COLUMN_SPRITES: Record<ColumnKey, SpriteName> = {
  init: 'flag',
  'architect-design': 'gem',
  design: 'hammer',
  develop: 'gear',
  review: 'lens',
  test: 'shield',
  merge: 'merge',
  done: 'trophy',
};

/** 列状态 → 小节奏（run 快挥 / wait 慢挥 / idle 站立）。 */
export function workerRhythm(state: StationState): 'run' | 'wait' | 'idle' {
  // `StationState` 是轨道站点的词表（比货箱状态多 done/stop/dev/test），先归一到契约的
  // `CrateState`，再读 `STATE_STYLES.worker`——节奏的**定义**在契约里，这里只做词表转换，
  // 不在生产侧再抄一份映射（否则「唯一事实源」只是名义上的）。
  const normalized: CrateState =
    state === 'warn'
      ? 'pending'
      : state === 'go' || state === 'dev' || state === 'test'
        ? 'running'
        : state === 'stop'
          ? 'failed'
          : state === 'done'
            ? 'done'
            : 'queued';
  return STATE_STYLES[normalized].worker;
}

/** 货箱状态 → 量表 tone（四盏信号灯语义，不引第二套色）。 */
/** 货箱状态 → 量表 tone（四盏信号灯语义，不引第二套色）。灯色由契约给出。 */
export function crateTone(state: CrateState): 'go' | 'warn' | 'stop' | 'dim' {
  switch (STATE_STYLES[state].lamp) {
    case '--go':
      return 'go';
    case '--pending':
      return 'warn';
    case '--stop':
      return 'stop';
    default:
      return 'dim';
  }
}

/* ─────────────────────────────── 迷你轨 ─────────────────────────────── */

export type MiniDotState =
  | 'idle'
  | 'past'
  | 'cur'
  | 'cur-warn'
  | 'cur-stop'
  | 'dev'
  | 'tst'
  | 'done';

export interface MiniRailInput {
  status: TaskStatus;
  current_stage: Stage;
  stalled: boolean;
  branches?: BranchCursor[];
}

/** 9 刻度状态（卡片身份特征，替代进度条，§5.2）。 */
export function miniRailState(task: MiniRailInput): MiniDotState[] {
  const dots: MiniDotState[] = new Array(RAIL_STAGES.length).fill('idle');

  if (task.status === 'done') return dots.fill('done');

  const idx = railIndex(task.current_stage);

  // 并行区间：按分支游标独立着色（决策 84）。
  const parallel = (task.branches ?? []).filter(
    (b) => b.status !== 'archived' && (b.branch === 'develop-design' || b.branch === 'test-design'),
  );
  if (parallel.length > 0) {
    for (const b of parallel) {
      const i = railIndex(b.stage);
      const slot = b.branch === 'develop-design' ? 2 : 3;
      if (b.status === 'pending') dots[slot] = 'cur-warn';
      else if (b.status === 'waiting_join' || b.status === 'archived') dots[slot] = 'past';
      else dots[slot] = b.branch === 'develop-design' ? 'dev' : 'tst';
      void i;
    }
  }

  for (let i = 0; i < idx; i++) {
    if (dots[i] === 'idle') dots[i] = 'past';
  }

  if (idx >= 0) {
    const terminal = task.status === 'failed' || task.status === 'cancelled';
    if (terminal) {
      if (dots[idx] === 'idle') dots[idx] = 'cur-stop';
    } else if (task.stalled || task.status === 'pending') {
      if (dots[idx] === 'idle') dots[idx] = 'cur-warn';
    } else if (dots[idx] === 'idle') {
      dots[idx] = 'cur';
    }
  }

  // 有游标置于当前阶段但 miniRail 未点亮时兜底
  if (task.status === 'running' && dots[idx] === 'idle') dots[idx] = 'cur';
  return dots;
}

/* ─────────────────────────────── 字符迷你轨 ─────────────────────────────── */

/**
 * 迷你轨刻度记号（像素主题：灯 / 方块取色用，§3 共享元素映射）。
 * 结构靠字符，不靠盒子：f 未到 / p 已过 / d 完成 / c 当前 / v 当前分叉
 * / w 等待 / x 失败 / t test 未决。
 */
export type RailToken = 'f' | 'p' | 'd' | 'c' | 'v' | 'w' | 'x' | 't';

const DOT_TO_TOKEN: Record<MiniDotState, RailToken> = {
  idle: 'f',
  past: 'p',
  done: 'd',
  cur: 'c',
  'cur-warn': 'w',
  'cur-stop': 'x',
  dev: 'v',
  tst: 't',
};

export function railTokens(dots: MiniDotState[]): RailToken[] {
  return dots.map((d) => DOT_TO_TOKEN[d]);
}

/** 该记号之后连接段点亮（已过 / 完成 / 当前分叉）。 */
export function tokenLitsSegment(token: RailToken): boolean {
  return token === 'p' || token === 'd' || token === 'v';
}

/** 该任务是否在并行区间（决定卡片渲染双药丸）。 */
export function isInParallel(task: MiniRailInput): boolean {
  if (task.current_stage === 'develop-design' || task.current_stage === 'test-design') return true;
  return (task.branches ?? []).some(
    (b) =>
      b.status !== 'archived' && (b.branch === 'develop-design' || b.branch === 'test-design'),
  );
}

/* ─────────────────────────────── 轨道站点视图 ─────────────────────────────── */

export type StationState = 'done' | 'go' | 'warn' | 'stop' | 'idle' | 'dev' | 'test';

export interface StationView {
  key: string;
  stage: Stage;
  label: string;
  x: number;
  y: number;
  state: StationState;
  count?: number;
  /** 并行双轨站点。 */
  parallel?: 'dev' | 'test';
}

interface Geometry {
  x: number;
  y: number;
}

/**
 * 脊线站点坐标：列宽 264px、列间共享 1px 框线，站点落在列中心
 * （列 i 中心 = columnWidth/2 + columnWidth × i；并行区间是一个列两个侧站，共用同一列 x）。
 *
 * x 一律由契约列宽推导（决策 196 裁决 ⑥ 的推荐做法）——**不重抄 264 / 132**，
 * 于是「改列宽忘了改坐标」这种静默错位在源码层面就不可能发生；y 与列宽无关，照旧写字面值。
 * 这条关系与链节带的列宽整数关系由 `pipeline.geometry.test.ts` 逐值守护。
 */
const SPINE_GEOM: Record<string, Geometry> = {
  init: { x: spineColumnX(0), y: 62 },
  'architect-design': { x: spineColumnX(1), y: 62 },
  'develop-design': { x: spineColumnX(2), y: 22 },
  'test-design': { x: spineColumnX(2), y: 50 },
  develop: { x: spineColumnX(3), y: 62 },
  review: { x: spineColumnX(4), y: 62 },
  test: { x: spineColumnX(5), y: 62 },
  merge: { x: spineColumnX(6), y: 62 },
  done: { x: spineColumnX(7), y: 62 },
};

/**
 * hero 坐标：逐字对齐冻结原型 `design/prototype-pixel.html` 的 `#v-run .hrail`
 * （9 站，左边距 56px 起、站距约 106px，末站 done=798）。详情内容区 max-width 1000
 * 含 20px 内边距 → 可用 960px；末站标签（约 28px 宽）止于 ~812，不出容器。
 * 主站灯排在顶部一行（x 有效、y 仅对并行侧站有意义），主带 y=76；
 * 并行双带 y=62 / y=90（原型 twin belts），侧站 y 即两条分带中线。
 */
const HERO_GEOM: Record<string, Geometry> = {
  init: { x: 56, y: 62 },
  'architect-design': { x: 162, y: 62 },
  'develop-design': { x: 272, y: 62 },
  'test-design': { x: 272, y: 90 },
  develop: { x: 374, y: 62 },
  review: { x: 480, y: 62 },
  test: { x: 586, y: 62 },
  merge: { x: 692, y: 62 },
  done: { x: 798, y: 62 },
};

export const RAIL_LABELS: Record<string, string> = {
  init: 'init',
  'architect-design': 'architect',
  'develop-design': 'dev-design',
  'test-design': 'test-design',
  develop: 'develop',
  review: 'review',
  test: 'test',
  merge: 'merge',
  done: 'done',
};

export function railGeometry(variant: 'spine' | 'hero'): Record<string, Geometry> {
  return variant === 'hero' ? HERO_GEOM : SPINE_GEOM;
}

/* ───────────────── 看板溢出：两段与脊线几何（决策 196） ───────────────── */

/**
 * 钉在右侧的列数（`merge` / `done`，决策 196）。
 *
 * 钉住区宽 = `columnWidth × 本值`（列是 264px 边框盒，两列正好 528px），
 * 钉缝是钉住区左缘的 2px `--pane` 竖线——即下面 `pinned` 段的 `base` 处。
 */
export const PINNED_COLUMN_COUNT = 2;

/** 看板横滚内容里的一段：`base` 是起始列（整条看板口径），`span` 是列数。 */
export interface SpineSegment {
  base: number;
  span: number;
}

/** 看板的两段：可横滚的六列 + 钉在右侧的两列。列数从 `BOARD_COLUMNS` 推导，不重抄。 */
export function boardSegments(): { scroll: SpineSegment; pinned: SpineSegment } {
  const base = BOARD_COLUMNS.length - PINNED_COLUMN_COUNT;
  return { scroll: { base: 0, span: base }, pinned: { base, span: PINNED_COLUMN_COUNT } };
}

/**
 * 列 i 的站心 x（**段内** i 从 0 计）：`columnWidth/2 + columnWidth × i`。
 *
 * 这是决策 196 定的口径：两段各自从 0 计，故脊线与列在同一把刀下断开而不错位。
 */
export function spineColumnX(i: number): number {
  return GEOMETRY.columnWidth / 2 + GEOMETRY.columnWidth * i;
}

/**
 * 由站心 x 反查它落在第几列。
 *
 * 成立的前提是脊线坐标与列宽同源（`SPINE_GEOM` 逐列满足 `132 + 264i`），
 * 这条关系由 `pipeline.geometry.test.ts` 守护——不是本函数自己保证的。
 */
export function spineColumnIndex(x: number): number {
  return Math.round((x - GEOMETRY.columnWidth / 2) / GEOMETRY.columnWidth);
}

/** 链节带 / 回流带的静态布局，**以「列」为单位**（px 一律由契约列宽推导，不重抄 264）。 */
interface SpineBeltSpec {
  /** 起点列（0 = 第 1 列站心）。 */
  col: number;
  /** 跨列数（1 = 相邻两列站心之间）。 */
  cols: number;
  /** 带顶 px（纵向，与列宽无关）。 */
  top: number;
  kind: 'main' | 'br' | 'vt' | 'ret';
}

const SPINE_BELTS: readonly SpineBeltSpec[] = [
  { col: 0, cols: 7, top: 36, kind: 'main' }, // 主链节带：init 站心 → done 站心（按段切开）
  { col: 1, cols: 2, top: 22, kind: 'br' }, // 并行上带
  { col: 1, cols: 2, top: 50, kind: 'br' }, // 并行下带
  { col: 1, cols: 0, top: 22, kind: 'vt' }, // 并行双带与主带的纵向接头
  { col: 3, cols: 0, top: 22, kind: 'vt' },
  { col: 1, cols: 1, top: 76, kind: 'ret' }, // 回流带（打回路径，虚线）
  { col: 3, cols: 1, top: 88, kind: 'ret' },
  { col: 5, cols: 1, top: 76, kind: 'ret' },
  { col: 3, cols: 3, top: 100, kind: 'ret' },
];

/** 段内一条带的 px 几何。 */
export interface SpineBelt {
  key: string;
  kind: 'main' | 'br' | 'vt' | 'ret';
  /** 段内坐标，可为负（段起点之后的带被宿主裁掉，见 {@link spineBelts}）。 */
  left: number;
  /** `vt`（纵向接头）的宽度由 CSS 定 6px，此处恒为 0。 */
  width: number;
  top: number;
}

/**
 * 段内的链节带几何（决策 196「同一把刀切列与脊线」）。
 *
 * 带只有**一套**（整条看板的列坐标，即冻结原型的静态几何），两段各自把它**平移**到本段起点：
 * 段内 `left` = 静态 `left` − `columnWidth × base`（可为负）。段外那截由宿主的
 * `overflow: hidden` 裁掉——于是两段拼起来在钉缝处严丝合缝，脊线与列断在同一条 x 上。
 */
export function spineBelts(seg: SpineSegment): SpineBelt[] {
  const c = GEOMETRY.columnWidth;
  return SPINE_BELTS.map((b) => ({
    // 键要能区分**同列同类的多条带**（并行的上下两条 `br` 同 col=1，只按 kind+col 会撞键，
    // Svelte 的 `{#each ... (key)}` 会当场抛 `each_key_duplicate`）——top 在同一 col 内唯一。
    key: `${b.kind}${b.col}@${b.top}`,
    kind: b.kind,
    left: spineColumnX(b.col) - c * seg.base,
    width: b.kind === 'vt' ? 0 : c * b.cols,
    top: b.top,
  }));
}

/** 段内站心 x：整条看板的列 x 减去段起点（两段各自从 0 计）。 */
export function segmentStationX(x: number, base: number): number {
  return x - GEOMETRY.columnWidth * base;
}

/** 整条看板的脊线带几何（八列一段，用于几何守护的端点与「同一把刀」断言）。 */
export function spineBeltsAll(): SpineBelt[] {
  return spineBelts({ base: 0, span: BOARD_COLUMNS.length });
}

export function stationStateFromDots(dots: MiniDotState[], i: number): StationState {
  switch (dots[i]) {
    case 'done':
      return 'done';
    case 'past':
      return 'done';
    case 'cur':
      return 'go';
    case 'cur-warn':
      return 'warn';
    case 'cur-stop':
      return 'stop';
    case 'dev':
      return 'dev';
    case 'tst':
      return 'test';
    default:
      return 'idle';
  }
}

/**
 * 工位灯的聚合状态：急停琥珀 > 在跑绿 > 失败红 > 归档灰 > 空（决策 251②）。
 *
 * **看板列头与对讲台值班板读的是同一个答案**（规格 `design/theme-6-pixel.md:628`：值班板是
 * 「同一份读数在看板 8 列与顶栏灯带上各有一份，**这是第三份**」）。顶栏那条灯带是**逐任务**
 * 的灯、输入形状不同（一颗灯 = 一个任务），不在此列。
 *
 * 优先序取**看板的可见次序**：`running` 压过 `failed`——「这个工位还在动」比「它有一次失败」
 * 更该被说出口，反过来会把正在重试的工位报成红的。这一档此前正是反的，于是同一问题在
 * helper、看板列头、值班板三处给出两个答案。
 */
export function aggregateStationState(statuses: TaskStatus[]): StationState {
  if (statuses.some((s) => s === 'pending')) return 'warn';
  if (statuses.some((s) => s === 'running')) return 'go';
  if (statuses.some((s) => s === 'failed' || s === 'cancelled')) return 'stop';
  if (statuses.length > 0 && statuses.every((s) => s === 'done')) return 'done';
  return 'idle';
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m${String(s % 60).padStart(2, '0')}s`;
  const h = Math.floor(m / 60);
  return `${h}h${String(m % 60).padStart(2, '0')}m`;
}

/** 卡片持续时长（从创建到 updated_at）。 */
export function taskDuration(task: { created_at: string; updated_at: string }): number {
  const a = Date.parse(task.created_at);
  const b = Date.parse(task.updated_at);
  if (Number.isNaN(a) || Number.isNaN(b)) return 0;
  return Math.max(0, b - a);
}

export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

/** stalled 角标小时数（从 updated_at 起算；测试可注入 now）。 */
export function stalledHours(
  task: { updated_at: string },
  now: number = Date.now(),
): number {
  const since = Date.parse(task.updated_at);
  if (Number.isNaN(since)) return 0;
  return Math.max(0, Math.floor((now - since) / 3_600_000));
}

export function formatStalled(hours: number): string {
  if (hours >= 24) return `已滞留 ${Math.floor(hours / 24)} 天`;
  return `已滞留 ${Math.max(1, hours)} 小时`;
}

/**
 * 移动款标题行的状态短码（theme-6-pixel.md §5 移动原型 `.bar-row`）。
 *
 * 标题行是**一行**（返回 + 标题 + 状态标记），故标记只能是一个词；完整状态句由
 * 紧随其下的 `.dmeta` 行承担（§5 视图 2/3 的 `■ pending ▪ merge_approval`）。
 * 两处都写完整句会让同一件事在屏上出现两遍，且把标题挤成省略号。
 *
 * 原型钉了 pending / running 两码（`.bar-row` 写 `WAIT` / `RUN`）；其余状态原型没有
 * 移动款详情样例，取状态名大写——沿用像素主题自己的全大写短码写法，不另造词表。
 */
export function statusCode(status: TaskStatus): string {
  if (status === 'pending') return 'WAIT';
  if (status === 'running') return 'RUN';
  return status.toUpperCase();
}

/** pending 类型的界面短标签（trigger 用词表原样展示于时间线）。 */
export function pendingLabel(reason: { type: string; context?: { kind?: string } } | null): string {
  if (!reason) return '等待处理';
  switch (reason.type) {
    case 'info_insufficient':
      return '信息不足';
    case 'conflict_wait':
      return '等待冲突任务';
    case 'retry_exhausted':
      return '重试耗尽';
    case 'merge_approval':
      return '合并提案';
    case 'human_review':
      return '人工评审';
    case 'dependency_failed':
      return '依赖失败';
    case 'context_overflow':
      return '上下文超限';
    case 'timeout':
      return '执行超时';
    // 人自己按下的暂停（决策 276）：与其它待办的区别就在这一格——
    // 「等决定」是流水线在等人，「已暂停」是人在按住它。
    case 'user_paused':
      return '已暂停';
    case 'user_decision':
      switch (reason.context?.kind) {
        case 'duplicate_risk':
          return '语义重复风险';
        case 'dirty_worktree':
          return '脏工作区';
        case 'test_code_issue':
          return '测试用例问题';
        case 'gate_recheck':
          return '闸门复检';
        case 'judge_disagreement':
          return '判定分歧';
        case 'review':
          return '评审不通过';
        case 'develop_design_input_insufficient':
        case 'test_design_input_insufficient':
          return '设计输入不足';
        default:
          return '等待决定';
      }
    default:
      return reason.type;
  }
}

export const TRIGGER_LABELS: Record<string, string> = {
  normal: 'normal',
  retry: 'retry',
  node_retry: 'node_retry',
  kickback: 'kickback',
  user_resume: 'user_resume',
  auto_resume: 'auto_resume',
  timeout: 'timeout',
  start: 'start',
};

/** 脊线数字的口径词（决策 197）：带这个词的是流量（累计到过这一站），不带词的是存量。 */
export const SPINE_COUNT_LABEL = '累计';

/** 由各任务的迷你轨状态聚合出看板脊线站点（与卡片迷你轨同源）。 */
export function buildSpineStations(tasks: MiniRailInput[]): StationView[] {
  const allDots = tasks.map((t) => miniRailState(t));
  const geom = railGeometry('spine');
  return RAIL_STAGES.map((stage, i) => {
    const states = allDots
      .map((dots) => stationStateFromDots(dots, i))
      .filter((s) => s !== 'idle');
    // 口径：**累计到过这一站**的任务数（漏斗），与列头的「此刻停在这一列」（存量）是两个量
    // ——故脊线那个数字在框里带 `累计` 词（决策 197），列头数字不带词。
    const count = allDots.filter((dots) => dots[i] !== 'idle').length;
    const parallel = stage === 'develop-design' || stage === 'test-design';
    return {
      key: stage,
      stage,
      label: RAIL_LABELS[stage],
      x: geom[stage].x,
      y: geom[stage].y,
      state: combineStationStates(states),
      // 并行两个侧站没有数字（它是两条轨道，不是一个存量）→ 整块不渲染，不把「没有」画成 0
      count: parallel ? undefined : count,
      parallel: parallel ? (stage === 'develop-design' ? 'dev' : 'test') : undefined,
    };
  });
}

function combineStationStates(states: StationState[]): StationState {
  const order: StationState[] = ['warn', 'stop', 'go', 'dev', 'test', 'done', 'idle'];
  for (const s of order) {
    if (states.includes(s)) return s;
  }
  return 'idle';
}

/** hero：单任务的节点状态（9 站，游标滑动圆点 + 心跳微光）。 */
export function buildHeroStations(task: MiniRailInput): StationView[] {
  const dots = miniRailState(task);
  const geom = railGeometry('hero');
  return RAIL_STAGES.map((stage, i) => {
    const parallel = stage === 'develop-design' || stage === 'test-design';
    return {
      key: stage,
      stage,
      label: RAIL_LABELS[stage],
      x: geom[stage].x,
      y: geom[stage].y,
      state: stationStateFromDots(dots, i),
      parallel: parallel ? (stage === 'develop-design' ? 'dev' : 'test') : undefined,
    };
  });
}

/**
 * 空列引导（§5.3：不放插画，一句话；票 13 的语汇：**状态 → 下一步**）。
 *
 * 与 `EmptyState` 同一口径，只是它落在 264px 宽的列体里，故不用组件、只用同一套说法。
 */
export const EMPTY_HINTS: Record<ColumnKey, string> = {
  init: '这一列还没有任务。新建一个，流水线从 init 开始走。',
  'architect-design': '还没有任务走到这里。任务过了 init 就落架构设计。',
  design: '还没有任务进入设计。架构通过后，开发方案与测试场景在这里并行产出。',
  develop: '还没有任务开始开发。设计汇合后进入这里实现。',
  review: '还没有任务等评审。开发完成的任务会先过评审。',
  test: '还没有任务在跑测试。评审通过的任务会落在这里。',
  merge: '还没有任务等合入。测试通过的任务在这里等待合入 main。',
  done: '还没有任务走到终点。跑完一个任务它会出现。',
};
