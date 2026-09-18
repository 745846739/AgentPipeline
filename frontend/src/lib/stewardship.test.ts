import { describe, expect, it } from 'vitest';

import type { Stewardship } from '../api/types';
import {
  STEWARDSHIP_MAX_AUTO_RESUMES,
  stewardshipFace,
  toggleStewardship,
  type StewardshipDeps,
} from './stewardship';

function stewardship(patch: Partial<Stewardship> = {}): Stewardship {
  return {
    enabled: true,
    auto_resumes: 0,
    last_fingerprint: null,
    updated_at: null,
    ...patch,
  };
}

describe('托管开关该不该摆（决策 210① / 票 14）', () => {
  it('值班长未接线时不摆——没人会用那份授权（端点回 503）', () => {
    expect(stewardshipFace({ status: 'running', stewardship: null }, false)).toBeNull();
  });

  it('终态任务不摆——托管随任务自限（端点回 400）', () => {
    for (const status of ['done', 'failed', 'cancelled'] as const) {
      expect(
        stewardshipFace({ status, stewardship: stewardship() }, true),
        `${status} 上不该摆`,
      ).toBeNull();
    }
  });

  it('未托管时说的是「只能提议，动手要你按键」', () => {
    const face = stewardshipFace({ status: 'pending', stewardship: null }, true);
    expect(face?.enabled).toBe(false);
    expect(face?.note).toContain('按键');
    expect(face?.label).toBe('托管：关');
  });

  it('托管中要说清还剩几次自动动作——那是止损线的可见形态', () => {
    const face = stewardshipFace(
      { status: 'running', stewardship: stewardship({ auto_resumes: 1 }) },
      true,
    );
    expect(face?.enabled).toBe(true);
    expect(face?.note).toContain(`还剩 ${STEWARDSHIP_MAX_AUTO_RESUMES - 1} 次`);

    const usedUp = stewardshipFace(
      {
        status: 'running',
        stewardship: stewardship({ auto_resumes: STEWARDSHIP_MAX_AUTO_RESUMES }),
      },
      true,
    );
    expect(usedUp?.note).toContain('只能提议');
    expect(usedUp?.enabled, '用满之后托管仍是开着的——只是不再自动动手').toBe(true);
  });
});

describe('拨一下（决策 210①）', () => {
  function deps(fail?: Error): { calls: Array<[string, boolean]>; deps: StewardshipDeps } {
    const calls: Array<[string, boolean]> = [];
    return {
      calls,
      deps: {
        async set(id, enabled) {
          calls.push([id, enabled]);
          if (fail) throw fail;
        },
      },
    };
  }

  it('打开与关掉走同一个端点，方向由 enabled 说', async () => {
    const { calls, deps: d } = deps();
    expect((await toggleStewardship('t1', true, d)).ok).toBe(true);
    expect((await toggleStewardship('t1', false, d)).ok).toBe(true);
    expect(calls).toEqual([
      ['t1', true],
      ['t1', false],
    ]);
  });

  it('被拒时把后端那句原样说出来——那是给人读的原因', async () => {
    const { deps: d } = deps(new Error('任务 t1 已是终态（done）：托管随任务自限'));
    const result = await toggleStewardship('t1', true, d);
    expect(result.ok).toBe(false);
    expect(result.ok === false && result.message).toContain('终态');
  });
});
