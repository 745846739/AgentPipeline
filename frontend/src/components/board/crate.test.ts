/**
 * 看板像素原语单测（票 05 / 决策 169）。
 *
 * 断言落在**可见图元**上：量表段数、boss 条段数与转红、列头小人节奏、
 * sprite 渲染。这些是视觉改造的"外部行为"。
 */
import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { GEOMETRY, SPRITES, bossFilled, bossExhausted, gaugeFilled } from '../../theme/contract';
import { COLUMN_SPRITES, crateState, crateTone, workerRhythm } from '../../lib/pipeline';
import { BOARD_COLUMNS } from '../../lib/pipeline';
import type { TaskListItem } from '../../api/types';
import Gauge from '../render/Gauge.svelte';
import Sprite from '../render/Sprite.svelte';
import BossBar from './BossBar.svelte';
import TaskCard from './TaskCard.svelte';

/** 最小可用任务（只填 TaskCard 会读到的字段）。 */
function task(overrides: Partial<TaskListItem> = {}): TaskListItem {
  return {
    id: 't-1',
    project_id: 'p-1',
    title: '货箱',
    description: '',
    status: 'running',
    current_stage: 'develop',
    current_node: 'execute',
    validate_attempts: 0,
    pending_reason: null,
    worktree_path: null,
    branch_name: null,
    total_tokens: 1000,
    total_calls: 3,
    review_mode: 'agent',
    model_override: null,
    archived_at: null,
    stalled: false,
    executor_owner: null,
    created_at: '2026-09-13T00:00:00Z',
    updated_at: '2026-09-13T00:01:00Z',
    branches: [],
    blocks: [],
    ...overrides,
  };
}

describe('token 量表（Gauge）', () => {
  it('渲染 16 段，点亮段数由 token 数折算', () => {
    const { container } = render(Gauge, { props: { tokens: 32_000 } });
    const segs = container.querySelectorAll('i');
    expect(segs.length).toBe(GEOMETRY.gaugeSegments);
    expect(segs.length).toBe(16);
    expect(container.querySelectorAll('i.f').length).toBe(gaugeFilled(32_000));
    expect(gaugeFilled(32_000)).toBe(8);
  });

  it('0 token 不点亮任何段（不假装有量）', () => {
    const { container } = render(Gauge, { props: { tokens: 0 } });
    expect(container.querySelectorAll('i.f').length).toBe(0);
  });

  it('tone 取四盏信号灯语义（go / warn / stop / dim），不引第二套色', () => {
    for (const tone of ['go', 'warn', 'stop', 'dim'] as const) {
      const { container } = render(Gauge, { props: { tokens: 1000, tone } });
      expect(container.querySelector('.gauge')?.classList.contains(tone)).toBe(true);
    }
  });
});

describe('boss 战尝试条（BossBar）', () => {
  it('渲染 20 段', () => {
    const { container } = render(BossBar, { props: { used: 1, limit: 3 } });
    expect(container.querySelectorAll('.segs i').length).toBe(GEOMETRY.bossSegments);
    expect(container.querySelectorAll('.segs i').length).toBe(20);
  });

  it('点亮段数按 已用/上限 折算', () => {
    const { container } = render(BossBar, { props: { used: 2, limit: 3 } });
    const lit = container.querySelectorAll('.segs i.f').length;
    expect(lit).toBe(bossFilled(2, 3));
    expect(lit).toBe(13);
  });

  it('最后一次尝试整条转红', () => {
    const atLimit = render(BossBar, { props: { used: 3, limit: 3 } });
    expect(atLimit.container.querySelector('.segs.red')).not.toBeNull();
    expect(bossExhausted(3, 3)).toBe(true);

    const beforeLimit = render(BossBar, { props: { used: 2, limit: 3 } });
    expect(beforeLimit.container.querySelector('.segs.red')).toBeNull();
  });

  it('后端权威下发 retry_exhausted 时强制转红（不依赖镜像分母）', () => {
    const { container } = render(BossBar, { props: { used: 1, limit: 9, exhausted: true } });
    expect(container.querySelector('.segs.red')).not.toBeNull();
  });
});

