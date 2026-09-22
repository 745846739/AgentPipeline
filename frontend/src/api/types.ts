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

export type Stage =
  | 'init'
  | 'architect-design'
  | 'develop-design'
  | 'test-design'
  | 'sync-check'
  | 'develop'
  | 'review'
  | 'test'
  | 'merge'
  | 'done';

export type Node = 'validate_input' | 'execute' | 'validate_output';

export type CursorStatus = 'active' | 'waiting_join' | 'pending' | 'archived';

export type PendingKind =
  | 'info_insufficient'
  | 'conflict_wait'
  | 'retry_exhausted'
  | 'user_decision'
  | 'merge_approval'
  | 'human_review'
  | 'dependency_failed'
  | 'context_overflow'
  | 'timeout';

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
  created_at: string;
}

export interface NodeCommand {
  id: number;
  task_id: string;
  run_id: number | null;
  stage: Stage;
  node: Node;
  source: CommandSource;
  command: string;
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
  /** 仅绑定回环时为 true——手机连不上，分享页需给出开启指引。 */
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

/** 该轮工具痕迹的一项（`ForemanTrace`）。`args_summary` 是参数摘要，不是原文。 */
export interface ForemanTrace {
  tool: string;
  args_summary: string;
  ok: boolean;
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
 * 归档 = 置 `archived_at`：从列表里收起来，**不物理删除**（消息也照旧吃保留期）。
 */
export interface ForemanSessionMeta {
  id: string;
  title: string;
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
 */
export interface ForemanSession {
  session: ForemanSessionMeta | null;
  messages: ForemanMessage[];
  proposals: ForemanProposal[];
  total_tokens: number;
  total_calls: number;
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
