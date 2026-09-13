import { describe, expect, it } from 'vitest';
import { parseRoute } from './router.svelte';

/**
 * 路由解析单测（决策 167 新增 `/share` 后补齐）。
 *
 * 只测纯函数 `parseRoute`：RouterStore 依赖 window，属浏览器行为，
 * 由 Playwright 侧覆盖；这里钉住「路径 → 路由」的映射与尾随时间不会回归。
 */
describe('parseRoute', () => {
  it('看板是根与空 hash 的共同落点', () => {
    expect(parseRoute('')).toEqual({ name: 'board' });
    expect(parseRoute('#')).toEqual({ name: 'board' });
    expect(parseRoute('#/')).toEqual({ name: 'board' });
  });

  it('任务详情解出 id 并解码', () => {
    expect(parseRoute('#/task/01HZX')).toEqual({ name: 'task', id: '01HZX' });
    expect(parseRoute('#/task/a%2Fb')).toEqual({ name: 'task', id: 'a/b' });
  });

  it('手机访问页（决策 167）可被解析', () => {
    expect(parseRoute('#/share')).toEqual({ name: 'share' });
  });

  it('query 与尾斜杠不影响匹配', () => {
    // hash 路由下分享页可能被带上查询串；parseRoute 只取 ? 之前的部分
    expect(parseRoute('#/share?from=board')).toEqual({ name: 'share' });
    expect(parseRoute('#/metrics?x=1')).toEqual({ name: 'metrics' });
  });

  it('未知路径回落到 not-found 并带回原路径', () => {
    expect(parseRoute('#/nope')).toEqual({ name: 'not-found', path: '/nope' });
  });
});
