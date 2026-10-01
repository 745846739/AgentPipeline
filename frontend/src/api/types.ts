/**
 * 前端 TS 类型 —— 与 docs/data-model.md §4 及 crates/core/src/types.rs、
 * crates/app/src/routes/*.rs 的**实际** JSON 序列化逐字段对齐。
 *
 * 注意（与 docs 的偏差，实现时以代码为准）：
 * - `GET /tasks` 的每条任务在 Task 之上附加 `branches`（游标摘要）与 `blocks`；
 *   `GET /tasks/{id}` 把 `cursors` / `allowed_actions` / `depends_on` / `blocks` 平铺在顶层。
 * - 游标摘要不含 task_id / created_at（见 routes/tasks.rs 的 `cursors_json`）。
 * - `messages_json` 是 OpenAI 风格的 Message[]（crates/core/src/agent/client.rs）。
 */

export type TaskStatus =
  | 'queued'
  | 'waiting'
  | 'running'
  | 'pending'
  | 'done'
  | 'failed'
  | 'cancelled';

/**
 * 流水线阶段的**成员表**（决策 252 / 253）。
 *
 * 成员集是**权威**而不是副本：`Stage` 联合由它推出来（`(typeof STAGE_MEMBERS)[number]`），
 * 故二者不可能漂——手抄两份联合与数组才会漂。它与后端 `crates/core/src/types.rs` 的
 * `Stage` 枚举由 `tests/fixtures/enum_members.json` 机器钉住
 * （`lib/enumMembersFixture.test.ts`，两者**集合相等**：多一个值也红）。
 *
 * 顺序 = 后端的声明序 = 流程图全序（`ALL_STAGES` 的注释：「阶段全序（流程图顺序）」）。
 * 展示用的格子顺序**不是**这一份——那是规格，走 `lib/stageConfigs.ts::STAGE_KEYS`。
 */
export const STAGE_MEMBERS = [
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
] as const;

export type Stage = (typeof STAGE_MEMBERS)[number];

export type Node = 'validate_input' | 'execute' | 'validate_output';

export type CursorStatus = 'active' | 'waiting_join' | 'pending' | 'archived';

/**
 * pending 原因的**成员表**（决策 253 ②）——与 {@link STAGE_MEMBERS} 同一姿态：
 * 成员集是权威，`PendingKind` 联合由它推出来，与后端 `types.rs::PendingKind` 由
 * `tests/fixtures/enum_members.json` 机器钉住。
 */
export const PENDING_KIND_MEMBERS = [
  'info_insufficient',
  'conflict_wait',
  'retry_exhausted',
  'user_decision',
  'merge_approval',
  'human_review',
  'dependency_failed',
  'context_overflow',
  'timeout',
  // 人自己按下的暂停（决策 276）：暂停中的任务与其它待办同一格（都是 pending），
  // 区别在原因那一栏与「不需要别人来管」这条豁免。
  'user_paused',
] as const;

export type PendingKind = (typeof PENDING_KIND_MEMBERS)[number];

export type ReviewMode = 'agent' | 'human';

export type TransitionTrigger =
  | 'normal'
  | 'retry'
  | 'node_retry'
  | 'kickback'
  | 'user_resume'
  | 'auto_resume'
  | 'timeout'
  | 'start';

export type CommandSource = 'agent' | 'system';

export interface PendingContext {
  /** duplicate_risk | dirty_worktree | test_code_issue | judge_disagreement | ... */
  kind?: string;
  /** conflict_wait 专用：全部冲突任务 id（决策 102）。 */
  conflict_task_ids?: string[];
  /** 闸门失败详情（决策 85）。 */
  gate_failure_output?: string;
  /** 原始诊断（主流程票 03）：provider 配置类失败时的原始错误串，供排查。 */
  diagnostic?: string;
  /** 其余自由字段平铺。 */
  [key: string]: unknown;
}

/** 决策 130：动作表 key = (type, context.kind)。 */
export interface PendingReason {
  type: PendingKind;
  stage: Stage;
  node: Node;
  message: string;
  suggested_actions?: string[];
  context?: PendingContext;
}

/**
 * 任务级托管（决策 210① / 票 08）。**关掉 = 这一列被清空**，不是 `enabled: false`——
 * 「从来没开过」与「开过又关了」在库里不该长得一样。
 */
export interface Stewardship {
  enabled: boolean;
  /** 已被值班长**自动** resume 过的次数（决策 210⑨ 的止损：满 `STEWARDSHIP_MAX_AUTO_RESUMES` 即停手）。 */
  auto_resumes: number;
  /** 上一次自动动手时的态势指纹（同一指纹不重复动手）。 */
  last_fingerprint: string | null;
  updated_at: string | null;
}

export interface Task {
  id: string;
  project_id: string;
  title: string;
  description: string;
  status: TaskStatus;
  /** 焦点游标投影（决策 80/92），只供看板展示与筛选。 */
  current_stage: Stage;
  current_node: Node;
  validate_attempts: number;
  pending_reason: PendingReason | null;
  /** 托管没开时为 `null`（决策 210①，端点回读同一列）。 */
  stewardship: Stewardship | null;
  worktree_path: string | null;
  branch_name: string | null;
  total_tokens: number;
  total_calls: number;
  review_mode: ReviewMode;
  model_override: string | null;
  archived_at: string | null;
  stalled: boolean;
  executor_owner: string | null;
  created_at: string;
  updated_at: string;
}

/** `cursors_json` 的形态（routes/tasks.rs）。 */
export interface BranchCursor {
  cursor_id: string;
  branch: string;
  stage: Stage;
  node: Node;
  status: CursorStatus;
  validate_attempts: number;
  skipped_to_join: boolean;
  pending_reason: PendingReason | null;
}

/** `GET /tasks` 列表项的附加字段。 */
export interface TaskListItem extends Task {
  branches: BranchCursor[];
  blocks: string[];
}

export type ActionKind = 'resume' | 'side_effect' | 'wait';

export interface ActionTarget {
  stage: Stage;
  node: Node;
  node_kind?: string;
}

/** 后端下发的动作项（crates/core/src/actions.rs）。前端纯渲染。 */
export interface AllowedAction {
  action: string;
  kind: ActionKind;
  label: string;
  cursor_id?: string;
  requires_input?: boolean;
  target?: ActionTarget;
}

export interface TaskDetail {
  task: Task;
  cursors: BranchCursor[];
  allowed_actions: AllowedAction[];
  depends_on: string[];
  blocks: string[];
}

export interface Transition {
  id: number;
  task_id: string;
  branch: string;
  from_stage: Stage | null;
  from_node: Node | null;
  to_stage: Stage;
  to_node: Node;
  trigger: TransitionTrigger;
  reason: string | null;
  created_at: string;
}

