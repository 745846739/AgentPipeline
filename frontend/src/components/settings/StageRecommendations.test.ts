import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { RecommendedSkill, RecommendedStage } from '../../api/types';
import StageRecommendations from './StageRecommendations.svelte';

/**
 * 推荐面板的三态（票 16「未安装的项带安装按钮，**已安装的可直接启用**」，票 01）。
 *
 * 这一票的由来是一次实机报障：本机技能根里十个推荐技能全都在，而面板的按钮条件写的是
 * 「未安装」，于是**一行按钮都没有**——「已安装的可直接启用」那半条链后端早已实现
 * （`POST /skills/install` 撞上「已在技能根里」时只写配置、不重新下载），界面这一半从来没写。
 *
 * 判据是后端给的 `declared_here`（机器可读），**不是** `declared_in`：后者是给人看的位置
 * 说明串（「阶段 test-design」），拿它判断阶段等于 parse 文案。故下面用例里的 `declared_in`
 * 与 `declared_here` 刻意造得不一致——那正是 develop 行上 `tdd` 的真实形态。
 */

function skill(over: Partial<RecommendedSkill> = {}): RecommendedSkill {
  return {
    name: 'tdd',
    reason: '先写测试再写实现',
    installed: true,
    declared_in: [],
    declared_here: false,
    ...over,
  };
}

function stage(...skills: RecommendedSkill[]): RecommendedStage[] {
  return [{ stage: 'develop', skills }];
}

function renderPanel(stages: RecommendedStage[], oninstall = vi.fn()) {
  render(StageRecommendations, { props: { stages, busy: null, preview: null, oninstall } });
  return oninstall;
}

describe('推荐面板：三态与启用钮', () => {
  it('未装 → 给「安装」，没有「启用」', () => {
    renderPanel(stage(skill({ name: 'domain-modeling', installed: false })));

    expect(screen.getByRole('button', { name: '安装' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: '启用' })).toBeNull();
    expect(screen.getByText('未安装')).toBeTruthy();
  });

  it('已装但本阶段没启用 → 给「启用」（本票的落点：此前一枚钮都没有）', () => {
    renderPanel(
      stage(
        skill({
          installed: true,
          declared_here: false,
          declared_in: ['阶段 test-design'],
        }),
      ),
    );

    expect(screen.getByRole('button', { name: '启用' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: '安装' })).toBeNull();
  });

  it('已装且本阶段已启用 → 只读标签，两枚钮都没有', () => {
    renderPanel(stage(skill({ installed: true, declared_here: true, declared_in: ['阶段 develop'] })));

    expect(screen.queryByRole('button', { name: '启用' })).toBeNull();
    expect(screen.queryByRole('button', { name: '安装' })).toBeNull();
    expect(screen.getByText(/已启用：阶段 develop/)).toBeTruthy();
  });

  it('别处被引用时说的是「已被引用」，不是「已启用」（同一行上不能一边说已启用一边给启用钮）', () => {
    renderPanel(
      stage(
        skill({
          installed: true,
          declared_here: false,
          declared_in: ['阶段 test-design'],
        }),
      ),
    );

    expect(screen.getByText(/已被引用：阶段 test-design/)).toBeTruthy();
    expect(screen.queryByText(/^已启用/)).toBeNull();
  });

  it('点「启用」把 (本阶段, 技能名) 交给调用点——装与启用打的是同一个回调', async () => {
    const oninstall = renderPanel(
      stage(
        skill({
          installed: true,
          declared_here: false,
          declared_in: ['阶段 test-design'],
        }),
      ),
    );

    await fireEvent.click(screen.getByRole('button', { name: '启用' }));

    expect(oninstall).toHaveBeenCalledTimes(1);
    expect(oninstall.mock.calls[0]).toEqual(['develop', 'tdd']);
  });

  it('忙的时候两颗钮都禁用（一次只放一个动作过去）', () => {
    render(StageRecommendations, {
      props: {
        stages: stage(
          skill({ name: 'tdd', installed: false }),
          skill({ name: 'code-review', installed: true, declared_here: false }),
        ),
        busy: 'develop:tdd',
        preview: null,
        oninstall: vi.fn(),
      },
    });

    expect(screen.getByRole('button', { name: '安装' }).hasAttribute('disabled')).toBe(true);
    expect(screen.getByRole('button', { name: '启用' }).hasAttribute('disabled')).toBe(true);
  });
});
