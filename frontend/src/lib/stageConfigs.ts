import type {
  SkillDeclaration,
  SkillMode,
  StageConfig,
  StageConfigPutPayload,
} from '../api/types';

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
  /** 结构化技能声明（票 15）：不再是自由文本，形态由 serializer 决定。 */
  skills: SkillDeclDraft[];
  idle_timeout_sec: string;
  max_duration_sec: string;
  /** pending → resume 时续接上一轮对话（决策 180）；默认 false。 */
  resume_continuation: boolean;
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
    skills: [],
    resume_continuation: false,
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
    skills: parseSkillDecls(config.skills_json).decls,
    idle_timeout_sec: config.idle_timeout_sec === null ? '' : String(config.idle_timeout_sec),
    max_duration_sec: config.max_duration_sec === null ? '' : String(config.max_duration_sec),
    resume_continuation: config.resume_continuation === true,
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

  // 技能声明：结构化 → 混合数组（票 05 的 `string | {name, mode, trusted}`）。
  // 沿用本表单唯一的规则「留空 = 省略 = 清空」：空列表与省略在整条替换下等价，
  // 多发一个 `[]` 只会让「用户到底有没有动过技能」更难从请求里读出来。
  const decls = serializeSkillDecls(draft.skills);
  if (decls.length > 0) payload.skills_json = decls;
  // 续接开关的默认就是关（后端 `resume_continuation` 缺省读出 `None`），故只在打开时下发
  if (draft.resume_continuation) payload.resume_continuation = true;

  const overrides = parseOptionalJson(draft.node_overrides_json, 'node_overrides_json');
  if ('error' in overrides) return { ok: false, error: overrides.error };
  if (overrides.value !== undefined) payload.node_overrides_json = overrides.value;

  return { ok: true, payload };
}

/* ────────────────── 技能声明（决策 172④，票 05 / 15）────────────────── */

/** 三个节点（crates/core/src/types.rs::ALL_NODES）。 */
export const SKILL_NODES = ['validate_input', 'execute', 'validate_output'] as const;
export type SkillNode = (typeof SKILL_NODES)[number];

/**
 * 界面上的一条技能声明。
 *
 * `bare` 记录它**原本**是不是裸字符串：裸字符串按 `{full, trusted:false}` 解释，
 * 而这样的对象形态会被后端写入门拒绝（未信任不得全文注入）。所以保存时必须原样写回裸字符串
 * ——这是「零迁移」在界面上的落点，不是实现细节。
 */
export interface SkillDeclDraft {
  name: string;
  mode: SkillMode;
  trusted: boolean;
  bare: boolean;
}

/**
 * 解析 `skills_json`（混合数组）为界面形态。
 *
 * 裸字符串 → `{mode: 'full', trusted: false, bare: true}`；对象缺省 `mode` 为 `full`、
 * `trusted` 为 `false`（与后端 `parse_skill_decls` 同口径）。既不是字符串也不是对象的
 * 元素按「不是声明」忽略——与后端一致，避免界面把 `42` / `null` 这类杂值放大成错误。
 */
export function parseSkillDecls(value: unknown): { decls: SkillDeclDraft[]; error?: string } {
  if (value === null || value === undefined) return { decls: [] };
  if (!Array.isArray(value)) {
    return { decls: [], error: 'skills_json 不是数组：请改成 `["技能名"]` 或对象数组。' };
  }
  const decls: SkillDeclDraft[] = [];
  for (const item of value) {
    if (typeof item === 'string') {
      decls.push({ name: item, mode: 'full', trusted: false, bare: true });
      continue;
    }
    if (typeof item !== 'object' || item === null || Array.isArray(item)) continue;
    const obj = item as Record<string, unknown>;
    const name = typeof obj.name === 'string' ? obj.name : null;
    if (name === null) continue;
    const rawMode = obj.mode;
    if (rawMode !== undefined && rawMode !== 'full' && rawMode !== 'name') {
      return { decls, error: `技能 ${name} 的 mode 非法：${String(rawMode)}（须为 full 或 name）。` };
    }
    decls.push({
      name,
      mode: rawMode === 'name' ? 'name' : 'full',
      trusted: obj.trusted === true,
      bare: false,
    });
  }
  return { decls };
}

/**
 * 序列化回 `skills_json` 的混合数组。
 *
 * 裸字符串形态**原样写回**（`full` + 未信任）：它若被物化成对象，就会撞上
 * 「未信任技能不得以全文模式保存」的写入门。其余一律写显式对象——这样 `trusted` 才有地方放。
 */
export function serializeSkillDecls(decls: SkillDeclDraft[]): SkillDeclaration[] {
  return decls.map((d) =>
    d.bare && d.mode === 'full' && !d.trusted
      ? d.name
      : { name: d.name, mode: d.mode, trusted: d.trusted },
  );
}

