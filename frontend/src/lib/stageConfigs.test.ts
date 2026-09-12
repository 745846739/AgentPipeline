import { describe, expect, it } from 'vitest';
import type { StageConfig } from '../api/types';
import {
  STAGE_KEYS,
  draftFromStageConfig,
  emptyStageConfigDraft,
  isPseudoStage,
  stageKeyLabel,
} from './stageConfigs';
import { buildStageConfigPut } from './stageConfigs';

function config(overrides: Partial<StageConfig> = {}): StageConfig {
  return {
    stage: 'develop',
    provider_id: 'p1',
    temperature: 0.2,
    max_tokens: 4096,
    persona_path: 'prompts/develop/execute.md',
    persona_append: null,
    tools_json: null,
    skills_json: null,
    idle_timeout_sec: null,
    max_duration_sec: null,
    node_overrides_json: null,
    updated_at: '2026-09-12T00:00:00Z',
    ...overrides,
  };
}

describe('stage_configs 键与预填', () => {
  it('包含 10 个真实阶段 + 3 个伪阶段键', () => {
    expect(STAGE_KEYS).toHaveLength(13);
    expect(STAGE_KEYS).toContain('sync-check');
    expect(STAGE_KEYS).toContain('validator_cross_check');
    expect(isPseudoStage('project_analysis')).toBe(true);
    expect(isPseudoStage('develop')).toBe(false);
    expect(stageKeyLabel('conflict_check')).toContain('伪阶段');
  });

  it('draftFromStageConfig 把 null 变空串、JSON 值美化打印', () => {
    const draft = draftFromStageConfig(
      config({ tools_json: { execute: ['read_file'] }, persona_append: '注意' }),
    );
    expect(draft.provider_id).toBe('p1');
    expect(draft.temperature).toBe('0.2');
    expect(draft.persona_append).toBe('注意');
    expect(draft.tools_json).toBe(JSON.stringify({ execute: ['read_file'] }, null, 2));
    expect(draft.skills_json).toBe('');
  });
});

describe('buildStageConfigPut（整条替换：留空 = 省略 = 清空）', () => {
  it('全空草稿 → 空 payload（后端据此清空为默认）', () => {
    const result = buildStageConfigPut(emptyStageConfigDraft('review'));
    expect(result).toEqual({ ok: true, payload: {} });
  });

  it('只填的字段进入 payload，其余保持省略', () => {
    const draft = { ...emptyStageConfigDraft('develop'), provider_id: ' p9 ', temperature: '0.7' };
    const result = buildStageConfigPut(draft);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.payload.provider_id).toBe('p9');
    expect(result.payload.temperature).toBe(0.7);
    expect('max_tokens' in result.payload).toBe(false);
    expect('persona_path' in result.payload).toBe(false);
    expect('tools_json' in result.payload).toBe(false);
  });

  it('显式 JSON（含 null）原样下发', () => {
    const draft = {
      ...emptyStageConfigDraft('test'),
      tools_json: '{"execute":["read_file","run_command"]}',
      skills_json: 'null',
    };
    const result = buildStageConfigPut(draft);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.payload.tools_json).toEqual({ execute: ['read_file', 'run_command'] });
    // "null" 是显式值（与留空的 omitted 不同）
    expect(result.payload.skills_json).toBeNull();
  });

  it('整数 / 浮点字段按字段语义解析', () => {
    const draft = {
      ...emptyStageConfigDraft('merge'),
      max_tokens: '8192',
      idle_timeout_sec: '600',
      max_duration_sec: '3600',
      node_overrides_json: '{"execute":{"idle_timeout_sec":600}}',
    };
    const result = buildStageConfigPut(draft);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.payload.max_tokens).toBe(8192);
    expect(result.payload.idle_timeout_sec).toBe(600);
    expect(result.payload.max_duration_sec).toBe(3600);
    expect(result.payload.node_overrides_json).toEqual({ execute: { idle_timeout_sec: 600 } });
  });

  it('非法数字 / 非法 JSON 被拦下并给出可读原因', () => {
    const badNumber = buildStageConfigPut({ ...emptyStageConfigDraft(), temperature: 'abc' });
    expect(badNumber).toEqual({ ok: false, error: 'temperature 必须是数字。' });

    const badInt = buildStageConfigPut({ ...emptyStageConfigDraft(), max_tokens: '1.5' });
    expect(badInt).toEqual({ ok: false, error: 'max_tokens 必须是整数。' });

    const nonPositive = buildStageConfigPut({ ...emptyStageConfigDraft(), max_tokens: '0' });
    expect(nonPositive).toEqual({ ok: false, error: 'max_tokens 必须为正整数。' });

    const badJson = buildStageConfigPut({ ...emptyStageConfigDraft(), tools_json: '{oops}' });
    expect(badJson).toEqual({ ok: false, error: 'tools_json 不是合法 JSON。' });
  });

  it('非法值出现时不产生半成品 payload', () => {
    const result = buildStageConfigPut({
      ...emptyStageConfigDraft(),
      provider_id: 'p1',
      skills_json: '[',
    });
    expect('payload' in result).toBe(false);
  });
});
