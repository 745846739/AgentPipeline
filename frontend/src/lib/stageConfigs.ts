import type {
  EnvMode,
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

/** 10 个真实阶段 + 4 个非阶段配置键（routes/stage_configs.rs::PSEUDO_STAGE_KEYS）。 */
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
  'foreman',
] as const;
export type StageKey = (typeof STAGE_KEYS)[number];

/**
 * 值班长的阶段键（`crates/core/src/pipeline/foreman.rs::FOREMAN_STAGE_KEY` 的前端镜像）。
 *
 * 界面有两处需要**只对那一行**成立的东西（`env_mode` 的 `ask` 档、`max_rounds` 那一格），
 * 而在这之前它是散落的字面量 `'foreman'`——两处各写一遍就会与后端那个键漂移，
 * 而漂移的形状是「格子不见了」，不是报错。
 */
export const FOREMAN_STAGE_KEY: StageKey = 'foreman';

/**
 * 不是真实阶段的配置键。`foreman`（值班长，决策 182①）与三个伪阶段并列，
 * 但它**不是伪阶段**——它不在流水线里，是任务无关的对话角色；
 * 归在这一组只是为了复用同一个「加备注」的渲染。
 */
const PSEUDO_KEYS = new Set([
  'conflict_check',
  'validator_cross_check',
  'project_analysis',
  'foreman',
]);

export function isPseudoStage(stage: string): boolean {
  return PSEUDO_KEYS.has(stage);
}

/**
 * 这个阶段能用 `ask` 档吗（决策 206；`run-command-permissions` 规格 §4）。
 *
 * **只有值班长能用**：`ask` 的载体是「等人按那颗确认钮」，而流水线节点无人值守、也没有提议
 * 通道——在那里配 `ask` 的结果是静默收掉这个阶段全部的环境写动作（一条 develop 会卡在
 * 「写不了文件」上，而配置看上去只是一行 `ask`）。要收紧就配 `deny`。
 *
 * 判据与后端 `types::stage_may_use_ask` **同源**（后端在 `PUT /stage-configs` 上把关），
 * 这里只是不把那个选项摆出来——摆出来再报错等于让人白填一次。
 */
export function stageMayUseAsk(stage: string): boolean {
  return stage === FOREMAN_STAGE_KEY;
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
  node_overrides_json: string;
  /**
   * 值班长一轮的轮数上限（决策 233① / 239）。空串 = 没配过（缺省 300）。
   *
   * 只对 `foreman` 那一行有意义，而表单是逐阶段通用的，故它在其它阶段上只是「没填」。
   */
  max_rounds: string;
  /**
   * 值守轮一轮的生成 token 预算（决策 292 / 票 07）。空串 = 没配过（缺省 120000）。
   *
   * 与 `max_rounds` 同一条纪律（只对 `foreman` 有意义、只收正整数），差别在语义：
   * 它**只对值守轮是硬界**——人的那一轮没有硬界，同一条线在那里只落一条软告警。
   */
  watch_token_budget: string;
  /** 环境层档位（决策 206）。空串 = 没配过（用全局默认 / 该阶段的缺省）。 */
  env_mode: EnvMode | '';
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
    idle_timeout_sec: '',
    max_duration_sec: '',
    node_overrides_json: '',
    max_rounds: '',
    watch_token_budget: '',
    env_mode: '',
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
    node_overrides_json: stringifyJson(config.node_overrides_json),
    max_rounds: config.max_rounds === null || config.max_rounds === undefined ? '' : String(config.max_rounds),
    watch_token_budget:
      config.watch_token_budget === null || config.watch_token_budget === undefined
        ? ''
        : String(config.watch_token_budget),
    env_mode: config.env_mode ?? '',
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
 * **例外（决策 261⑤）**：foreman 行的 `persona_append` 清空时显式下发 `""`——
 * 省略会被后端存成 NULL，下次启动把用户的「关」当成「没配过」重新播种点名。
 * 任意字段非法时返回 `{ ok: false, error }`，不产生半成品 payload。
 */
export function buildStageConfigPut(draft: StageConfigDraft): StageConfigPutResult {
  const payload: StageConfigPutPayload = {};

  const providerId = draft.provider_id.trim();
  if (providerId) payload.provider_id = providerId;

  const personaPath = draft.persona_path.trim();
  if (personaPath) payload.persona_path = personaPath;

  const personaAppend = draft.persona_append.trim();
  if (personaAppend || draft.stage === 'foreman') payload.persona_append = personaAppend;

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

  const overrides = parseOptionalJson(draft.node_overrides_json, 'node_overrides_json');
  if ('error' in overrides) return { ok: false, error: overrides.error };
  if (overrides.value !== undefined) payload.node_overrides_json = overrides.value;

  // 轮数上限（决策 233① / 239）：只收正整数，缺省（留空）不上送。
  // 「0 = 无上限」这一档不存在，故这里把它挡在界面这一层（后端也会拒，两处都要）——
  // 让填错的人在按下之前就看见原因，而不是拿到一句 400。
  const rounds = parseOptionalNumber(draft.max_rounds, 'max_rounds', true);
  if ('error' in rounds) return { ok: false, error: rounds.error };
  if (rounds.value !== undefined) {
    if (rounds.value <= 0) {
      return { ok: false, error: 'max_rounds 必须为正整数（没有「无上限」这一档）。' };
    }
    payload.max_rounds = rounds.value;
  }

  // token 预算（决策 292 / 票 07）：与轮数上限同一姿态——只收正整数，留空 = 省略
  // （整条替换下即「清成缺省 120000」）。「无预算」这一档不存在，故 0 / 负数在按下之前挡。
  const budget = parseOptionalNumber(draft.watch_token_budget, 'watch_token_budget', true);
  if ('error' in budget) return { ok: false, error: budget.error };
  if (budget.value !== undefined) {
    if (budget.value <= 0) {
      return { ok: false, error: 'watch_token_budget 必须为正整数（没有「无预算」这一档）。' };
    }
    payload.watch_token_budget = budget.value;
  }

  // 环境层档位（决策 206）：空串 = 不配置（留给全局默认 / 该阶段的缺省），
  // 与「整条替换」的其余字段同一条规则——留空即省略。
  if (draft.env_mode) payload.env_mode = draft.env_mode;

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
    // 决策 172④：未受信任的技能不得注入全文。文案不带编号（决策 199）。
    return {
      ok: false,
      error: `技能 ${target.name} 未受信任，不能注入全文。请先确认信任此技能，或改用「仅注入名字」。`,
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