/** 未受信任的技能能不能切到全文模式（控件层拦，不依赖后端报错）。 */
export function canSwitchToFull(decl: SkillDeclDraft): boolean {
  return decl.trusted || decl.bare;
}

export type SkillEdit = { ok: true; decls: SkillDeclDraft[] } | { ok: false; error: string };

/** 切换某条的注入模式。未受信任 → 全文被拒，给出**可操作**的原因。 */
export function setSkillMode(decls: SkillDeclDraft[], index: number, mode: SkillMode): SkillEdit {
  const target = decls[index];
  if (!target) return { ok: false, error: '技能不存在。' };
  if (mode === 'full' && !canSwitchToFull(target)) {
    return {
      ok: false,
      error: `技能 ${target.name} 未受信任，不能注入全文（决策 172④）。请先确认信任此技能，或改用「仅注入名字」。`,
    };
  }
  const next = decls.map((d, i) => (i === index ? { ...d, mode, bare: false } : d));
  return { ok: true, decls: next };
}

/** 翻转某条的信任态。信任 = 物化成显式对象（裸字符串没地方放 `trusted`）。 */
export function setSkillTrust(decls: SkillDeclDraft[], index: number, trusted: boolean): SkillEdit {
  const target = decls[index];
  if (!target) return { ok: false, error: '技能不存在。' };
  if (!trusted && target.mode === 'full' && !target.bare) {
    return {
      ok: false,
      error:
        `${target.name} 正以全文模式注入，撤销信任会让这份配置失效。` +
        `请先把它切成「仅注入名字」，再撤销信任。`,
    };
  }
  const next = decls.map((d, i) => (i === index ? { ...d, trusted, bare: false } : d));
  return { ok: true, decls: next };
}

/** 追加一条声明（已存在同名则不加，返回原列表）。 */
export function addSkillDecl(decls: SkillDeclDraft[], name: string): SkillDeclDraft[] {
  const trimmed = name.trim();
  if (!trimmed || decls.some((d) => d.name === trimmed)) return decls;
  // 新加的技能未受信任 → 只能先名字态（与后端一键安装同口径）
  return [...decls, { name: trimmed, mode: 'name', trusted: false, bare: false }];
}

export function removeSkillDecl(decls: SkillDeclDraft[], index: number): SkillDeclDraft[] {
  return decls.filter((_, i) => i !== index);
}

/**
 * 从 `node_overrides_json` 文本里取出某节点的技能声明（不改动原文）。
 *
 * 节点级技能与阶段级是**并集**（只增不减，§10.6.4），故两组控件各管各的、互不覆盖。
 */
export function nodeSkillsFromJson(
  raw: string,
  node: string,
): { decls: SkillDeclDraft[]; error?: string } {
  const text = raw.trim();
  if (!text) return { decls: [] };
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { decls: [], error: 'node_overrides_json 还不是合法 JSON。' };
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return { decls: [], error: 'node_overrides_json 须是对象。' };
  }
  const nodeObj = (parsed as Record<string, unknown>)[node];
  if (typeof nodeObj !== 'object' || nodeObj === null || Array.isArray(nodeObj)) {
    return { decls: [] };
  }
  return parseSkillDecls((nodeObj as Record<string, unknown>).skills);
}

/**
 * 把某节点的技能声明写回 `node_overrides_json` 文本（其余键与其余节点逐字保留）。
 *
 * 声明为空时**删掉** `skills` 键而不是留一个空数组：空数组在语义上等于「没有节点级技能」，
 * 但会被后端的并集计算当成一次无意义的声明，也让用户读到一堆 `"skills": []`。
 */
export function withNodeSkills(
  raw: string,
  node: string,
  decls: SkillDeclDraft[],
): { ok: true; text: string } | { ok: false; error: string } {
  const text = raw.trim();
  let parsed: Record<string, unknown> = {};
  if (text) {
    let value: unknown;
    try {
      value = JSON.parse(text);
    } catch {
      return { ok: false, error: 'node_overrides_json 不是合法 JSON，请先修好它再编辑节点级技能。' };
    }
    if (typeof value !== 'object' || value === null || Array.isArray(value)) {
      return { ok: false, error: 'node_overrides_json 须是对象。' };
    }
    parsed = value as Record<string, unknown>;
  }

  const existing = parsed[node];
  const nodeObj: Record<string, unknown> =
    typeof existing === 'object' && existing !== null && !Array.isArray(existing)
      ? { ...(existing as Record<string, unknown>) }
      : {};
  if (decls.length === 0) {
    delete nodeObj.skills;
  } else {
    nodeObj.skills = serializeSkillDecls(decls);
  }

  if (Object.keys(nodeObj).length === 0) {
    delete parsed[node];
  } else {
    parsed[node] = nodeObj;
  }
  return { ok: true, text: JSON.stringify(parsed, null, 2) };
}