export interface FlowResponse {
  transitions: Transition[];
  cursors: BranchCursor[];
}

export interface ConversationSummary {
  run_id: number;
  stage: Stage;
  node: Node;
  attempt: number;
  agent_type: string;
  parent_run_id: number | null;
  prompt_tokens: number;
  completion_tokens: number;
  /** run 的台账状态（success / timeout / failed / …）——药丸过滤的「状态」维（票 03）。 */
  status: string;
}

export interface ToolCallWire {
  id: string;
  type: string;
  function: { name: string; arguments: string };
}

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | 'tool';
  content?: string | null;
  tool_calls?: ToolCallWire[];
  tool_call_id?: string | null;
  name?: string | null;
}

export interface NodeConversation {
  id: number;
  task_id: string;
  run_id: number;
  stage: Stage;
  node: Node;
  attempt: number;
  agent_type: string;
  parent_run_id: number | null;
  messages_json: ChatMessage[];
  metadata_json: unknown;
  prompt_tokens: number;
  completion_tokens: number;
  /** 组装后的**系统段**原文（决策 211②）；历史行 / 不调 LLM 的 run 为 null。 */
  system_prompt: string | null;
  /** 组装后**用户段**的原文（同上）。落库的 messages 里没有这两段。 */
  user_prompt: string | null;
  /**
   * 这次 run 全部调用的思考留痕（决策 360）：多次调用按到达序以空行相连。
   * 它不进 `messages_json`（决策 244：绝不回灌），只供现场时间线的思考步展示。
   */
  reasoning: string | null;
  created_at: string;
}

export interface NodeCommand {
  id: number;
  task_id: string;
  run_id: number | null;
  stage: Stage;
  node: Node;
  source: CommandSource;
  /** **实际执行的**命令串（脱敏后）。 */
  command: string;
  /**
   * 改写之前模型（或项目配置）原本写的那一条（决策 297）。
   *
   * `null` = 按原样跑。界面默认显示**原串**（那才是模型想要的东西），能展开看实际执行的串。
   */
  original_command: string | null;
  cwd: string;
  exit_code: number | null;
  stdout_path: string | null;
  stdout_preview: string | null;
  stderr_preview: string | null;
  duration_ms: number | null;
  started_at: string;
  finished_at: string | null;
}

export interface Project {
  id: string;
  name: string;
  local_path: string;
  default_branch: string;
  language: string | null;
  test_framework: string | null;
  lint_command: string | null;
  agents_md_path: string | null;
  created_at: string;
}

export type FileDiffStatus = 'added' | 'modified' | 'deleted';

export interface FileDiffDetail {
  path: string;
  additions: number;
  deletions: number;
  status: FileDiffStatus;
}

export interface DiffStats {
  files_changed: number;
  insertions: number;
  deletions: number;
  file_details: FileDiffDetail[];
}

export interface Provider {
  id: string;
  vendor: string;
  model: string;
  context_window: number;
  base_url: string | null;
  /** 读接口只回显 `***`（决策 112）。 */
  api_key: string | null;
  enabled: boolean;
}

export interface CreateTaskPayload {
  project_id: string;
  title: string;
  description?: string;
  depends_on?: string[];
  review_mode?: ReviewMode;
  model_override?: string;
}

export interface ResumePayload {
  action: string;
  cursor_id?: string;
  target_stage?: Stage;
  target_node?: Node;
  input?: string;
}

/* ─────────────── SSE 事件（crates/core/src/sse.rs，决策 76/84/123）─────────────── */

export type ToolPhase = 'start' | 'end' | 'error';

export interface SseBase {
  type: SseEventType;
  task_id: string;
  branch: string;
}

export type SseEventType =
  | 'node_started'
  | 'node_finished'
  | 'stage_changed'
  | 'cursor_changed'
  | 'pending'
  | 'pending_updated'
  | 'command_started'
  | 'command_output'
  | 'command_finished'
  | 'conversation_delta'
  | 'tool_event'
  | 'stalled'
  | 'task_cancelled'
  | 'task_done'
  | 'task_failed';

export interface NodeStartedEvent extends SseBase {
  type: 'node_started';
  stage: Stage;
  node: Node;
  attempt: number;
  run_id: number;
}
export interface NodeFinishedEvent extends SseBase {
  type: 'node_finished';
  stage: Stage;
  node: Node;
  attempt: number;
  run_id: number;
  status: string;
  duration_ms: number;
  prompt_tokens: number;
  completion_tokens: number;
}
export interface StageChangedEvent extends SseBase {
  type: 'stage_changed';
  from_stage: Stage | null;
  from_node: Node | null;
  to_stage: Stage;
  to_node: Node;
  trigger: string;
  reason: string | null;
}
export interface CursorChangedEvent extends SseBase {
  type: 'cursor_changed';
  cursor_id: string;
  status: string;
  stage: Stage;
  node: Node;
}
export interface PendingEvent extends SseBase {
  type: 'pending';
  cursor_id: string;
  reason: PendingReason;
}
export interface PendingUpdatedEvent extends SseBase {
  type: 'pending_updated';
  cursor_id: string;
  context: PendingContext;
}
export interface CommandStartedEvent extends SseBase {
  type: 'command_started';
  command_id: number;
  command: string;
  source: string;
}
export interface CommandOutputEvent extends SseBase {
  type: 'command_output';
  command_id: number;
  chunk: string;
}
export interface CommandFinishedEvent extends SseBase {
  type: 'command_finished';
  command_id: number;
  exit_code: number | null;
  duration_ms: number;
}
export interface ConversationDeltaEvent extends SseBase {
  type: 'conversation_delta';
  run_id: number;
  agent_type: string;
  /**
   * 归属班次（决策 204⑥）。流水线的增量为空串；**可选**是因为后端加了
   * `#[serde(default)]`——加字段是加性改动，老客户端读到的事件照旧能解析。
   */
  session_id?: string;
  /**
   * 这一段增量是**回话**还是**思考**（决策 244）。**可选**：老后端不发这个字段，
   * 缺省按 `content` 处理——那正是它此前唯一见过的形状。
   */
  channel?: 'content' | 'reasoning';
  role: string;
  text: string;
  prompt_tokens: number;
  completion_tokens: number;
  /**
   * 所属的**在途台账行**（票 02）。**可选**：流水线事件 / 老后端没有它，判据里
   * 「没有就按老路径接」的那一支说的就是它。
   */
  ledger_id?: number | null;
  /**
   * 该行内的**位置序号**（票 02）：与快照的 `seq0` 对账——`seq > seq0` 才接。
   * **只做去重，不做回放**（决策 275）：判据只决定到达的这条接不接，不触发任何补取。
   */
  seq?: number | null;
}
export interface ToolEventEvent extends SseBase {
  type: 'tool_event';
  run_id: number;
  /**
   * 发起这次调用的 agent 身份（决策 244）。**可选**：老后端不发这个字段，而缺省的空串
   * 不会等于 `"foreman"`——即「认不出来就当作别人的」，不会误收。
   */
  agent_type?: string;
  /**
   * 归属班次（决策 244）。对讲台据此丢弃别的班次的工具事件（与增量的班次守卫同一个
   * 判据，决策 204⑥）——手机与电脑同时连着时，两台设备不会看到对方的工具调用。
   */
  session_id?: string;
  tool: string;
  phase: ToolPhase;
  args_summary: string;
  /**
   * 完整参数原文与工具结果（决策 301）：截到 12k，给展开的工具详情。
   * `args` 老后端不发（缺省）；`result` 只在 end / error 相位带。
   */
  args?: string;
  result?: string;
  /** 所属在途台账行与行内位置序号（票 02，与增量同一条去重口径）。 */
  ledger_id?: number | null;
  seq?: number | null;
}
export interface StalledEvent extends SseBase {
  type: 'stalled';
  pending_hours: number;
}
export interface TaskCancelledEvent extends SseBase {
  type: 'task_cancelled';
}
export interface TaskDoneEvent extends SseBase {
  type: 'task_done';
}
export interface TaskFailedEvent extends SseBase {
  type: 'task_failed';
}

