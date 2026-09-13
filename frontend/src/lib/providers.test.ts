import { describe, expect, it } from 'vitest';
import type { Provider } from '../api/types';
import {
  API_KEY_MASK,
  buildProviderCreate,
  buildProviderPatch,
  buildProviderTest,
  draftFromProvider,
  emptyProviderDraft,
  isApiKeyMask,
  isSupportedAdapter,
  validateProviderDraft,
} from './providers';

function provider(overrides: Partial<Provider> = {}): Provider {
  return {
    id: 'p1',
    vendor: 'openai',
    model: 'gpt-4o',
    context_window: 128000,
    base_url: null,
    api_key: API_KEY_MASK,
    enabled: true,
    ...overrides,
  };
}

describe('provider 掩码保存规则（决策 112）', () => {
  it('未改动 api_key 时 PATCH 不带 api_key 字段', () => {
    const original = provider();
    const draft = draftFromProvider(original); // api_key 预填 `***`
    const patch = buildProviderPatch(original, draft);
    expect('api_key' in patch).toBe(false);
    expect(patch.vendor).toBe('openai');
    expect(patch.context_window).toBe(128000);
  });

  it('api_key 输入框为空时同样不回传', () => {
    const original = provider({ api_key: null });
    const draft = { ...draftFromProvider(original), api_key: '   ' };
    const patch = buildProviderPatch(original, draft);
    expect('api_key' in patch).toBe(false);
  });

  it('用户输入新 key 时才回传 api_key', () => {
    const original = provider();
    const draft = { ...draftFromProvider(original), api_key: 'sk-new-123' };
    const patch = buildProviderPatch(original, draft);
    expect(patch.api_key).toBe('sk-new-123');
  });

  it('显式键入掩码也不会被当成新密钥回传', () => {
    const original = provider();
    const draft = { ...draftFromProvider(original), api_key: API_KEY_MASK };
    expect('api_key' in buildProviderPatch(original, draft)).toBe(false);
  });

  it('创建时只有真填了 key 才带 api_key', () => {
    const draft = emptyProviderDraft();
    expect('api_key' in buildProviderCreate(draft)).toBe(false);
    const withKey = buildProviderCreate({ ...draft, api_key: 'sk-x', base_url: 'http://local/v1' });
    expect(withKey.api_key).toBe('sk-x');
    expect(withKey.base_url).toBe('http://local/v1');
    // 空 base_url / 掩码 key 不下发
    expect('base_url' in buildProviderCreate({ ...draft, base_url: '', api_key: API_KEY_MASK })).toBe(
      false,
    );
  });
});

describe('supported_adapters 判定（决策 103）', () => {
  it('识别代码支持的三个适配器', () => {
    expect(isSupportedAdapter('openai')).toBe(true);
    expect(isSupportedAdapter('deepseek')).toBe(true);
    expect(isSupportedAdapter('anthropic')).toBe(true);
  });

  it('升级后被移除 / 手工改库的厂商判为不受支持', () => {
    expect(isSupportedAdapter('gemini')).toBe(false);
    expect(isSupportedAdapter('')).toBe(false);
  });

  it('掩码判定', () => {
    expect(isApiKeyMask(API_KEY_MASK)).toBe(true);
    expect(isApiKeyMask('sk-real')).toBe(false);
    expect(isApiKeyMask(null)).toBe(false);
  });
});

describe('provider 表单校验', () => {
  it('vendor / model / context_window 必填且为正整数', () => {
    const base = { ...emptyProviderDraft(), model: 'gpt-4o' };
    expect(validateProviderDraft(base)).toBeNull();
    expect(validateProviderDraft({ ...base, vendor: ' ' })).toMatch(/厂商/);
    expect(validateProviderDraft({ ...base, model: '' })).toMatch(/模型/);
    expect(validateProviderDraft({ ...base, context_window: 0 })).toMatch(/正整数/);
    expect(validateProviderDraft({ ...base, context_window: 1.5 })).toMatch(/正整数/);
  });
});

describe('buildProviderTest（决策 160：测试连接）', () => {
  it('编辑态掩码不回传：省略 api_key，由后端按 id 沿用已存密钥', () => {
    const draft = { ...emptyProviderDraft(), api_key: API_KEY_MASK };
    const payload = buildProviderTest(draft, 'p-1');
    expect(payload.id).toBe('p-1');
    expect(payload.api_key).toBeUndefined();
    expect(payload.vendor).toBe('openai');
  });

  it('编辑态输入新 key 则覆盖；新增态不带 id', () => {
    const edited = buildProviderTest({ ...emptyProviderDraft(), api_key: 'sk-new' }, 'p-1');
    expect(edited.api_key).toBe('sk-new');
    const fresh = buildProviderTest({ ...emptyProviderDraft(), api_key: 'sk-new' }, null);
    expect(fresh.id).toBeUndefined();
    expect(fresh.api_key).toBe('sk-new');
  });

  it('base_url 空串省略', () => {
    const payload = buildProviderTest(emptyProviderDraft(), null);
    expect(payload.base_url).toBeUndefined();
  });
});
