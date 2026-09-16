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

  it('对讲台（决策 174）可被解析，并接受原型的 v-talk 写法', () => {
    expect(parseRoute('#/talk')).toEqual({ name: 'talk' });
    // `#v-talk` 是 theme-6-pixel.md §3.3 原型的视图 id：照原型手敲的地址
    // 不应落到 not-found（对讲台是原型先行、实现补票）。
    expect(parseRoute('#v-talk')).toEqual({ name: 'talk' });
  });

  it('技能市场（决策 187）可被解析', () => {
    expect(parseRoute('#/settings/market')).toEqual({ name: 'settings-market' });
    // 与既有两个设置页并列，且不能被它们的前缀吃掉
    expect(parseRoute('#/settings/providers')).toEqual({ name: 'settings-providers' });
    expect(parseRoute('#/settings/projects')).toEqual({ name: 'settings-projects' });
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