export type SseEvent =
  | NodeStartedEvent
  | NodeFinishedEvent
  | StageChangedEvent
  | CursorChangedEvent
  | PendingEvent
  | PendingUpdatedEvent
  | CommandStartedEvent
  | CommandOutputEvent
  | CommandFinishedEvent
  | ConversationDeltaEvent
  | ToolEventEvent
  | StalledEvent
  | TaskCancelledEvent
  | TaskDoneEvent
  | TaskFailedEvent;

/* ─────────────── 配置与指标（票 22，crates/app/src/routes/{providers,projects,tasks}.rs）─────────────── */

/** `POST /providers` 请求体（ProviderBody）。`id` 缺省由后端生成。 */
export interface ProviderCreatePayload {
  id?: string;
  vendor: string;
  model: string;
  context_window: number;
  base_url?: string;
  api_key?: string;
  enabled?: boolean;
}

/** `POST /providers/test` 请求体（决策 160）。`api_key` 缺省/掩码 = 后端按 id 沿用已存密钥。 */
export interface ProviderTestPayload {
  id?: string;
  vendor: string;
  model: string;
  base_url?: string;
  api_key?: string;
}

/** `POST /providers/test` 结果：**不含 api_key**（决策 112 掩码语义不因探针弱化）。 */
export interface ConnectionTestResult {
  ok: boolean;
  latency_ms: number;
  /** 失败时的可归因类别（llm_auth / …）；未知情形缺省。 */
  kind?: string;
  message: string;
  raw?: string;
}

/**
 * `PATCH /providers/{id}` 请求体（PatchProvider）。
 * 未提供的字段保持原值；`api_key` 传 `***` 后端也视为不修改（本前端更严格：直接省略）。
 */
export interface ProviderPatchPayload {
  vendor?: string;
  model?: string;
  context_window?: number;
  base_url?: string;
  api_key?: string;
  enabled?: boolean;
}

/** `POST /projects`（CreateProject）。`local_path` 创建后不可改（决策 29）。 */
export interface ProjectCreatePayload {
  name: string;
  local_path: string;
  default_branch?: string;
}

/** `PATCH /projects/{id}`（PatchProject）：面向前端可编辑字段。 */
export interface ProjectPatchPayload {
  name?: string;
  default_branch?: string;
  test_framework?: string;
  lint_command?: string;
}

/** `kanban_project_analyses.status`（catalog.rs：running → done | failed）。 */
export type AnalysisStatus = 'running' | 'done' | 'failed';

/** 分析结果 JSON（routes/projects.rs::analyze 的 spawn 结果体）。 */
export interface ProjectAnalysisResult {
  language: string | null;
  test_framework: string | null;
  lint_command: string | null;
  agents_md_path: string | null;
  has_gitignore: boolean;
  default_branch: string;
  suspicious: unknown[];
}

/** `GET /projects/{id}/analysis`。 */
export interface ProjectAnalysis {
  analysis_id: string;
  status: AnalysisStatus | string;
  result: ProjectAnalysisResult | null;
  error: string | null;
}

/** `POST /projects/analyze` → 202。 */
export interface AnalyzeResponse {
  analysis_id: string;
}

/** 阶段级聚合行（core metrics::StageMetric 的序列化形态）。 */
export interface StageMetric {
  stage: Stage;
  total_runs: number;
  avg_duration_ms: number;
  /** attempt > 1 的比例（0..1）。 */
  retry_rate: number;
}

/**
 * `GET /metrics`（routes/tasks.rs::global_metrics）。
 * `escape_events` 是 `Vec<(Option<String>, i64)>` → JSON 元组数组 `[stage|null, count]`。
 */
export interface GlobalMetrics {
  tasks: number;
  success_rate: number | null;
  stage_aggregation: StageMetric[];
  escape_events: Array<[string | null, number]>;
  /** 全局首过率：validate_output 首次 attempt 即通过的比例（`crates/core/src/metrics.rs` 口径）。 */
  validate_first_pass_rate?: number | null;
  /** 全量 run 行 token 求和（prompt + completion）。 */
  total_tokens?: number;
  /** 全部 LLM run 行数（不含 `agent_type = "system"`，决策 130②）。 */
  total_calls?: number;
}

/** `GET /tasks/{id}/metrics`（routes/tasks.rs::metrics）。 */
export interface TaskMetrics {
  total_tokens: number;
  total_calls: number;
  /** 任务表持久化的累计值；与实时求和可能短暂不一致（观测值）。 */
  stored_total_tokens: number;
  stored_total_calls: number;
  stages: StageMetric[];
  validate_first_pass_rate: number | null;
}

/* ─────────────── stage_configs（crates/app/src/routes/stage_configs.rs，票 22）─────────────── */

/**
 * 阶段级 agent 配置行（crates/core/src/types.rs::StageConfig）。
 * 字段是**扁平**的；未设置的 Option 序列化为 `null`（非省略）。
 */
