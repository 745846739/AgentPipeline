import type { BranchCursor, Stage, TaskStatus } from '../api/types';

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
 * 迷你轨字符记号（theme-3 §3 共享元素映射）。
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
 * （列 i 中心 = 132 + 264i；并行区间是一个列两个侧站，共用 x = 660）。
 */
const SPINE_GEOM: Record<string, Geometry> = {
  init: { x: 132, y: 62 },
  'architect-design': { x: 396, y: 62 },
  'develop-design': { x: 660, y: 22 },
  'test-design': { x: 660, y: 50 },
  develop: { x: 924, y: 62 },
  review: { x: 1188, y: 62 },
  test: { x: 1452, y: 62 },
  merge: { x: 1716, y: 62 },
  done: { x: 1980, y: 62 },
};

/**
 * hero 坐标：详情内容区最宽 1040px，主站均匀分布，并行区间双侧站共用 x。
 * y 仅用于侧站上下分行（主站恒 62，由 .stn 的 top:29px 决定）。
 */
const HERO_GEOM: Record<string, Geometry> = {
  init: { x: 60, y: 62 },
  'architect-design': { x: 165, y: 62 },
  'develop-design': { x: 365, y: 22 },
  'test-design': { x: 365, y: 50 },
  develop: { x: 565, y: 62 },
  review: { x: 690, y: 62 },
  test: { x: 815, y: 62 },
  merge: { x: 940, y: 62 },
  done: { x: 1030, y: 62 },
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

/** 看板列头的聚合站点状态：有 pending → warn；在跑 → go；否则 idle。 */
export function aggregateStationState(statuses: TaskStatus[]): StationState {
  if (statuses.some((s) => s === 'pending')) return 'warn';
  if (statuses.some((s) => s === 'failed' || s === 'cancelled')) return 'stop';
  if (statuses.some((s) => s === 'running')) return 'go';
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

/** 由各任务的迷你轨状态聚合出看板脊线站点（与卡片迷你轨同源）。 */
export function buildSpineStations(tasks: MiniRailInput[]): StationView[] {
  const allDots = tasks.map((t) => miniRailState(t));
  const geom = railGeometry('spine');
  return RAIL_STAGES.map((stage, i) => {
    const states = allDots
      .map((dots) => stationStateFromDots(dots, i))
      .filter((s) => s !== 'idle');
    const count = allDots.filter((dots) => dots[i] !== 'idle').length;
    const parallel = stage === 'develop-design' || stage === 'test-design';
    return {
      key: stage,
      stage,
      label: RAIL_LABELS[stage],
      x: geom[stage].x,
      y: geom[stage].y,
      state: combineStationStates(states),
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

/** 空态文案（§5.3：不放插画，一句话）。 */export const EMPTY_HINTS: Record<ColumnKey, string> = {
  init: '新建第一个任务，流水线会从 init 开始走。',
  'architect-design': '任务开始后第一步会落到架构设计。',
  design: '架构通过后，开发方案与测试场景在这里并行产出。',
  develop: '设计汇合后进入开发实现。',
  review: '开发完成的任务会先过评审。',
  test: '评审通过的任务会在这里跑集成测试。',
  merge: '测试通过的任务在这里等待合入 main。',
  done: '还没有任务走到终点。跑完一个任务它会出现。',
};
