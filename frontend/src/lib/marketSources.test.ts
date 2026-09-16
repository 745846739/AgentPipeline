import { describe, expect, it } from 'vitest';

import { addSource, normalizeSource, removeSource, validateSource } from './marketSources';

describe('市场来源编辑器的判据（决策 187）', () => {
  it('归一：小写 + 去尾斜杠，其余原样（与后端 normalize_origin 同口径）', () => {
    expect(normalizeSource('  HTTPS://Skills.Example.com/  ')).toBe('https://skills.example.com');
    // 默认端口**保留**：后端也不抹，前端抹了会让「重复判定」两处不一致
    expect(normalizeSource('https://skills.example.com:443')).toBe('https://skills.example.com:443');
    expect(normalizeSource('http://127.0.0.1:8787/')).toBe('http://127.0.0.1:8787');
  });

  it('接受合法 origin（https 任意主机 / http 仅回环）', () => {
    expect(validateSource('https://skills.example.com')).toBe(null);
    expect(validateSource('https://skills.example.com:8443')).toBe(null);
    expect(validateSource('http://127.0.0.1:8787')).toBe(null);
    expect(validateSource('http://localhost:8787')).toBe(null);
    expect(validateSource('http://[::1]:8787')).toBe(null);
  });

  it('拒绝非回环的明文 http（决策 177③）', () => {
    // 明文 http 上 sha256 挡不住同时替换索引与包的中间人
    expect(validateSource('http://skills.example.com')).toMatch(/https/);
    expect(validateSource('http://192.168.1.10:8787')).toMatch(/https/);
  });

  it('只接受 origin：带路径 / 查询 / 片段都不行', () => {
    expect(validateSource('https://skills.example.com/path')).toMatch(/origin/);
    expect(validateSource('https://skills.example.com?q=1')).toMatch(/origin/);
    expect(validateSource('https://skills.example.com/#x')).toMatch(/origin/);
  });

  it('拒绝畸形与空输入', () => {
    expect(validateSource('')).toMatch(/请填写/);
    expect(validateSource('   ')).toMatch(/请填写/);
    expect(validateSource('skills.example.com')).toMatch(/origin/);
    expect(validateSource('ftp://skills.example.com')).toMatch(/origin/);
    expect(validateSource('https://user@skills.example.com')).toMatch(/主机名/);
    expect(validateSource('https://')).toMatch(/origin/);
  });

  it('追加：归一后写入，重复（含写法不同的同一个 origin）拒绝', () => {
    expect(addSource([], ' HTTPS://A.Example.com/ ')).toEqual(['https://a.example.com']);
    expect(addSource(['https://a.example.com'], 'HTTPS://A.Example.com/')).toBe(null);
    expect(addSource(['https://a.example.com'], 'https://b.example.com')).toEqual([
      'https://a.example.com',
      'https://b.example.com',
    ]);
  });

  it('追加非法项 → null（调用方据此提示，不静默丢弃）', () => {
    expect(addSource([], 'http://skills.example.com')).toBe(null);
    expect(addSource([], 'not-a-url')).toBe(null);
    expect(addSource([], '')).toBe(null);
  });

  it('移除只拿掉那一条，顺序与其余项不动', () => {
    const list = ['https://a.example.com', 'https://b.example.com'];
    expect(removeSource(list, 'https://a.example.com')).toEqual(['https://b.example.com']);
    expect(removeSource(list, 'https://nope.example.com')).toEqual(list);
  });
});