export interface StageConfig {
  /** 真实阶段或伪阶段键（conflict_check / validator_cross_check / project_analysis）。 */
  stage: string;
  provider_id: string | null;
  temperature: number | null;
  max_tokens: number | null;
  persona_path: string | null;
  persona_append: string | null;
  tools_json: unknown | null;
  skills_json: unknown | null;
  idle_timeout_sec: number | null;
  max_duration_sec: number | null;
  node_overrides_json: unknown | null;
  /** 环境层权限档位（决策 206）。`null` = 没配过 → 用全局默认 / 该阶段的缺省。 */
  env_mode: EnvMode | null;
  /**
   * 值班长一轮的轮数上限（决策 233① / 239）：`null` = 没配过（缺省 300）。
   * 只收正整数——`0` 与「无上限」都不存在（后端会拒 400）。只对 `foreman` 那一行有意义。
   */
  max_rounds: number | null;
  /**
   * 值班长一轮的生成 token 预算（决策 292 / 票 07）：`null` = 没配过（缺省 120000）。
   * **只对值守轮是硬界**（人的那一轮只落软告警）；只收正整数——`0` 与「无预算」都不存在
   * （后端会拒 400）。只对 `foreman` 那一行有意义。
   */
  watch_token_budget: number | null;
  updated_at: string;
}

/**
 * 环境层权限档位（决策 206，`crates/core/src/types.rs::EnvMode`）。
 *
 * `auto` 直接执行（缺省，等于现状）/ `ask` 转成提议等值班经理按键 / `deny` 拒绝且不广告。
 * **只管环境层**（文件、命令、技能拉取、子代理）；本服务的写接口（建任务、拍板、合入……）
 * 恒为提议 + 确认钮，不受这个值影响。
 */
export type EnvMode = 'auto' | 'ask' | 'deny';

/**
 * `PUT /stage-configs/{stage}` 请求体（PutStageConfig）。
 * **整条替换**：省略的字段被清空为默认（不是「保持原值」）。
 */
export interface StageConfigPutPayload {
  provider_id?: string;
  temperature?: number;
  max_tokens?: number;
  persona_path?: string;
  persona_append?: string;
  tools_json?: unknown;
  skills_json?: unknown;
  idle_timeout_sec?: number;
  max_duration_sec?: number;
  node_overrides_json?: unknown;
  env_mode?: EnvMode;
  max_rounds?: number;
  watch_token_budget?: number;
}

/* ─────────────── server-info（crates/app/src/routes/server_info.rs，决策 167）─────────────── */

/** `GET /server-info` 的单个候选地址。 */
export interface ServerAddress {
  /** 网卡名（诊断用，如 `en0` / `utun3`）。 */
  interface: string;
  /** 手机可直接访问的完整 URL（含端口）。 */
  url: string;
  /** 是否被判定为大概率可连（分享页把首选放大显示）。 */
  preferred: boolean;
}

/** 绑定地址是谁定的（决策 186）：启动参数 / 界面上的开关 / 配置文件。 */
export type BindSource = 'startup' | 'settings' | 'config';

/**
 * 端口是谁给的（决策 213）：启动参数 / 配置文件 / **退让**。
 *
 * `fallback` = 首选端口被别的进程占着，当前这个端口是内核临时给的——它**重启后会变**，
 * 手机上存过的地址这次就是打不开的原因，故这一档必须由界面说出去。
 */
export type PortSource = 'startup' | 'config' | 'fallback';

/** `GET /server-info`：局域网分享所需的服务自述。 */
export interface ServerInfo {
  /** 实际绑定地址（`0.0.0.0` 表示全网卡）。 */
  host: string;
  port: number;
  /**
   * 仅绑定回环时为 true——**默认形态下**手机连不上，分享页需给出开启指引。
   *
   * 注意它是**绑定**的事实，不是「手机够不够得着」的判据：配了 `public_base_url`
   * （决策 334）时外面那道反代就是手机的门，判据收在 `lib/sharePairing.ts::phoneCanReach`。
   */
  loopback_only: boolean;
  /**
   * `host` 的来源（决策 186）。界面必须能说出「这颗钮按了重启还算不算数」：
   * `startup` = `--host` / `AGENTPIPELINE_LAN` 说了算，界面只改得动这一次；
   * `settings` = 界面上的开关（住 DB，重启仍生效）；`config` = `config.toml`。
   */
  bind_source: BindSource;
  /**
   * 端口的来源（决策 213）：`fallback` 表示首选端口被占用、当前端口是临时给的——
   * 手机上的旧书签会因此失效，见 `lib/sharePairing.ts::portFallbackNote`。
   */
  port_source: PortSource;
  /**
   * **手机实际访问的入口**（决策 334）：`[server] public_base_url` /
   * `--public-base-url` 给出的公网 origin；`null` = 没有这一层（手机直连本机 / 局域网）。
   *
   * 有它时后端往往只绑回环（外面是 Caddy / Nginx）、地址表里也只有它一项。
   */
  public_base_url: string | null;
  addresses: ServerAddress[];
}

/* ─────────────── 技能（crates/app/src/routes/skills.rs，决策 172④⑤ / 票 05 09 11 16）─────────────── */

/** 注入模式（`crates/core/src/agent/skills.rs::SkillMode`）。 */
export type SkillMode = 'full' | 'name';

/** 显式对象形态的技能声明。 */
export interface SkillDeclarationObject {
  name: string;
  mode: SkillMode;
  /** 未受信任的技能**不得**以 `full` 保存（票 05 的写入门）。 */
  trusted: boolean;
}

/**
 * `skills_json` 的元素形态（票 05 / 决策 172④）：`string | {name, mode, trusted}`。
 *
 * 裸字符串是信任概念出现之前的老写法，按 `{mode: "full", trusted: false}` 解释——
 * 写回去时必须保持裸字符串，否则会撞上「未信任 + full」的写入门。
 */
export type SkillDeclaration = string | SkillDeclarationObject;

/**
 * `GET /skills` 的一项。
 *
 * **`kind` 已退场**（决策 185）：技能只有技能根下的 markdown 一个来源，一个恒为
 * `"markdown"` 的判别位只会让人以为还有别的可能。
 */
export interface SkillSummary {
  name: string;
  description: string | null;
  /** 手动触发型：默认不自动注入（票 16 Notes 的硬约束）。 */
  disable_model_invocation: boolean;
  /** `{技能根}/{name}/SKILL.md` 的绝对路径。 */
  path: string;
  /** 被哪些阶段 / 节点引用（`"阶段 architect-design"` 这类可读串）。 */
  declared_in: string[];
}

/* ─────────────── 技能市场（crates/app/src/routes/market.rs，决策 194）─────────────── */

/** 仓名单是谁定的（继承决策 187 的两级结构）：界面保存的那一份，还是 `config.toml` 的 `[market]`。 */
export type MarketOrigin = 'settings' | 'config';

