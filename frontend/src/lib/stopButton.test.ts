/**
 * 停钮的阶段态（决策 294 / 票 09）。
 *
 * 三个态各有一个不同的错法（见 `stopButton.ts` 的模块注释），而组件那一侧只能靠渲染才看得见
 * ——这条纯函数是用例的落点：出现条件、按下之后、下一轮开头。
 */
import { describe, expect, it } from 'vitest';
import { stopButtonLabel, stopButtonState } from './stopButton';

describe('停钮只在这一轮在飞时出现', () => {
  it('没在飞就不渲染（点了也只会回一句「没有一轮在跑」）', () => {
    expect(stopButtonState({ inFlight: false, asked: false })).toBe('hidden');
    // 上一轮按过停、这一轮还没开始：**不许**带着上一轮的「正在停」——那会让新一轮一上来
    // 就是一颗按不动的钮。态由 `inFlight` 归零，与 `asked` 无关。
    expect(stopButtonState({ inFlight: false, asked: true })).toBe('hidden');
  });

  it('在飞时是一颗可按的「停」', () => {
    expect(stopButtonState({ inFlight: true, asked: false })).toBe('ready');
    expect(stopButtonLabel('ready')).toBe('停');
  });

  it('按下之后转「正在停」（等它收口，不许连点当没反应）', () => {
    expect(stopButtonState({ inFlight: true, asked: true })).toBe('stopping');
    expect(stopButtonLabel('stopping')).toBe('正在停…');
    // `hidden` 那一份是空串：不渲染时它也没有字（组件靠 `{#if}` 判，不从字反推）。
    expect(stopButtonLabel('hidden')).toBe('');
  });
});
