import { describe, expect, it } from 'vitest';
import { buildHeroStations, RAIL_STAGES, railGeometry, type MiniRailInput } from './pipeline';

/**
 * 详情 hero 与冻结原型对账（票 06 / 决策 169）。
 *
 * `design/prototype-pixel.html` 的 `#v-run .hrail` 是验收参照：9 站、站距约 106px、
 * 并行双站共用同一 x 上下分行。本用例把这些坐标钉住，防止后续改动悄悄偏离原型
 * （原型坐标由 playwright 实测：56 / 162 / 272 / 272 / 374 / 480 / 586 / 692 / 798）。
 */
const running: MiniRailInput = {
  status: 'running',
  current_stage: 'develop',
  stalled: false,
};

describe('hero 轨道与冻结原型对账（票 06）', () => {
  it('9 站、不含 sync-check（决策 107）', () => {
    const stages = buildHeroStations(running).map((s) => s.stage);
    expect(stages).toEqual(RAIL_STAGES);
    expect(stages).toHaveLength(9);
    expect(stages).not.toContain('sync-check');
  });

  it('站点 x 与原型 #v-run .hrail 逐站一致', () => {
    const xs = Object.fromEntries(
      buildHeroStations(running).map((s) => [s.stage, s.x]),
    );
    expect(xs).toEqual({
      init: 56,
      'architect-design': 162,
      'develop-design': 272,
      'test-design': 272,
      develop: 374,
      review: 480,
      test: 586,
      merge: 692,
      done: 798,
    });
  });

  it('并行双站共用 x、上下分行（twin belts）', () => {
    const stations = buildHeroStations(running);
    const dev = stations.find((s) => s.stage === 'develop-design');
    const tst = stations.find((s) => s.stage === 'test-design');
    expect(dev?.parallel).toBe('dev');
    expect(tst?.parallel).toBe('test');
    expect(dev?.x).toBe(tst?.x);
    expect(dev?.y).toBeLessThan(tst?.y ?? 0);
  });

  it('hero 几何是独立于看板脊线的一组坐标', () => {
    const hero = railGeometry('hero');
    const spine = railGeometry('spine');
    expect(hero.init.x).not.toBe(spine.init.x);
    expect(Object.keys(hero)).toHaveLength(9);
  });
});
