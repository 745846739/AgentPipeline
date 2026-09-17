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
import {
  addSkillDecl,
  canSwitchToFull,
  nodeSkillsFromJson,
  parseSkillDecls,
  removeSkillDecl,
  serializeSkillDecls,
  setSkillMode,
  setSkillTrust,
  withNodeSkills,
} from './stageConfigs';

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
  it('包含 10 个真实阶段 + 4 个非阶段配置键', () => {
    // 第 4 个是值班长 `foreman`（决策 182①）：与三个伪阶段并列，但**不是伪阶段**
    // ——它不在流水线里，是任务无关的对话角色。后端 `PSEUDO_STAGE_KEYS` 必须同长。
    expect(STAGE_KEYS).toHaveLength(14);
    expect(STAGE_KEYS).toContain('sync-check');
    expect(STAGE_KEYS).toContain('validator_cross_check');
    expect(STAGE_KEYS).toContain('foreman');
    expect(isPseudoStage('project_analysis')).toBe(true);
    expect(isPseudoStage('foreman')).toBe(true);
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
    expect(draft.skills).toEqual([]);
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
      node_overrides_json: 'null',
    };
    const result = buildStageConfigPut(draft);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.payload.tools_json).toEqual({ execute: ['read_file', 'run_command'] });
    // "null" 是显式值（与留空的 omitted 不同）
    expect(result.payload.node_overrides_json).toBeNull();
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
      node_overrides_json: '[',
    });
    expect('payload' in result).toBe(false);
  });
});

/* ────────────────── 技能声明（决策 172④，票 15）────────────────── */

describe('技能声明：混合数组的读与写', () => {
  it('裸字符串按 full + 未信任解释，并标记为老格式', () => {
    const { decls, error } = parseSkillDecls(['rtk']);
    expect(error).toBeUndefined();
    expect(decls).toEqual([{ name: 'rtk', mode: 'full', trusted: false, bare: true }]);
  });

  it('对象形态缺省 mode 为 full、trusted 为 false（与后端 parse_skill_decls 同口径）', () => {
    const { decls } = parseSkillDecls([
      { name: 'a' },
      { name: 'b', mode: 'name' },
      { name: 'c', mode: 'name', trusted: true },
    ]);
    expect(decls.map((d) => [d.mode, d.trusted, d.bare])).toEqual([
      ['full', false, false],
      ['name', false, false],
      ['name', true, false],
    ]);
  });

  it('旧格式（纯字符串数组）回填后写回的是**同一份数组**——零迁移', () => {
    const original = ['grilling', 'tdk'];
    const { decls } = parseSkillDecls(original);
    expect(serializeSkillDecls(decls)).toEqual(original);
  });

  it('裸字符串不得被物化成对象：未信任 + full 会被写入门拒绝', () => {
    const { decls } = parseSkillDecls(['rtk']);
    expect(serializeSkillDecls(decls)).toEqual(['rtk']);
    // 信任之后才有地方放 trusted，此时物化成对象
    const trusted = setSkillTrust(decls, 0, true);
    expect(trusted.ok).toBe(true);
    if (!trusted.ok) return;
    expect(serializeSkillDecls(trusted.decls)).toEqual([
      { name: 'rtk', mode: 'full', trusted: true },
    ]);
  });

  it('既不是字符串也不是对象的元素被忽略（与后端宽松口径一致）', () => {
    expect(parseSkillDecls(['ok', 42, null, [], true]).decls).toEqual([
      { name: 'ok', mode: 'full', trusted: false, bare: true },
    ]);
  });

  it('非法输入给出可读原因', () => {
    expect(parseSkillDecls({ nope: 1 }).error).toContain('数组');
    expect(parseSkillDecls([{ name: 'x', mode: 'half' }]).error).toContain('half');
  });
});

