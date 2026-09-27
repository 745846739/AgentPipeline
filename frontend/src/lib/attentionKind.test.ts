import { describe, expect, it } from 'vitest';
import {
  ATTENTION_KIND_LABELS,
  attentionKindLabel,
  attentionKindSummary,
  visibleAttention,
} from './attentionKind';

/**
 * 类别表与后端**逐字对齐**（决策 307，票 06）。
 *
 * 这张清单是**钉住的**：后端 `AttentionKind` 加一类而这里没跟，就会在提示里露出一个
 * 英文标识（`blocked_read 1`）——那比不提示更让人困惑。所以这里逐个列出来，加一类就
 * 得两边一起改。
 */
const KINDS = [
  'task_pending',
  'retry_exhausted',
  'context_overflow',
  'gate_failure',
  'repeated_pending',
  'scheduler_no_effect',
  'owner_stuck',
  'task_stale',
  'task_done',
  'slow_run',
  'run_failed',
  'task_cancelled',
  'resume_blocked',
  'blocked_read',
];

describe('待办类别的中文名', () => {
  it('十四类齐全，且没有多出后端不认的键', () => {
    expect(Object.keys(ATTENTION_KIND_LABELS).sort()).toEqual([...KINDS].sort());
    expect(KINDS).toHaveLength(14);
  });

  it('每一类的名字都非空', () => {
    for (const kind of KINDS) {
      expect(attentionKindLabel(kind)).toBeTruthy();
    }
  });

  it('认不出的值原样回落（不吞信息）', () => {
    expect(attentionKindLabel('something_new')).toBe('something_new');
  });

  it('摘要按条数降序，同数按名字', () => {
    expect(
      attentionKindSummary({
        blocked_read: 1,
        owner_stuck: 2,
        resume_blocked: 2,
      }),
    ).toBe('执行体卡住 2、续跑被挡下 2、文件读卡住 1');
  });
});

/**
 * 页头那枚读数的**渲染条件**（决策 307，票 06）：有未消费待办才在，0 条时不占窄档空间。
 *
 * 这条判据的意义在于「最需要被看见的那一刻」：值守轮**正在排队**（`in_flight = false`）时
 * 它照样在——而那一刻「值守台账 · 正在跑」那枚 crumb 根本不出现（它的判据是轮次在飞）。
 * 判据读的是 `/foreman/attention` 的读数字段，与值守轮的状态无关。
 */
describe('页头待办读数的渲染条件', () => {
  it('有未消费待办就渲染（值守轮在不在飞都要在）', () => {
    const open = { open: 2, by_kind: { run_failed: 2 } };
    expect(visibleAttention(open)).toEqual(open);
  });

  it('0 条时不渲染，也不在窄档占位置', () => {
    expect(visibleAttention({ open: 0, by_kind: {} })).toBeNull();
  });

  it('读数还没拿到（端点失败 / 首帧）时不渲染，不画一个 0 条', () => {
    expect(visibleAttention(null)).toBeNull();
  });
});
