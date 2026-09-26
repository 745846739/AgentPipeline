import { describe, expect, it } from 'vitest';

import { maskPairValue, pairingLaunchNote, pairingLaunchVerdict } from './pairingLaunch';

/**
 * 钉的是**一次排障**（决策 285）：用户报「手机上添加到主屏幕的书签没有带上 pair」，
 * 而这个事实在 standalone 窗口里本来无处可见（没有地址栏）。这一行把它变成可读的，
 * 所以判据要准——三种来源对应三条不同的修法，判混了就会把人送去重扫一个已经对的码。
 */
describe('pairingLaunch', () => {
  const bare = 'http://192.168.2.9:8788/#/talk?session=abc';
  const paired = 'http://192.168.2.9:8788/?pair=TOKEN#/talk?session=abc';

  it('地址里带着令牌 → url（服务端不认，说明令牌旧了或已重置）', () => {
    expect(pairingLaunchVerdict(paired, null)).toBe('url');
    // 与「本机也存着一份」并列时，地址那条优先——它是这次启动真正递进来的东西。
    expect(pairingLaunchVerdict(paired, 'T0KEN')).toBe('url');
  });

  it('地址里没有、本机存着 → stored（那份旧了）', () => {
    expect(pairingLaunchVerdict(bare, 'T0KEN')).toBe('stored');
  });

  it('两处都没有 → none（图标是从裸地址添加的）', () => {
    expect(pairingLaunchVerdict(bare, null)).toBe('none');
    expect(pairingLaunchVerdict(bare, '')).toBe('none');
  });

  it('空值不算令牌：`?pair=` 与 `#` 之后的 `pair=` 都不作数', () => {
    expect(pairingLaunchVerdict('http://h:1/?pair=', null)).toBe('none');
    expect(pairingLaunchVerdict('http://h:1/?pair=#/talk', null)).toBe('none');
    expect(pairingLaunchVerdict('http://h:1/#/talk?pair=TOKEN', null)).toBe('none');
  });

  it('打码只动 pair 的值，其余地址原样', () => {
    expect(maskPairValue(paired)).toBe('http://192.168.2.9:8788/?pair=•••#/talk?session=abc');
    expect(maskPairValue('http://h:1/?pair=a%2Fb&amp=1')).toBe('http://h:1/?pair=•••&amp=1');
    expect(maskPairValue(bare)).toBe(bare);
  });

  it('三句话各不相同，且都指向「该往哪修」', () => {
    const notes = [
      pairingLaunchNote(paired, null),
      pairingLaunchNote(bare, 'T0KEN'),
      pairingLaunchNote(bare, null),
    ];
    expect(new Set(notes).size, '三种来源必须给三句不同的话').toBe(3);
    expect(notes[0]).toContain('地址里带着');
    expect(notes[1]).toContain('本机存的那一份');
    expect(notes[2]).toContain('不带令牌的地址');
  });
});
