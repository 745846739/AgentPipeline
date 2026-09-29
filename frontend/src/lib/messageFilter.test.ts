import { describe, expect, it } from 'vitest';
import { messageMatches } from './messageFilter';

describe('消息的内容 / 角色过滤（spec list-windowing：会话页签）', () => {
  const user = { role: 'user' as const, content: '把窗口超时改成续接' };
  const assistant = { role: 'assistant' as const, content: '好的，我先读 scheduler' };
  const tool = { role: 'tool' as const, content: '' };

  it('空查询：全部放行', () => {
    expect(messageMatches(user, '')).toBe(true);
    expect(messageMatches(user, '   ')).toBe(true);
  });

  it('按内容匹配：大小写不敏感的子串', () => {
    expect(messageMatches(assistant, 'scheduler')).toBe(true);
    expect(messageMatches(assistant, 'SCHEDULER')).toBe(true);
    expect(messageMatches(assistant, '续接')).toBe(false);
    expect(messageMatches(user, '续接')).toBe(true);
  });

  it('按角色匹配：角色名在命中面里', () => {
    expect(messageMatches(tool, 'tool')).toBe(true);
    expect(messageMatches(user, 'user')).toBe(true);
    expect(messageMatches(assistant, 'user')).toBe(false);
  });

  it('content 为空 / null 的行：仍可按角色捞出', () => {
    expect(messageMatches({ role: 'tool', content: null }, 'tool')).toBe(true);
    expect(messageMatches({ role: 'tool', content: null }, '别的词')).toBe(false);
  });
});