describe('技能声明：控件层的准入', () => {
  const untrustedName = parseSkillDecls([{ name: 'x', mode: 'name' }]).decls;
  const trustedFull = parseSkillDecls([{ name: 'y', mode: 'full', trusted: true }]).decls;

  it('未受信任的技能不可切全文，报错要说清怎么办', () => {
    expect(canSwitchToFull(untrustedName[0])).toBe(false);
    const result = setSkillMode(untrustedName, 0, 'full');
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('未受信任');
    expect(result.error).toContain('信任');
  });

  it('已信任 / 老格式（裸字符串）可以切全文', () => {
    expect(canSwitchToFull(trustedFull[0])).toBe(true);
    expect(canSwitchToFull(parseSkillDecls(['legacy']).decls[0])).toBe(true);
    expect(setSkillMode(trustedFull, 0, 'name').ok).toBe(true);
  });

  it('撤销信任撞上全文注入时拒绝，不静默降级', () => {
    const result = setSkillTrust(trustedFull, 0, false);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toContain('全文');
  });

  it('名字态可以撤销信任（本来就合法）', () => {
    const result = setSkillTrust(untrustedName, 0, false);
    expect(result.ok).toBe(true);
  });

  it('添加 / 移除：重名不加，新增一律先名字态 + 未信任', () => {
    const added = addSkillDecl([], 'grilling');
    expect(added).toEqual([{ name: 'grilling', mode: 'name', trusted: false, bare: false }]);
    expect(addSkillDecl(added, 'grilling')).toBe(added);
    expect(addSkillDecl(added, '  ')).toBe(added);
    expect(removeSkillDecl(added, 0)).toEqual([]);
  });
});

describe('节点级技能：写回 node_overrides_json', () => {
  it('只动目标节点的 skills，其余键与其余节点逐字保留', () => {
    const raw = JSON.stringify({
      execute: { idle_timeout_sec: 600 },
      validate_input: { provider_id: 'long-ctx' },
    });
    const result = withNodeSkills(raw, 'execute', addSkillDecl([], 'grilling'));
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const parsed = JSON.parse(result.text);
    expect(parsed.execute.idle_timeout_sec).toBe(600);
    expect(parsed.execute.skills).toEqual([
      { name: 'grilling', mode: 'name', trusted: false },
    ]);
    expect(parsed.validate_input).toEqual({ provider_id: 'long-ctx' });
  });

  it('清空节点级技能时删掉 skills 键（空数组是无意义的声明）', () => {
    const raw = JSON.stringify({ execute: { skills: ['x'], idle_timeout_sec: 60 } });
    const result = withNodeSkills(raw, 'execute', []);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(JSON.parse(result.text)).toEqual({ execute: { idle_timeout_sec: 60 } });
  });

  it('节点上没有别的键时整条节点对象一并清掉', () => {
    const raw = JSON.stringify({ execute: { skills: ['x'] } });
    const result = withNodeSkills(raw, 'execute', []);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(JSON.parse(result.text)).toEqual({});
  });

  it('坏 JSON 不猜：拒绝写入并说清原因', () => {
    const result = withNodeSkills('{oops}', 'execute', []);
    expect(result.ok).toBe(false);
    expect(nodeSkillsFromJson('{oops}', 'execute').error).toBeTruthy();
  });

  it('读取节点级技能（含老格式裸字符串）', () => {
    const raw = JSON.stringify({ validate_input: { skills: ['grilling'] } });
    expect(nodeSkillsFromJson(raw, 'validate_input').decls).toEqual([
      { name: 'grilling', mode: 'full', trusted: false, bare: true },
    ]);
    expect(nodeSkillsFromJson(raw, 'execute').decls).toEqual([]);
  });
});

describe('草稿 → payload：技能声明', () => {
  it('技能以混合数组下发；空列表走「留空 = 省略」', () => {
    const empty = buildStageConfigPut(emptyStageConfigDraft());
    expect(empty.ok).toBe(true);
    if (!empty.ok) return;
    expect(empty.payload.skills_json).toBeUndefined();

    const draft = { ...emptyStageConfigDraft(), skills: addSkillDecl([], 'grilling') };
    const result = buildStageConfigPut(draft);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.payload.skills_json).toEqual([
      { name: 'grilling', mode: 'name', trusted: false },
    ]);
  });

  it('旧配置（纯字符串数组）经草稿一轮往返后形态不变', () => {
    const draft = draftFromStageConfig(config({ skills_json: ['a', 'b'] }));
    const result = buildStageConfigPut(draft);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.payload.skills_json).toEqual(['a', 'b']);
  });

  // 「续不续接上一轮对话」那条开关由决策 205 整层退场（改由后端的原因表决定），
  // 故它没有对应的用例了——不是漏了，是那个字段不存在了。
});
