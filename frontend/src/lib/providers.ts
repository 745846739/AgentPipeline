import type {
  Provider,
  ProviderCreatePayload,
  ProviderPatchPayload,
  ProviderTestPayload,
} from '../api/types';

/**
 * provider 表单的纯逻辑（决策 103 / 111 / 112）。
 *
 * ── supported_adapters 的来源（决策 103）──
 * 后端 SystemBaseline.supported_adapters 目前尚未在 Rust 中落地（crates 里没有该常量），
 * 实际能分发的厂商见 `crates/core/src/agent/providers/mod.rs`（openai / deepseek / anthropic）。
 * 这里是与当前后端实现对齐的**前端常量**；后端补出 baseline/adapters 端点后应改为读取接口。
 */
export const SUPPORTED_ADAPTERS = ['openai', 'deepseek', 'anthropic'] as const;
export type SupportedAdapter = (typeof SUPPORTED_ADAPTERS)[number];

/** 读接口回显的 api_key 掩码（routes/providers.rs）。 */
export const API_KEY_MASK = '***';

export function isSupportedAdapter(vendor: string): boolean {
  return (SUPPORTED_ADAPTERS as readonly string[]).includes(vendor);
}

export function isApiKeyMask(value: string | null | undefined): boolean {
  return value === API_KEY_MASK;
}

/** 编辑态草稿：api_key 以读接口回显值（`***` 或空）预填。 */
export interface ProviderDraft {
  vendor: string;
  model: string;
  context_window: number;
  base_url: string;
  api_key: string;
  enabled: boolean;
}

export function emptyProviderDraft(): ProviderDraft {
  return {
    vendor: 'openai',
    model: '',
    context_window: 128_000,
    base_url: '',
    api_key: '',
    enabled: true,
  };
}

export function draftFromProvider(p: Provider): ProviderDraft {
  return {
    vendor: p.vendor,
    model: p.model,
    context_window: p.context_window,
    base_url: p.base_url ?? '',
    api_key: p.api_key ?? '',
    enabled: p.enabled,
  };
}

/** 表单校验；返回 null 表示通过。 */
export function validateProviderDraft(draft: ProviderDraft): string | null {
  if (!draft.vendor.trim()) return '请填写厂商（vendor）。';
  if (!draft.model.trim()) return '请填写模型名（model）。';
  if (!Number.isInteger(draft.context_window) || draft.context_window <= 0) {
    return 'context_window 必须是正整数。';
  }
  return null;
}

/**
 * 保存（PATCH）规则（决策 112）：
 * - 输入框仍是回显掩码 `***` 或为空 → **省略 api_key**，后端保持原值；
 * - 用户输入了新 key → 带上 api_key；**绝不把掩码当密钥回传**。
 */
export function buildProviderPatch(
  original: Provider,
  draft: ProviderDraft,
): ProviderPatchPayload {
  const patch: ProviderPatchPayload = {
    vendor: draft.vendor.trim(),
    model: draft.model.trim(),
    context_window: draft.context_window,
    base_url: draft.base_url.trim(),
    enabled: draft.enabled,
  };
  const key = draft.api_key.trim();
  if (key && !isApiKeyMask(key) && key !== (original.api_key ?? '')) {
    patch.api_key = key;
  }
  return patch;
}

/** 创建（POST）规则：只有用户真填了 key 才带 `api_key`。 */
export function buildProviderCreate(draft: ProviderDraft): ProviderCreatePayload {
  const payload: ProviderCreatePayload = {
    vendor: draft.vendor.trim(),
    model: draft.model.trim(),
    context_window: draft.context_window,
    enabled: draft.enabled,
  };
  const baseUrl = draft.base_url.trim();
  if (baseUrl) payload.base_url = baseUrl;
  const key = draft.api_key.trim();
  if (key && !isApiKeyMask(key)) payload.api_key = key;
  return payload;
}

/**
 * 测试连接（决策 160）请求体规则——与保存规则同源，掩码绝不回传真值：
 * - 编辑态（providerId 非空）且输入框仍是 `***` → 省略 api_key，后端按 id 沿用已存密钥；
 * - 新增态必须已填 key 才带上（探针端点对未命中 id 且缺 key 返回 400）。
 */
export function buildProviderTest(
  draft: ProviderDraft,
  providerId: string | null,
): ProviderTestPayload {
  const payload: ProviderTestPayload = {
    vendor: draft.vendor.trim(),
    model: draft.model.trim(),
  };
  if (providerId) payload.id = providerId;
  const key = draft.api_key.trim();
  if (key && !isApiKeyMask(key)) payload.api_key = key;
  const base = draft.base_url.trim();
  if (base) payload.base_url = base;
  return payload;
}
