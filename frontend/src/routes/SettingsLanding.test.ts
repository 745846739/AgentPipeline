import { render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import SettingsLanding from './SettingsLanding.svelte';

/**
 * 设置落地页（决策 198 / design §4.3）接线层：分类（决策 272⑧ 起三类）、八个入口，
 * 以及**「手机访问」项随来源取舍**那一条。
 *
 * 那一条的原判据钉在 `lib/localPage.test.ts`（主机名），这里钉的是**接线**——
 * 组件确实按 `onHostMachine()` 取舍了「手机访问」这一项，且其余四项一个不少
 * （藏一个入口顺带把别的一起藏了，是这类改动最容易犯的错）。这条接线测试原先在
 * `TopBar.test.ts`：入口从顶栏挪进落地页，接线跟着一起挪。
 *
 * 断言只落在可访问性契约上（heading / link 的可读名与 href），不落 class 名。
 */

const mocks = vi.hoisted(() => ({ onHostMachine: vi.fn() }));

vi.mock('../lib/localPage', () => ({ onHostMachine: mocks.onHostMachine }));

/** 各分类的项（design §4.3 的定稿项名，逐字）。 */
const WHO = ['项目', '手机访问'];
const REACH = ['离线通知'];
const HOW = ['值守轮', '命令执行', '模型与密钥', '阶段配置', '技能市场'];

afterEach(() => {
  vi.resetAllMocks();
  document.body.innerHTML = '';
});

describe('设置落地页（决策 198）', () => {
  it('标题「设置」，分类按用途三分（决策 272⑧ 加「怎么找到你」）', () => {
    mocks.onHostMachine.mockReturnValue(true);
    render(SettingsLanding);

    expect(screen.getByRole('heading', { level: 1, name: '设置' })).not.toBeNull();
    expect(screen.getByRole('heading', { name: '谁能进来' })).not.toBeNull();
    expect(screen.getByRole('heading', { name: '怎么找到你' })).not.toBeNull();
    expect(screen.getByRole('heading', { name: '怎么跑' })).not.toBeNull();
  });

  it('每一项都可点，且落在各自的路由上', () => {
    mocks.onHostMachine.mockReturnValue(true);
    render(SettingsLanding);

    const expected: Array<[RegExp, string]> = [
      [/项目/, '#/settings/projects'],
      [/手机访问/, '#/share'],
      [/离线通知/, '#/settings/notify'],
      [/值守轮/, '#/settings/foreman'],
      [/命令执行/, '#/settings/tools'],
      [/模型与密钥/, '#/settings/providers'],
      [/阶段配置/, '#/settings/stages'],
      [/技能市场/, '#/settings/market'],
    ];
    for (const [name, href] of expected) {
      expect(screen.getByRole('link', { name }).getAttribute('href'), String(name)).toBe(href);
    }
  });

  it('指标不列在这里（它不是设置，留在顶栏）', () => {
    mocks.onHostMachine.mockReturnValue(true);
    render(SettingsLanding);

    expect(screen.queryByRole('link', { name: /指标/ })).toBeNull();
  });

  it('在本机打开：「手机访问」项在', () => {
    mocks.onHostMachine.mockReturnValue(true);
    render(SettingsLanding);

    for (const label of [...WHO, ...REACH, ...HOW]) {
      expect(screen.queryByRole('link', { name: new RegExp(label) }), label).not.toBeNull();
    }
  });

  it('在手机（或任何非本机来源）打开：不给「手机访问」这一项，其余五项不动', () => {
    mocks.onHostMachine.mockReturnValue(false);
    render(SettingsLanding);

    // 不渲染这一项（不是禁用、不是留个空位）：规则与后果与决策 190 逐字一致
    expect(screen.queryByRole('link', { name: /手机访问/ })).toBeNull();
    expect(screen.queryByText(/手机访问/)).toBeNull();
    for (const label of ['项目', ...REACH, ...HOW]) {
      expect(screen.queryByRole('link', { name: new RegExp(label) }), label).not.toBeNull();
    }
  });
});