/** `GET / PUT / DELETE /market/repos`。 */
export interface MarketRepoConfig {
  /**
   * 生效的仓名单（界面 > 配置文件）；空表 = 不放行任何仓，看不到也装不上任何远程技能。
   *
   * `owner/repo` 原样展示（不小写）——它是**信任单元**，用户要认得出自己放行的是哪个仓。
   */
  repos: string[];
  origin: MarketOrigin;
  /**
   * 冷启动推荐名单（后端内置的公开技能仓）。
   *
   * **内置 ≠ 放行**：这些只是**字符串**，在用户点「添加」之前没有任何请求、一个字节都不下载
   * （票 03 的硬约束：Q6 选仓级白名单的代价曲线不能被便利性侵蚀）。
   */
  recommended: string[];
}

/** `GET /market/skills` 的一项技能：`dir` 是仓内该技能目录的路径，`name` 是它的 basename。 */
export interface MarketSkillRef {
  name: string;
  dir: string;
  description: string | null;
}

/**
 * 按技能目录的父路径分组（父路径是扫描时免费得到的，不读 `marketplace.json`）。
 *
 * `path` 是空串时表示技能直接住在仓根（界面把组标题写成「（根）」）。
 */
export interface MarketGroup {
  path: string;
  skills: MarketSkillRef[];
}

/**
 * `GET /market/skills?repo=owner/repo&q=…&refresh=1`。
 *
 * `commit` 是**列表当刻**的 tip，界面把它一路透传给安装——「看到的 = 装到的」全靠这一个字段
 * （否则锚会退化成「安装那一刻的 HEAD」，即 Pulumi 那个移动靶的形态）。
 * `listed_at` 是它被取到的时刻（不是 commit 的时间）。
 */
export interface MarketSkillList {
  repo: string;
  commit: string;
  commit_short: string;
  listed_at: string;
  groups: MarketGroup[];
}

/** 正文特征命中（票 11 第 ③ 项）：**具体到行**，不是布尔「有风险」。 */
export interface SkillFeatureHit {
  kind: 'run_command' | 'network' | 'credentials';
  label: string;
  /** 行号（1 起算，对着源文件能直接定位）。 */
  line: number;
  text: string;
}

/** `GET /skills/{name}/preview` 与 `POST /skills/install` 的 `preview` 字段。 */
export interface SkillPreview {
  name: string;
  /** ① 推荐去向。 */
  recommendations: { stage: string; reason: string }[];
  /** ② 注入模式与信任态。 */
  declarations: { declared_in: string; mode: SkillMode; trusted: boolean; bare: boolean }[];
  /** 尚未被任何配置引用时的默认形态说明。 */
  defaults: { mode: SkillMode; trusted: boolean; note: string | null };
  body_available: boolean;
  /** ③ 正文特征扫描（只用于告知，不参与准入）。 */
  features: { hits: SkillFeatureHit[]; counts: Record<string, number> };
  install?: { name: string; description: string | null; sibling_count: number };
}

/** `GET /skills/recommendations` 的一项推荐技能（票 16）。 */
export interface RecommendedSkill {
  name: string;
  reason: string;
  installed: boolean;
  /** 被谁引用（展示串，如「阶段 develop」/「阶段 develop 节点 execute」）。 */
  declared_in: string[];
  /**
   * 本阶段是否已声明它（票 16「已安装的可直接启用」，票 01）。
   *
   * 界面按它决定那一行给的是「启用」还是只读标签。**不要拿 `declared_in` 推算这一格**：
   * 那是给人看的展示串，按它判断阶段等于 parse 文案（后端另给这个布尔值就是为了这个）。
   */
  declared_here: boolean;
  /**
   * 这份推荐的来源定位（决策 194）：来源仓 `owner/repo` 与技能目录在仓内的相对路径。
   *
   * 清单是**指针不是名录**——没有 commit，它随上游漂移也不会变成一份陈旧的名录；commit 由技能
   * 市场在浏览那一刻补上。
   *
   * **两个字段同来同去**：后端从同一次清单查找里取出它们（`located.map(...)`），有就都有、
   * 没有就都是 `null`；而 `null` 那一支当前从本端点**不可达**（行的名字就取自这份清单，查找
   * 必然命中），类型与界面容得下它只是防御。
   */
  repo: string | null;
  dir: string | null;
}

/** 某阶段的推荐清单（票 16）。 */
export interface RecommendedStage {
  stage: string;
  skills: RecommendedSkill[];
}

/** `POST /skills/install` 的响应。 */
export interface OneClickInstallResult {
  skill: { name: string; description: string | null; sibling_count: number };
  stage_config: StageConfig;
  preview: SkillPreview;
}

/* ─────────────── 值班长 / 对讲台（crates/app/src/routes/foreman.rs，决策 182）─────────────── */

/** 该轮工具痕迹的一项（`ForemanTrace`）。`args_summary` 是参数摘要，原文在 `args`（决策 301）。 */
export interface ForemanTrace {
  tool: string;
  args_summary: string;
  /** 完整参数原文（截到 12k，决策 301）。老行没有这个字段 → 缺省，界面回落到摘要。 */
  args?: string;
  /** 工具结果 / 错误文本（同一上限）。老行同样缺省。 */
  result?: string;
  ok: boolean;
}

/**
 * 一轮里**按发生顺序**记下的一步（决策 273，`ForemanSegment` 的镜像）。
 *
 * 与 `ForemanTrace` / `ForemanMessage.thinking` 是同一件事的两种看法：那些是**聚合**
 * （工具查过什么、推理说了什么），这一份保留**顺序**——「先想 → 再查 → 然后说」。
 * 收口那一句是 `content` 列，**不在段序里**；`text` 段说的是这一轮**中途**说出口的话。
 *
 * 判据（哪一步是什么）由**后端**定：界面读 `kind`，不从正文里抠。
 */
export type ForemanSegment =
  | { kind: 'thinking'; text: string }
  | { kind: 'text'; text: string }
  | {
      kind: 'tool';
      tool: string;
      args_summary: string;
      /** 展开详情的原文与结果（决策 301）：老行缺省，界面回落到 `args_summary`。 */
      args?: string;
      result?: string;
      ok: boolean;
    };

/** 一个工具的回执标签（`GET /foreman/tools`，决策 247⑤）：界面上那个中文词。 */
export interface ForemanToolLabel {
  name: string;
  label: string;
}

/**
 * 全量工具清单的标签：与后端清单同序、**不按档位滤**——回执标的是历史上的工具
 * 调用，昨天 `auto` 今天 `deny`，昨天的回执仍要能翻译。
 */
