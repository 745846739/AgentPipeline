import { describe, expect, it, vi } from 'vitest';
import { parseRoute, readQuery, router, writeQuery } from './router.svelte';

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
  it('根 `#/` 是看板；空 hash 的**纯解析**也仍是看板（开屏那一层另有一手，见「开屏默认落点」）', () => {
    // 决策 241 把默认落点改成了对讲台，但改的是**开屏归一**（store 进场前那手 replaceState），
    // 不是 `parseRoute`：显式 `#/` 必须照旧是看板——决策 240 的「看板是根路由」不修订，
    // §4.2 四枚页签的落点与十几处 `goto('#/')` 的 e2e 也都指着它。
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

/**
 * 开屏默认落点（决策 241）：地址栏没写 hash 时归一到 `#/talk`。
 *
 * 这一手住在**模块初始化**里（`router.svelte.ts` 顶部、`new RouterStore()` 之前），
 * 所以只能靠清掉模块缓存重跑一遍来钉——直接调 `parseRoute('')` 钉不到它：纯解析照旧把空
 * hash 算成看板（上一组用例），归一是 store 进场前那手 `replaceState` 干的。
 */
describe('开屏默认落点（决策 241）', () => {
  it('空 hash 开屏归一成 #/talk，且 ?pair= 与后退历史都不动；显式 #/ 仍是看板', async () => {
    // 扫码进来的地址长这样：有 search、没 hash（决策 191 的令牌就挂在 search 上）
    window.history.replaceState(null, '', '/?pair=e2e-token');
    expect(window.location.hash).toBe('');
    const historyBefore = window.history.length;

    vi.resetModules();
    const fresh = await import('./router.svelte');

    // 归一到对讲台：地址与渲染读的是同一份（replaceState 不发 hashchange，
    // 所以归一必须跑在 store 首读之前）
    expect(window.location.hash).toBe('#/talk');
    expect(window.location.search).toBe('?pair=e2e-token');
    expect(fresh.router.route.name).toBe('talk');
    // 不进历史：开屏这一跳用 replaceState，后退不该退回「还没归一」的空地址
    expect(window.history.length).toBe(historyBefore);

    // 显式 #/ 照旧是看板（决策 240 不修订）：默认落点只接管空地址
    fresh.router.navigate('/');
    expect(window.location.hash).toBe('#/');
    expect(fresh.router.route.name).toBe('board');
  });

  it('带 hash 的开屏不动（#/、#/metrics 各归各位）', async () => {
    window.history.replaceState(null, '', '/#/metrics');
    vi.resetModules();
    const fresh = await import('./router.svelte');
    expect(window.location.hash).toBe('#/metrics');
    expect(fresh.router.route.name).toBe('metrics');
  });
});

/**
 * 查询串的读写口子（决策 217③，票 22 的实现面；对讲台的 `?session=` 是本叠第一个消费者）。
 *
 * `pushState` / `replaceState` **不发 `hashchange`**，故这里同时钉住「路由状态自己接上」
 * 这一条——不写就是「地址变了、页面没变」。
 */
describe('readQuery / writeQuery（决策 217）', () => {
  it('读的是当前地址那份查询串', async () => {
    router.navigate('/talk?session=abc');
    // 地址栏是权威：即使路由状态还没跟上（`hashchange` 是一次异步的任务），
    // 这里读到的也是**此刻地址里那一份**
    expect(readQuery()).toEqual({ session: 'abc' });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(router.route).toEqual({ name: 'talk', query: { session: 'abc' } });
  });

  it('写：默认 push（进历史），`replace` 不进；值没变时不动地址', () => {
    router.navigate('/talk');
    const base = window.history.length;

    writeQuery({ session: 'a' });
    expect(window.location.hash).toBe('#/talk?session=a');
    // 路由状态自己接上了（pushState 不发 hashchange）
    expect(router.route.query.session).toBe('a');
    expect(window.history.length).toBe(base + 1);

    // 同一个值再写一遍：地址没变，也就不该多一条历史
    writeQuery({ session: 'a' });
    expect(window.history.length).toBe(base + 1);

    writeQuery({ session: 'b' }, { replace: true });
    expect(window.location.hash).toBe('#/talk?session=b');
    expect(window.history.length).toBe(base + 1);
    expect(router.route.query.session).toBe('b');
  });

  it('`null` 删键；键删完之后回到没有查询串的干净地址', () => {
    router.navigate('/talk?session=a&x=1');
    writeQuery({ session: null });
    expect(window.location.hash).toBe('#/talk?x=1');
    writeQuery({ x: null });
    expect(window.location.hash).toBe('#/talk');
    expect(readQuery()).toEqual({});
  });

  it('只改查询串，路径一字不动（`#/task/01HZX?tab=diff` 这类）', () => {
    router.navigate('/task/01HZX');
    writeQuery({ tab: 'diff' });
    expect(window.location.hash).toBe('#/task/01HZX?tab=diff');
    expect(router.route).toMatchObject({ name: 'task', id: '01HZX', query: { tab: 'diff' } });
  });
});
