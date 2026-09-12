import type { StageConfig, StageConfigPutPayload } from '../api/types';

/**
 * stage_configs 编辑的纯逻辑（决策 22 / 46 / 66 / 111 / 129）。
 *
 * 后端 `PUT /stage-configs/{stage}` 是**整条替换**：请求体里省略的字段被清空为默认。
 * 因此草稿 → payload 的唯一规则是「留空 = 省略 = 清空」；这里不做「保持原值」的合并。
 * 非法数字 / 非法 JSON 在提交前拦下并返回可读原因，不把坏值发给后端。
 */

/** 10 个真实阶段 + 3 个伪阶段键（routes/stage_configs.rs::PSEUDO_STAGE_KEYS）。 */
export const STAGE_KEYS = [
  'init',
  'architect-design',
  'develop-design',
  'test-design',
  'sync-check',
  'develop',
  'review',
  'test',
  'merge',
  'done',
  'conflict_check',
  'validator_cross_check',
  'project_analysis',
] as const;
export type StageKey = (typeof STAGE_KEYS)[number];

const PSEUDO_KEYS = new Set(['conflict_check', 'validator_cross_check', 'project_analysis']);

export function isPseudoStage(stage: string): boolean {
  return PSEUDO_KEYS.has(stage);
}

/** 阶段键的界面标签（伪阶段加备注）。 */
export function stageKeyLabel(stage: string): string {
  return isPseudoStage(stage) ? `${stage}（伪阶段）` : stage;
}

/** 表单草稿：所有输入都是字符串（数字 / JSON 字段在提交时解析）。 */
export interface StageConfigDraft {
  stage: string;
  provider_id: string;
  temperature: string;
  max_tokens: string;
  persona_path: string;
  persona_append: string;
  tools_json: string;
  skills_json: string;
  idle_timeout_sec: string;
  max_duration_sec: string;
  node_overrides_json: string;
}

export function emptyStageConfigDraft(stage: string = STAGE_KEYS[0]): StageConfigDraft {
  return {
    stage,
    provider_id: '',
    temperature: '',
    max_tokens: '',
    persona_path: '',
    persona_append: '',
    tools_json: '',
    skills_json: '',
    idle_timeout_sec: '',
    max_duration_sec: '',
    node_overrides_json: '',
  };
}

/** 用现有行预填（编辑态）：null → 空串；JSON 值美化打印。 */
export function draftFromStageConfig(config: StageConfig): StageConfigDraft {
  return {
    stage: config.stage,
    provider_id: config.provider_id ?? '',
    temperature: config.temperature === null ? '' : String(config.temperature),
    max_tokens: config.max_tokens === null ? '' : String(config.max_tokens),
    persona_path: config.persona_path ?? '',
    persona_append: config.persona_append ?? '',
    tools_json: stringifyJson(config.tools_json),
    skills_json: stringifyJson(config.skills_json),
    idle_timeout_sec: config.idle_timeout_sec === null ? '' : String(config.idle_timeout_sec),
    max_duration_sec: config.max_duration_sec === null ? '' : String(config.max_duration_sec),
    node_overrides_json: stringifyJson(config.node_overrides_json),
  };
}

function stringifyJson(value: unknown | null): string {
  if (value === null || value === undefined) return '';
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return '';
  }
}

export type StageConfigPutResult =
  | { ok: true; payload: StageConfigPutPayload }
  | { ok: false; error: string };

type Parsed<T> = { value: T } | { value: undefined } | { error: string };

function parseOptionalNumber(raw: string, label: string, integer: boolean): Parsed<number> {
  const text = raw.trim();
  if (!text) return { value: undefined };
  const n = Number(text);
  if (!Number.isFinite(n)) return { error: `${label} 必须是数字。` };
  if (integer && !Number.isInteger(n)) return { error: `${label} 必须是整数。` };
  return { value: n };
}

function parseOptionalJson(raw: string, label: string): Parsed<unknown> {
  const text = raw.trim();
  if (!text) return { value: undefined };
  try {
    return { value: JSON.parse(text) };
  } catch {
    return { error: `${label} 不是合法 JSON。` };
  }
}

/**
 * 草稿 → PUT payload。留空字段被省略（= 后端清空为默认）。
 * 任意字段非法时返回 `{ ok: false, error }`，不产生半成品 payload。
 */
export function buildStageConfigPut(draft: StageConfigDraft): StageConfigPutResult {
  const payload: StageConfigPutPayload = {};

  const providerId = draft.provider_id.trim();
  if (providerId) payload.provider_id = providerId;

  const personaPath = draft.persona_path.trim();
  if (personaPath) payload.persona_path = personaPath;

  const personaAppend = draft.persona_append.trim();
  if (personaAppend) payload.persona_append = personaAppend;

  const temperature = parseOptionalNumber(draft.temperature, 'temperature', false);
  if ('error' in temperature) return { ok: false, error: temperature.error };
  if (temperature.value !== undefined) payload.temperature = temperature.value;

  const maxTokens = parseOptionalNumber(draft.max_tokens, 'max_tokens', true);
  if ('error' in maxTokens) return { ok: false, error: maxTokens.error };
  if (maxTokens.value !== undefined) {
    if (maxTokens.value <= 0) return { ok: false, error: 'max_tokens 必须为正整数。' };
    payload.max_tokens = maxTokens.value;
  }

  const idle = parseOptionalNumber(draft.idle_timeout_sec, 'idle_timeout_sec', true);
  if ('error' in idle) return { ok: false, error: idle.error };
  if (idle.value !== undefined) {
    if (idle.value < 0) return { ok: false, error: 'idle_timeout_sec 不能为负。' };
    payload.idle_timeout_sec = idle.value;
  }

  const maxDuration = parseOptionalNumber(draft.max_duration_sec, 'max_duration_sec', true);
  if ('error' in maxDuration) return { ok: false, error: maxDuration.error };
  if (maxDuration.value !== undefined) {
    if (maxDuration.value < 0) return { ok: false, error: 'max_duration_sec 不能为负。' };
    payload.max_duration_sec = maxDuration.value;
  }

  const tools = parseOptionalJson(draft.tools_json, 'tools_json');
  if ('error' in tools) return { ok: false, error: tools.error };
  if (tools.value !== undefined) payload.tools_json = tools.value;

  const skills = parseOptionalJson(draft.skills_json, 'skills_json');
  if ('error' in skills) return { ok: false, error: skills.error };
  if (skills.value !== undefined) payload.skills_json = skills.value;

  const overrides = parseOptionalJson(draft.node_overrides_json, 'node_overrides_json');
  if ('error' in overrides) return { ok: false, error: overrides.error };
  if (overrides.value !== undefined) payload.node_overrides_json = overrides.value;

  return { ok: true, payload };
}