export interface ForemanToolLabelList {
  tools: ForemanToolLabel[];
}

/**
 * 未消费待办的只读读数（决策 307，票 executor-never-returns 06）。
 *
 * `by_kind` 的键是待办类别的**稳定标识**（落库列，如 `owner_stuck` / `resume_blocked`），
 * 前端按它判、不按文案。`blocked_reads` 是阻塞池里卡住的读（决策 308，票 07）——
 * 与待办同一个来回取，因为两者都是「我该不该去看一眼」的读数。
 */
export interface ForemanAttention {
  open: number;
  by_kind: Record<string, number>;
  blocked_reads: {
    stuck_now: number;
    stuck_total: number;
    longest_wait_ms: number;
  };
}

/**
 * 结构化选项提问的载荷（决策 265，第三种轮型 `.turn.ask`）。
 *
 * 由**后端**随那一行 assistant 消息下发（`message_wire` 的 `ask` 字段，决策 252 同一支
 * 判定点）：界面拿它直接渲染选项钮，不从正文里抠。校验在工具执行点（2–4 个非空短语），
 * 故到达前端时这个形状一定完整可用。
 */
export interface ForemanAsk {
  question: string;
  options: string[];
}

/** 态势快照里等人拍板的一条：`message` 是 pending 原因**原文**，不是枚举名。 */
export interface ForemanBriefingPending {
  task_id: string;
  title: string;
  stage: string;
  kind: string;
  message: string;
}

export interface ForemanBriefingRunning {
  task_id: string;
  title: string;
  stage: string;
}

export interface ForemanBriefingFailed {
  task_id: string;
  title: string;
  stage: string;
  message: string | null;
}

export interface ForemanBriefingProject {
  id: string;
  name: string;
}

/**
 * 该轮注入 prompt 的夜班态势快照（审计用，不是界面数据源）。
 *
 * 界面据它把值班长引用的结论**标出来源工位**（`stage`）——快照里的读数与那一轮
 * 值班长看到的是同一份，故来源可考。
 */
export interface ForemanBriefing {
  pending: ForemanBriefingPending[];
  running: ForemanBriefingRunning[];
  failed: ForemanBriefingFailed[];
  projects: ForemanBriefingProject[];
  done_count: number;
}

/**
 * 一个班次（会话）。**决策 204**：一条长台账拆成一排可新建 / 切换 / 重命名 / 归档的班次。
 *
 * 归档 = 置 `archived_at`：从列表里收起来，**不物理删除**。消息**已豁免保留期**
 * （票 04，显式修订决策 204⑦「归档不保护消息」与 182④「同一把保留期尺」）：
 * 归档且超龄之后消息照样在——归档与否不再改变消息的去留。
 */
export interface ForemanSessionMeta {
  id: string;
  title: string;
  /**
   * 班次身份（决策 286 / 票 foreman-unbounded 01）：`talk`（人的班次）/
   * `watch`（值守台账——只读的一本账，票 04）。存量旧行由迁移 0030 落成 talk。
   */
  kind: string;
  created_at: string;
  last_active_at: string;
  archived_at: string | null;
}

/** 会话台账的一行。`role` 线上是 `"user" | "assistant"`（后端按串存）。 */
export interface ForemanMessage {
  id: number;
  /** 所属班次。会话隔离上下文，故它是每行的必填归属（决策 204②）。 */
  session_id: string;
  role: string;
  content: string;
  prompt_tokens: number;
  completion_tokens: number;
  briefing: ForemanBriefing | null;
  traces: ForemanTrace[] | null;
  /**
   * 这一轮**按发生顺序**的步骤（决策 273）。`null` = 没有任何一步（不产推理、无工具、
   * 只收口一句），或这一行写在那一列落地之前（老行——界面由两份聚合视图兜底）。
   *
   * 与 `traces` / `thinking` **并存**：聚合各服务自己的消费者，段序服务时间线的顺序。
   */
  segments?: ForemanSegment[] | null;
  /**
   * 这一轮的**推理 / 思考**原文（决策 244）。不产推理的模型是 `null`。
   *
   * **展示留痕，永不回灌**：它不是回话的一部分，后端也不会把它发回模型。
   * 界面把它收进一个折叠块——它常常很长（一段完整的推演），默认展开会把时间线冲垮。
   */
  thinking?: string | null;
  created_at: string;
  /**
   * 这一轮的**归因类别**（决策 235 / 238）：`host` / `pipeline` / `project_code` /
   * `prompt_config`，或 `unlocated`（没给结构块、或四类之外）。
   *
   * 由**后端解析**——结构块住在回话文本里，而解析点只有一处（`parse_attribution`）；
   * 界面照它显示类别标记，不自己从正文里抠。非助理轮（`user` / `system`）是 `null`。
   */
  attribution?: string | null;
  /** 归因类别的中文词（后端给的那一份）。未定位时为 `null`。 */
  attribution_label?: string | null;
  /** 未定位的原因（`missing` / `四类之外` / …）；定位成功时为 `null`。 */
  attribution_reason?: string | null;
  /**
   * **这一行是什么**（决策 252）：`mine`（值班经理的话）/ `console`（操作台记的一轮）/
   * `failed`（没跑起来的那一轮，**人的与值守轮的都算**——两者由 `proactive` 分开）/
   * `fm`（值班长的话）/ `ask`（提问轮，决策 265）。
   *
   * 由**后端判定**：这几个前缀是后端拼进正文的 / 字段是后端给的，界面此前靠 `startsWith`
   * 自己认——常量漂了只是症状，「正文即接口」才是病。认不出的 `role` 值落到 `fm`
   * （与 storage 层「非法值不打垮查询、前端按 `=== "user"` 判定」的原口径一致）。
   */
  kind: 'mine' | 'console' | 'failed' | 'fm' | 'ask';
  /**
   * 这一轮是不是**值班长自己醒来的那一轮**（值守播报，决策 209④；值守轮的失败账，决策 271）。
   *
   * 与 `kind` **正交**（决策 252③）：「自发的轮失败了」是可能的组合——那里 `kind = "failed"`
   * 且 `proactive = true`（播报要求助理轮，而失败账是 `system` 行）。压成一个枚举会让这个
   * 组合从形状上不可能，而界面正是靠它把「值守 · 没跑起来」与「发送失败」分开写。
   */
  proactive: boolean;
  /**
   * 提问轮的问题与选项（决策 265）：非提问行恒 `null`（加性字段，老客户端解析不受影响）。
   *
   * 只有 assistant 行可能携带——那一列（迁移 0026）的生产者只有工具执行点，
   * 用户行的 INSERT 不写它。
   */
  ask?: ForemanAsk | null;
  /**
   * 行的**在途状态**（迁移 0036，票 01）：`null` = 已收口的正常行（绝大多数），
   * `'in_flight'` = 正在跑的半截行，`'interrupted'` = 进程被杀留下的半截行（票 03）。
   *
   * **可选**：老后端不发这个字段（加性改动）——缺省当普通行渲染，正是它此前的行为。
   */
  status?: string | null;
  /**
   * 行内**位置序号**（迁移 0036，票 02）：拼接基准 `seq0` 的来源——直播里 `seq > seq0`
   * 的增量才接。只对在途行有意义，其余恒 0。
   */
  seq?: number;
  /**
   * **中断时刻**（迁移 0036，票 03）：`status === 'interrupted'` 的行记下什么时候断的。
   * 时间线把「已中断 + 这一刻」摆在名牌旁边——**可选**：老后端不发（加性改动）。
   */
  interrupted_at?: string | null;
}