describe('货箱卡上的 boss 条可见性（票 05 招牌行为的可达性）', () => {
  it('运行中的货箱带 boss 条', () => {
    const { container } = render(TaskCard, {
      props: { task: task({ status: 'running', validate_attempts: 1 }) },
    });
    expect(container.querySelector('.bossbar')).not.toBeNull();
  });

  it('耗尽重试的 pending 货箱也带 boss 条，且整条转红', () => {
    // 关键回归点：retry_exhausted 的状态是 pending（不是 running）。若按 running 门槛，
    // 「最后一次尝试整条转红」这条招牌行为在真应用里永远不可达——这里钉住它。
    const { container } = render(TaskCard, {
      props: {
        task: task({
          status: 'pending',
          validate_attempts: 3,
          pending_reason: {
            type: 'retry_exhausted',
            stage: 'develop',
            node: 'execute',
            message: '重试耗尽',
          },
        }),
      },
    });
    expect(container.querySelector('.bossbar')).not.toBeNull();
    expect(container.querySelector('.segs.red')).not.toBeNull();
  });

  it('其它 pending 类型不带 boss 条（不把每次等人拍板都画成 boss 战）', () => {
    const { container } = render(TaskCard, {
      props: {
        task: task({
          status: 'pending',
          pending_reason: {
            type: 'merge_approval',
            stage: 'merge',
            node: 'execute',
            message: '等待审批合入',
          },
        }),
      },
    });
    expect(container.querySelector('.bossbar')).toBeNull();
  });

  it('queued / waiting 不带 boss 条', () => {
    for (const status of ['queued', 'waiting'] as const) {
      const { container } = render(TaskCard, { props: { task: task({ status }) } });
      expect(container.querySelector('.bossbar'), status).toBeNull();
    }
  });
});

describe('状态映射（灯 / 描边 / 小人节奏）', () => {
  it('任务状态 → 契约货箱状态', () => {
    expect(crateState({ status: 'running' })).toBe('running');
    expect(crateState({ status: 'pending' })).toBe('pending');
    expect(crateState({ status: 'failed' })).toBe('failed');
    expect(crateState({ status: 'cancelled' })).toBe('failed');
    expect(crateState({ status: 'done' })).toBe('done');
    expect(crateState({ status: 'queued' })).toBe('queued');
    expect(crateState({ status: 'waiting' })).toBe('waiting');
  });

  it('量表 tone 随状态灯（执行绿 / 急停琥珀 / 失败红 / 归档灰）', () => {
    expect(crateTone('running')).toBe('go');
    expect(crateTone('pending')).toBe('warn');
    expect(crateTone('failed')).toBe('stop');
    expect(crateTone('done')).toBe('dim');
    expect(crateTone('queued')).toBe('dim');
  });

  it('列头小人节奏：执行快挥 / 急停慢挥 / 其余站立', () => {
    expect(workerRhythm('go')).toBe('run');
    expect(workerRhythm('dev')).toBe('run');
    expect(workerRhythm('test')).toBe('run');
    expect(workerRhythm('warn')).toBe('wait');
    expect(workerRhythm('idle')).toBe('idle');
    expect(workerRhythm('done')).toBe('idle');
    expect(workerRhythm('stop')).toBe('idle');
  });

  it('8 列各自映射到受控 sprite 表里的一枚图元', () => {
    expect(Object.keys(COLUMN_SPRITES).length).toBe(BOARD_COLUMNS.length);
    for (const col of BOARD_COLUMNS) {
      const name = COLUMN_SPRITES[col.key];
      expect(Object.keys(SPRITES)).toContain(name);
    }
  });
});

describe('像素图元（Sprite）', () => {
  it('按名字渲染非空 SVG，用 currentColor', () => {
    const { container } = render(Sprite, { props: { name: 'hammer' } });
    const svg = container.querySelector('svg');
    expect(svg).not.toBeNull();
    expect(svg?.querySelectorAll('rect').length).toBeGreaterThan(0);
    expect(svg?.getAttribute('fill')).toBe('currentColor');
    expect(svg?.getAttribute('shape-rendering')).toBe('crispEdges');
  });

  it('viewBox 用契约的网格（8×8 图元 / 16×16 工头）', () => {
    const { container } = render(Sprite, { props: { name: 'flag' } });
    expect(container.querySelector('svg')?.getAttribute('viewBox')).toBe('0 0 8 8');

    const foreman = render(Sprite, { props: { name: 'foreman' } });
    expect(foreman.container.querySelector('svg')?.getAttribute('viewBox')).toBe('0 0 16 16');
  });

  it('契约里写 token 名的 rect 渲染成 var(--token)', () => {
    const { container } = render(Sprite, { props: { name: 'chest' } });
    const fills = [...container.querySelectorAll('rect')].map((r) => r.getAttribute('fill'));
    expect(fills).toContain('var(--bg)');
  });

  it('工头脸块用固定肤色（§2.4 偏差②：浅色下不变墨块）', () => {
    const { container } = render(Sprite, { props: { name: 'foreman' } });
    const fills = [...container.querySelectorAll('rect')].map((r) => r.getAttribute('fill'));
    expect(fills).toContain('#E3C7A6');
    expect(fills).not.toContain('var(--text-hi)');
  });

  it('图元不带可访问名（装饰性，宿主负责语义）', () => {
    render(Sprite, { props: { name: 'trophy' } });
    expect(screen.queryByRole('img')).toBeNull();
  });
});
