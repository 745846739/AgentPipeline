import { describe, expect, it } from 'vitest';

import { statusCode } from './pipeline';

/**
 * 移动款标题行状态短码（§5 移动原型 `.bar-row`）。
 *
 * 钉住两件事：① 原型明写的 `WAIT` / `RUN` 两码逐字不变；② 其余状态取大写而不是
 * 又一套自造词——短码只在移动款标题行出现，完整状态句在下一行 `.dmeta` 里。
 */
describe('移动款标题行状态短码（§5 移动款）', () => {
  it('原型明写的两码逐字不变', () => {
    expect(statusCode('pending')).toBe('WAIT');
    expect(statusCode('running')).toBe('RUN');
  });

  it('其余状态取状态名大写，不自造词表', () => {
    expect(statusCode('queued')).toBe('QUEUED');
    expect(statusCode('waiting')).toBe('WAITING');
    expect(statusCode('done')).toBe('DONE');
    expect(statusCode('failed')).toBe('FAILED');
    expect(statusCode('cancelled')).toBe('CANCELLED');
  });
});