/**
 * `GET /foreman/session`：某个班次按 id 升序的轮次 + **该班次**的合计
 * （决策 204⑤：按班次过滤之前，这个数其实是「自建库以来的累计值」）。
 *
 * `session` 为 `null` = 这台机器上一个班次都还没有（不是错误）：前端据此走
 * 「开一个班次」的空态。`foreman.wired` 为假时是未接线（503 之外的另一读法）。
 *
 * `proposals` 是**这个班次的全部**提议（含已执行 / 被拒绝 / 已过期）：时间线要的是
 * 「它当时提议过什么」，只有未决的会在 `GET /foreman/proposals` 里（那是待办，不是台账）。
 *
 * `turn_in_flight` 是**此刻**这一班有没有一轮在跑（决策 260）。它不是台账里的一行
 * （回话落库才算数），故刷新页面会把它重新问一遍——界面据它决定要不要接着把增量
 * 接进时间线（判据：接手锚点 `realtime/foreman.ts::maxLedgerId`、落地 `turnLanded`、
 * 接线在 `Talk.svelte` 的 `followingSince`）。
 *
 * `page_limit` 是**这一段自己的分页尺**（决策 354④，加性字段）：`messages` 至多这么多条。
 * 前端判「这一段读满了没有」（`stores/talk.svelte.ts` 的 `hasMoreEarlier`）只认它——
 * 此前那份判据靠一条注释与一个前端常量隔线对齐，改一边忘一边就是静默的分页错位。
 */
export interface ForemanSession {
  session: ForemanSessionMeta | null;
  messages: ForemanMessage[];
  proposals: ForemanProposal[];
  total_tokens: number;
  total_calls: number;
  turn_in_flight: boolean;
  page_limit: number;
  foreman: { agent_type: string; stage_key: string; wired: boolean };
}

/**
 * 一条**提议**（决策 188 / 207）：值班长想做一件会改动东西的事，落库等人按键。
 *
 * `summary` 是**后端按参数生成**的一句话（不是模型写的自由文本）——它是人按键之前读到的
 * 唯一一行字，故不能由模型自己措辞。`args` 原样下发（按键之前要看得出它到底要什么）。
 */
/** 修复提议的现场（决策 212① / 票 12）：闸门读数 + diff。 */
export interface RepairPayload {
  repair_id: string;
  worktree_path: string;
  branch: string;
  base_ref: string;
  base_commit: string;
  /** 闸门过了没有。**没过不会出 diff**（决策 210④）。 */
  gate_passed: boolean;
  gate: {
    kind: string;
    command: string;
    exit_code: number;
    duration_ms: number;
    output_path: string | null;
    output_preview: string;
  }[];
  commit: string | null;
  diff: string | null;
  diff_stat: string | null;
}

export interface ForemanProposal {
  id: string;
  session_id: string;
  /** 工具名（`write_file` / `run_command` / `task` …）。认不出的原样显示。 */
  tool: string;
  args: unknown;
  summary: string;
  status: string;
  /**
   * 载荷形态（票 12）：`api_call` = 一次工具调用；`repair` = **一次修复**。
   *
   * 后者执行的是「合入一个分支」，载荷里带 diff 与闸门读数——故它的渲染与工具调用不同
   * （人要看的是那份补丁，不是参数摘要）。
   */
  kind?: 'api_call' | 'repair';
  payload?: RepairPayload | null;
  /**
   * **来路**：它来自一轮被人按停的话吗（决策 294 / 票 09）。
   *
   * 人按停那一轮提的悬空提议**不作废**（显式修订 233③：作废只对「轮自己死了」）——
   * 于是这一条与正常来路的提议在卡片上唯一的差别就是这个读数：多写一行
   * 「那份结论没说完，按之前多看一眼」。地位同 `expires_at`：只影响怎么读，不影响能不能按。
   */
  stopped_round: boolean;
  created_at: string;
  /**
   * 有效期到点（决策 207：TTL 10 分钟）。**前端按它自己算过期**，不等后端标。
   *
   * 修复类例外（决策 212①）：它们的有效期是远期的「不按时间过期」——人有意留到第二天
   * 早上看，做成 10 分钟会让人早上看到一排灰按钮。
   */
  expires_at: string;
  resolved_at: string | null;
}

/** `POST /foreman/proposals/{id}/execute` 与 `…/reject` 的响应。 */
export interface ForemanProposalResult {
  proposal: ForemanProposal | null;
  message: ForemanMessage | null;
}

/** 会话命令台账的一行（`GET /foreman/commands`）。`task_id` 恒为 null——它挂会话。 */
export interface ForemanCommand {
  id: number;
  session_id: string | null;
  command: string;
  cwd: string;
  exit_code: number | null;
  started_at: string;
  finished_at: string | null;
}

/** `GET /foreman/sessions`：未归档的班次，按最近活动倒序。 */
export interface ForemanSessionList {
  sessions: ForemanSessionMeta[];
}

/**
 * `POST /foreman/messages`：`message` 是刚落库的回话行（LLM 失败时整个请求失败，但 user 行已落库）。
 *
 * `session` 是**这句话落进了哪个班次**：请求没带 `session_id` 时服务端会选或建一个，
 * 客户端据此更新自己的「当前班次」——否则第一次说话会落进一个它不知道的会话。
 */
export interface ForemanSendResult {
  message: ForemanMessage | null;
  reply: string;
  session: ForemanSessionMeta;
  total_tokens: number;
  total_calls: number;
}

/* ─────────────── 离线通知（crates/app/src/routes/notify.rs，决策 272 / 284）─────────────── */

/** 通知的某一级是谁定的（决策 272⑥ / 284② 的两级结构）：界面保存的单元，还是 `config.toml`。 */
export type NotifyOrigin = 'settings' | 'config';

