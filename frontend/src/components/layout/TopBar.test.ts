import { render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import TopBar from './TopBar.svelte';

/**
 * 顶栏**页面导航行**（决策 198 / design §4.2）：由六项收到三项——对讲台 / 指标 / 设置。
 *
 * 这是**有意的收缩**：顶栏是「第一屏必须懂」的那一处。原「项目 / 模型与密钥 / 技能市场 /
 * 手机访问」四项从这一行移入设置落地页（项名逐字不改、各自路由不变），故这里同时钉两件事：
 * 三项**恰好在**、四项**确实不在**——只钉前者，多留一项也照样绿。
 *
 * 「手机访问」那条「只在本机给入口」的规则随入口一起挪到了落地页，接线测试因此在
 * `SettingsLanding.test.ts`（本文件原先那份 mock `onHostMachine` 的用例已随之搬走）。
 *
 * 断言只落在可访问性契约上（导航区的名字、链接的可读名与 href），不落 class 名。
 */
const NAV: Array<{ label: string; href: string }> = [
  { label: '对讲台', href: '#/talk' },
  { label: '指标', href: '#/metrics' },
  { label: '设置', href: '#/settings' },
];

/** 从这一行移入落地页的四项（它们不该再出现在顶栏导航行里）。 */
const MOVED_OUT = ['项目', '模型与密钥', '技能市场', '手机访问'];

afterEach(() => {
  document.body.innerHTML = '';
});

describe('顶栏页面导航行（决策 198）', () => {
  it('恰三项：对讲台 / 指标 / 设置（顺序与落点逐字如此）', () => {
    render(TopBar);
    const nav = screen.getByRole('navigation', { name: '页面导航' });
    const chips = within(nav).getAllByRole('link');

    expect(chips.map((a) => a.textContent?.trim())).toEqual(NAV.map((n) => n.label));
    expect(chips.map((a) => a.getAttribute('href'))).toEqual(NAV.map((n) => n.href));
  });

  it('「新建任务」与 wordmark 仍在这一行之外（收缩只针对导航行）', () => {
    render(TopBar);
    const nav = screen.getByRole('navigation', { name: '页面导航' });

    // 「新建任务」是顶栏的道具栏动作，不在导航行里——它是第一屏控件，一字不动
    expect(within(nav).queryByRole('button', { name: '新建任务' })).toBeNull();
    expect(screen.getByRole('button', { name: '新建任务' })).not.toBeNull();
    // wordmark 是看板入口，也不是页面导航项
    expect(within(nav).queryByRole('link', { name: 'AGENTPIPELINE' })).toBeNull();
    expect(screen.getByRole('link', { name: 'AGENTPIPELINE' })).not.toBeNull();
  });

  it('移入落地页的四项不再出现在导航行里', () => {
    render(TopBar);
    const nav = screen.getByRole('navigation', { name: '页面导航' });

    for (const label of MOVED_OUT) {
      expect(
        within(nav).queryByRole('link', { name: label }),
        `${label} 仍在顶栏导航行里`,
      ).toBeNull();
    }
  });
});
