import { describe, expect, it } from 'vitest';
import { parseRoute } from './router.svelte';

/**
 * 路由解析单测（决策 167 新增 `/share` 后补齐；决策 198 新增设置落地页与阶段配置页后重写）。
 *
 * 只测纯函数 `parseRoute`：RouterStore 依赖 window，属浏览器行为，
 * 由 Playwright 侧覆盖；这里钉住「路径 → 路由」的映射、**查询串解析**与尾随时间不会回归。
 *
 * 查询串那一组是**跨流契约**（`parallel-brief.md` §二 末尾的接口表）：
 * `#/metrics?task=<id>` 与 `#/settings/projects?project=<id>&analyze=1` 两个 key 名逐字如此，
 * 消费方（指标页 / 项目页）按它们取值，改名会静默断掉那两个入口。
 */
describe('parseRoute', () => {
  it('看板是根与空 hash 的共同落点', () => {
    expect(parseRoute('')).toEqual({ name: 'board', query: {} });
    expect(parseRoute('#')).toEqual({ name: 'board', query: {} });
    expect(parseRoute('#/')).toEqual({ name: 'board', query: {} });
  });

  it('任务详情解出 id 并解码', () => {
    expect(parseRoute('#/task/01HZX')).toEqual({ name: 'task', id: '01HZX', query: {} });
    expect(parseRoute('#/task/a%2Fb')).toEqual({ name: 'task', id: 'a/b', query: {} });
  });

  it('手机访问页（决策 167）可被解析', () => {
    expect(parseRoute('#/share')).toEqual({ name: 'share', query: {} });
  });

  it('对讲台（决策 174）可被解析，并接受原型的 v-talk 写法', () => {
    expect(parseRoute('#/talk')).toEqual({ name: 'talk', query: {} });
    // `#v-talk` 是 theme-6-pixel.md §3.3 原型的视图 id：照原型手敲的地址
    // 不应落到 not-found（对讲台是原型先行、实现补票）。
    expect(parseRoute('#v-talk')).toEqual({ name: 'talk', query: {} });
  });

  it('技能市场（决策 187）可被解析', () => {
    expect(parseRoute('#/settings/market')).toEqual({ name: 'settings-market', query: {} });
    // 与既有两个设置页并列，且不能被它们的前缀吃掉
    expect(parseRoute('#/settings/providers')).toEqual({ name: 'settings-providers', query: {} });
    expect(parseRoute('#/settings/projects')).toEqual({ name: 'settings-projects', query: {} });
  });

  it('设置落地页与阶段配置页（决策 198）各有自己的名字', () => {
    expect(parseRoute('#/settings')).toEqual({ name: 'settings-landing', query: {} });
    // `/settings/stages` 是独立页，不能被 `/settings` 的前缀吃掉（反过来也一样）
    expect(parseRoute('#/settings/stages')).toEqual({ name: 'settings-stages', query: {} });
    // 尾斜杠不是第三条路由，落 not-found 而不是被静默当成落地页
    expect(parseRoute('#/settings/')).toEqual({ name: 'not-found', path: '/settings/', query: {} });
  });

  it('query 与尾斜杠不影响匹配', () => {
    // hash 路由下分享页可能被带上查询串；路径部分照旧只取 ? 之前
    expect(parseRoute('#/share?from=board')).toEqual({
      name: 'share',
      query: { from: 'board' },
    });
    expect(parseRoute('#/metrics?x=1')).toEqual({ name: 'metrics', query: { x: '1' } });
  });

  describe('跨流入口的查询串（决策 198 / 票 21·06·07）', () => {
    it('任务指标入口：#/metrics?task=<id> 解出 query.task', () => {
      expect(parseRoute('#/metrics?task=01HZX2K9')).toEqual({
        name: 'metrics',
        query: { task: '01HZX2K9' },
      });
      // 值要解码（ULID 不会有特殊字符，但 id 允许百分号编码）
      expect(parseRoute('#/metrics?task=a%2Fb').query.task).toBe('a/b');
    });

    it('项目分析入口：#/settings/projects?project=<id>&analyze=1 解出两个 key', () => {
      expect(parseRoute('#/settings/projects?project=p-1&analyze=1')).toEqual({
        name: 'settings-projects',
        query: { project: 'p-1', analyze: '1' },
      });
      // 只有 project 时也成立（analyze 缺省 = 不触发分析）
      expect(parseRoute('#/settings/projects?project=p-1').query).toEqual({ project: 'p-1' });
    });

    it('没有查询串时 query 是空对象，不是 undefined', () => {
      // 调用方一律 `route.query.task ?? fallback` 读，不必先判空
      for (const hash of ['#/', '#/talk', '#/metrics', '#/settings', '#/settings/stages']) {
        expect(parseRoute(hash).query).toEqual({});
      }
    });

    it('重复 key 取首次出现的值（不是后者覆盖前者）', () => {
      expect(parseRoute('#/metrics?task=a&task=b').query).toEqual({ task: 'a' });
    });

    it('查询串也落在 task 与 not-found 上，不丢', () => {
      expect(parseRoute('#/task/01HZX?tab=diff')).toEqual({
        name: 'task',
        id: '01HZX',
        query: { tab: 'diff' },
      });
      expect(parseRoute('#/nope?x=1')).toEqual({
        name: 'not-found',
        path: '/nope',
        query: { x: '1' },
      });
    });
  });

  it('未知路径回落到 not-found 并带回原路径', () => {
    expect(parseRoute('#/nope')).toEqual({ name: 'not-found', path: '/nope', query: {} });
  });
});
