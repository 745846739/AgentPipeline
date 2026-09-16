import { describe, expect, it } from 'vitest';

import { addRepo, normalizeRepo, removeRepo, validateRepo } from './marketRepos';

describe('市场仓名单编辑器的判据（决策 194）', () => {
  it('归一：去粘贴前缀 / 去 .git 后缀 / 去尾斜杠，**不改大小写**', () => {
    expect(normalizeRepo('  HTTPS://GitHub.com/Obra/Superpowers/  ')).toBe('Obra/Superpowers');
    expect(normalizeRepo('https://github.com/Obra/Superpowers.git')).toBe('Obra/Superpowers');
    expect(normalizeRepo('github.com/obra/superpowers.git/')).toBe('obra/superpowers');
    expect(normalizeRepo('  obra/superpowers  ')).toBe('obra/superpowers');
    // 大小写是展示的一部分：改写它等于显示一个用户没输入过的仓名
    expect(normalizeRepo('obra/SuperPowers')).toBe('obra/SuperPowers');
    // 非 github 的主机不抹：留着让校验明确报「不要带 scheme」
    expect(normalizeRepo('https://gitlab.com/a/b')).toBe('https://gitlab.com/a/b');
  });

  it('接受合法的 owner/repo（含 . _ - 与大写）', () => {
    expect(validateRepo('obra/superpowers')).toBe(null);
    expect(validateRepo('Obra/Superpowers')).toBe(null);
    expect(validateRepo('mattpocock/skills')).toBe(null);
    expect(validateRepo('vercel-labs/agent-skills')).toBe(null);
    expect(validateRepo('a.b/c_d-e')).toBe(null);
    expect(validateRepo('https://github.com/obra/superpowers')).toBe(null);
  });

  it('拒绝带 scheme / 含 @（后端的 RepoId 判定同口径）', () => {
    expect(validateRepo('ftp://github.com/a/b')).toMatch(/scheme/);
    expect(validateRepo('git://github.com/a/b')).toMatch(/scheme/);
    expect(validateRepo('ssh://git@github.com/a/b')).toMatch(/scheme/);
    expect(validateRepo('git@github.com:a/b')).toMatch(/@/);
  });

  it('拒绝 .. 与多余的斜杠 / 空段 / 带路径', () => {
    expect(validateRepo('a/..')).toMatch(/\.\./);
    expect(validateRepo('../b')).toMatch(/\.\./);
    expect(validateRepo('a/b/c')).toMatch(/两段/);
    expect(validateRepo('a//b')).toMatch(/两段/);
    expect(validateRepo('a/')).toMatch(/两段/);
    expect(validateRepo('/b')).toMatch(/owner 与 repo/);
    expect(validateRepo('github.com/a/b/tree/main/skills/x')).toMatch(/两段/);
  });

  it('拒绝非 ASCII、以 . 或 - 开头的段、与其它非法字符', () => {
    expect(validateRepo('中文/仓')).toMatch(/ASCII/);
    expect(validateRepo('.hidden/b')).toMatch(/owner/);
    expect(validateRepo('a/-b')).toMatch(/repo/);
    expect(validateRepo('a/b c')).toMatch(/只能包含/);
    expect(validateRepo('a/b#c')).toMatch(/只能包含/);
  });

  it('拒绝空输入', () => {
    expect(validateRepo('')).toMatch(/请填写/);
    expect(validateRepo('   ')).toMatch(/请填写/);
  });

  it('追加：归一后写入；重复（含只有大小写不同的同一个仓）拒绝', () => {
    expect(addRepo([], ' https://github.com/Obra/Superpowers.git ')).toEqual(['Obra/Superpowers']);
    expect(addRepo(['Obra/Superpowers'], 'obra/superpowers')).toBe(null);
    expect(addRepo(['obra/superpowers'], 'Obra/Superpowers')).toBe(null);
    expect(addRepo(['obra/superpowers'], 'mattpocock/skills')).toEqual([
      'obra/superpowers',
      'mattpocock/skills',
    ]);
  });

  it('追加非法项 → null（调用方据此提示，不静默丢弃）', () => {
    expect(addRepo([], 'obra')).toBe(null);
    expect(addRepo([], 'git@github.com:obra/superpowers')).toBe(null);
    expect(addRepo([], '')).toBe(null);
  });

  it('移除只拿掉那一条，顺序与其余项不动', () => {
    const list = ['obra/superpowers', 'mattpocock/skills'];
    expect(removeRepo(list, 'obra/superpowers')).toEqual(['mattpocock/skills']);
    expect(removeRepo(list, 'nope/nope')).toEqual(list);
  });
});