/**
 * `GET /notify/settings` 的读数。
 *
 * 展示的是**生效的那一份**（单元 > 配置文件，整体覆盖）；`webhook_url` 与
 * `bluebubbles_password` 是秘密，只回常量掩码 `***`（决策 112 的范式）。
 *
 * 两级关系**两组各自成立**（决策 284②）：通道（`channel` / `origin`）与礼貌
 * （`cooldown_sec` / `quiet_hours` / `politeness_origin`）可以一个来自界面、
 * 一个来自配置文件。
 */
export interface NotifySettings {
  /** 一颗总开关（272⑧）：整条通道开/关，非每类一颗。 */
  enabled: boolean;
  /** 生效的通道声明；null = 还没有配置任何通道（268① 零配置零行为）。 */
  channel: 'generic' | 'feishu' | 'bluebubbles' | 'webpush' | null;
  /** 通道是谁定的。 */
  origin: NotifyOrigin;
  webhook_url: string;
  bluebubbles_url: string;
  bluebubbles_password: string;
  bluebubbles_recipient: string;
  /** 生效的节流窗口（秒，0 = 不节流；284② 起界面可改）。 */
  cooldown_sec: number;
  /** 生效的免打扰 `[开始, 结束)` 本地整点；起止相同 = 全天不静默。 */
  quiet_hours: [number, number];
  /** 礼貌两件是谁定的——通道的来源不代表礼貌的来源。 */
  politeness_origin: NotifyOrigin;
  /**
   * VAPID 公钥（pwa-webpush 02；base64url 无填充）——浏览器订阅时拿它当
   * `applicationServerKey`。**不是秘密**（本来就要交给浏览器），故原样回显。
   * 空串 = 还没生成过（在通道里保存一次「浏览器推送」就会生成）。
   */
  vapid_public_key: string;
  /** VAPID 私钥：**只给常量掩码** `***`（对齐 provider `api_key`，决策 112）。 */
  vapid_private_key: string;
  /** 当前两级解析不过时的原因（报错不静默，272⑧）；不在场 = 没有配置错误。 */
  config_error?: string;
}

/** 值守轮设置页的读数（决策 287 / 票 02）：开关 + provenance + 只读的节奏五个数。 */
export interface ForemanWatchSettings {
  /** 全局开关：关掉 = 值守轮不再自己醒（跑都不跑）；在飞的那一轮不受影响。 */
  enabled: boolean;
  /** 这一份是谁定的：`default` = 从没碰过设置（缺省开）；`settings` = 界面保存过。 */
  origin: 'default' | 'settings';
  /** `[pipeline] watch_*` 五个数，config.toml 那一级——**只读展示**，不开写口。 */
  config: {
    watch_event_window_minutes: number;
    watch_owner_stuck_minutes: number;
    watch_debounce_sec: number;
    watch_task_cooldown_minutes: number;
    watch_max_wakes_per_hour: number;
  };
}

/**
 * 一次可用性探测的读数（决策 297 / 票 03、05）。
 *
 * 三条判据全过才算 `available`：解析到绝对路径、`--version` 跑得起来、`hook claude`
 * 回得出一段可解析的改写。哪一条没过都写在 `reason` 里，界面原样摆出来——**不假装可用**。
 */
export interface RtkProbe {
  available: boolean;
  /** 解析到的绝对路径（判据①）。 */
  path: string | null;
  /** 这一份路径是谁定的：手填 / 服务进程的 PATH / 五个已知目录之一。 */
  source: 'manual' | 'path' | 'known-dir' | null;
  /** `rtk --version` 的第一行（判据②）。 */
  version: string | null;
  /** 不可用时的原因（三种失败各有各的说法）；可用时不在场。 */
  reason: string | null;
}

/**
 * 命令执行设置页的读数（决策 297 / 票 03、05）：开关 + provenance + **活体探测**。
 *
 * 探测**每次读都重做**（不缓存上次结果）：重读目标态才算数。
 */
export interface RtkSettings {
  /** 全局开关：关掉 = 命令按原样跑（逐字等于这个功能出现之前）。 */
  enabled: boolean;
  /** 这一份是谁定的：`default` = 从没碰过设置（缺省关）；`settings` = 界面保存过。 */
  origin: 'default' | 'settings';
  /** 手填的兜底路径（绝对路径）；`null` = 自动解析。 */
  path: string | null;
  probe: RtkProbe;
}

/** `PUT /notify/channel` 的载荷：通道单元**整体覆盖**（272⑥ 不允许混）。 */
export interface NotifyChannelPayload {
  channel: 'generic' | 'feishu' | 'bluebubbles' | 'webpush';
  webhook_url?: string;
  bluebubbles_url?: string;
  bluebubbles_password?: string;
  bluebubbles_recipient?: string;
}

/** `PUT /notify/politeness` 的载荷：礼貌单元**整体覆盖**（284②，与通道单元各自成立）。 */
export interface NotifyPolitenessPayload {
  /** 0–86400；0 = 不节流（免打扰照走）。 */
  cooldown_sec: number;
  /** 各 0–23；起止相同 = 全天不静默。 */
  quiet_hours: [number, number];
}

/** `POST /notify/test` 的结论（照 `POST /providers/test`，决策 160：成功失败都 200）。 */
export interface NotifyChannelTest {
  ok: boolean;
  message: string;
}

/**
 * `POST /notify/push/subscriptions` 的载荷（pwa-webpush 02）：**浏览器
 * `PushSubscription.toJSON()` 的形状**原样——前端不做转换，少一层就少一处漂移。
 */
export interface PushSubscriptionPayload {
  /** 推送服务给这台设备的地址（能力 URL）。 */
  endpoint: string;
  keys: {
    /** 浏览器公钥（P-256 未压缩点，base64url）。 */
    p256dh: string;
    /** 鉴权秘密（16 字节，base64url）。 */
    auth: string;
  };
}

/**
 * `GET /notify/push/subscriptions` 清单里的一行（pwa-webpush 02）。
 *
 * **只给摘要**：`endpoint_hint` 够认出「这是我那台 iPhone」，而完整 endpoint 是能力
 * URL（拿到它 + 密钥就能往那台设备推）——读接口是清单，不是能力包。
 */
export interface PushSubscriptionRow {
  id: number;
  /** endpoint 的摘要形（`…abc…xyz`）；不含主机名。 */
  endpoint_hint: string;
  /** 订阅那一刻的 UA（空串 = 浏览器没报）。 */
  user_agent: string;
  /** 订阅创建时刻（RFC3339）。 */
  created_at: string;
}

export interface PushSubscriptionList {
  subscriptions: PushSubscriptionRow[];
}
