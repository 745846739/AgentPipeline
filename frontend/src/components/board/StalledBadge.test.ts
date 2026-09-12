import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import StalledBadge from './StalledBadge.svelte';

describe('StalledBadge（决策 34）', () => {
  it('超过 24h 显示天数', () => {
    render(StalledBadge, { props: { hours: 72 } });
    expect(screen.getByText('已滞留 3 天')).toBeTruthy();
  });

  it('不足 24h 显示小时（至少 1）', () => {
    render(StalledBadge, { props: { hours: 5 } });
    expect(screen.getByText('已滞留 5 小时')).toBeTruthy();
  });
});
