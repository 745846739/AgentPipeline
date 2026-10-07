import { CLIENT_HEADER, PAIRING_HEADER, PAIRING_QUERY, apiUrl, getPairingToken } from './config';
import type {
  AnalyzeResponse,
  CompactionSettings,
  MarketRepoConfig,
  MarketSkillList,
  CreateTaskPayload,
  FlowResponse,
  ForemanSendResult,
  ForemanSession,
  ForemanSessionList,
  ForemanSessionMeta,
  ForemanProposalResult,
  ForemanToolLabelList,
  ForemanAttention,
  ForemanCommand,
  GlobalMetrics,
  NodeCommand,
  NodeConversation,
  NotifyChannelPayload,
  NotifyChannelTest,
  NotifyPolitenessPayload,
  PushSubscriptionList,
  PushSubscriptionPayload,
  ForemanWatchSettings,
  OffloadSettings,
  RtkSettings,
  NotifySettings,
  Project,
  ProjectAnalysis,
  ProjectCreatePayload,
  ProjectPatchPayload,
  Provider,
  ProviderCreatePayload,
  ProviderPatchPayload,
  ProviderTestPayload,
  ConnectionTestResult,
  ResumePayload,
  ServerInfo,
  RecommendedStage,
  OneClickInstallResult,
  SkillPreview,
  SkillSummary,
  StageConfig,
  StageConfigPutPayload,
  Task,
  TaskDetail,
  TaskListItem,
  TaskMetrics,
  TaskStatus,
  ConversationSummary,
  Stage,
  Node as PipelineNode,
} from './types';

export class ApiError extends Error {
  readonly status: number;
  /**
   * 后端错误体里的机器可读分类（`{ error, detail, kind }` 的第三项，票 02 的八类市场失败）。
   *
   * **界面按它分支**，不按状态码、更不按 `message` 里的字样——`repo_not_found` 与
   * `commit_not_found` 都是 404，只有 `kind` 分得开，而两者要用户做的事完全不同
   * （改仓名 vs 换 commit）。其余端点不带这个字段，故是 `undefined`。
   */
  readonly kind: string | undefined;
  constructor(status: number, message: string, kind?: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.kind = kind;
  }
}

/**
 * 「本地等不到回包」这一类失败的 `kind`（票 06，命名姿态照配对的 `pairing_required`）。
 *
 * **后端不参与**：超时是前端本地 `AbortSignal.timeout` 的产物、没有 HTTP 应答体，
 * 故这枚 kind 由 {@link mapRequestError} 在构造点自己带上——消费者
 * （`realtime/foreman.ts::isRequestTimeout`）据此分支，不摸报文字样。
 */
export const KIND_REQUEST_TIMEOUT = 'request_timeout';

/**
 * 请求的默认超时（票 12 / R2-14）。**全站唯一出处**，各页不再各写一个。
 *
 * 为什么必须有：`fetch` 在「TCP 连上但不回包」时**不会自己失败**——此前全仓没有一处
 * 传 `signal`、也没有 `AbortSignal.timeout`，于是技能安装的 `installing` 永不复位、
 * 市场列表卡在「正在读 X…」、指标页的按钮一直禁着，唯一的出路是刷新整页。
 *
 * 取 30s：比一切正常端点的实测慢得多（本地 axum 是毫秒级），又比「用户以为死了」短。
 * 已知慢的那条链（项目分析）自带 60s 的**轮询**上限（`lib/analysis.ts` 的
 * `ANALYSIS_POLL_TIMEOUT_MS`），单次请求仍走这个兜底；值班长那一轮要等模型把话说完，
 * 显式放宽到 3 分钟——**慢是那几个调用的属性，不该把全站的兜底一起拉长**。
 */
export const REQUEST_TIMEOUT_MS = 30_000;

interface RequestOptions {
  method?: string;
  body?: unknown;
  /** 期望纯文本（命令输出 / 产出文件）。 */
  text?: boolean;
  signal?: AbortSignal;
  /** 本次调用的超时（毫秒）；缺省 `REQUEST_TIMEOUT_MS`。 */
  timeoutMs?: number;
}

/**
 * `fetch` 的异常 → 界面口径的错误（票 12 / R2-14）。
 *
 * **导出是为了可测**：超时与「网络本身不通」要说得出来区别——前者是「等太久了，再试一次
 * 可能就好」，后者是「连不上，先看服务在不在」。押成同一句话，用户就失去下一步。
 *
 * `TimeoutError` 是 `AbortSignal.timeout` 的产物；`AbortError` 只有在**没有调用方 signal**
 * 时才归到超时（有调用方 signal 的那种 AbortError 是调用方主动取消，不是超时）。
 */
export function mapRequestError(err: unknown, timeoutMs: number, hasCallerSignal: boolean): ApiError {
  const e = err as Error;
  if (e?.name === 'TimeoutError' || (e?.name === 'AbortError' && !hasCallerSignal)) {
    // kind 在构造点带上（票 06）：报文是给用户看的，界面分支只认这枚字段
    return new ApiError(0, `请求超时（${Math.round(timeoutMs / 1000)} 秒没有回应）。`, KIND_REQUEST_TIMEOUT);
  }
  return new ApiError(0, `网络请求失败：${e?.message ?? String(err)}`);
}

async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const method = opts.method ?? 'GET';
  const headers: Record<string, string> = { Accept: 'application/json, text/plain, */*' };
  // 决策 153③：所有写请求恒携带 X-AgentPipeline（128 旁路）。
  if (method !== 'GET' && method !== 'HEAD') {
    headers[CLIENT_HEADER] = '1';
  }
  // 决策 182㉙（票 07）：配对令牌压在**所有**方法上，不只写请求——非回环形态下
  // 对讲台的读接口（`/foreman/session`、`/foreman/stream`）同样凭它通行，
  // 而「只写请求带头」会让手机上看得到看板、对讲台却一律 403，那是更难排查的形态。
  // 未配对时（回环形态的常态）这个头根本不带，本地使用零摩擦。
  const token = getPairingToken();
  if (token) headers[PAIRING_HEADER] = token;
  if (opts.body !== undefined) {
    headers['Content-Type'] = 'application/json';
  }

  // 超时与调用方自己的 signal 合成一个。老内核没有 `AbortSignal.any` 时只能二选一，
  // 这里**保调用方给的取消**（那是显式意图：调用方说撤就撤），超时那一条不生效；
  // 今天全仓没有一处传 `signal`，两条路都走不到这一格。宁可少一层兜底，也不悄悄
  // 吞掉调用方的取消。
  const timeoutMs = opts.timeoutMs ?? REQUEST_TIMEOUT_MS;
  let signal = opts.signal;
  try {
    const timeout = AbortSignal.timeout(timeoutMs);
    signal =
      opts.signal && typeof AbortSignal.any === 'function'
        ? AbortSignal.any([opts.signal, timeout])
        : (opts.signal ?? timeout);
  } catch {
    // 没有 `AbortSignal.timeout`（旧内核）：保持调用方给的那个，至少不更坏
  }

  let res: Response;
  try {
    res = await fetch(apiUrl(path), {
      method,
      headers,
      body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
      signal,
    });
  } catch (err) {
    throw mapRequestError(err, timeoutMs, opts.signal !== undefined);
  }

  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    let kind: string | undefined;
    try {
      const data = (await res.json()) as { error?: string; kind?: string };
      if (data?.error) message = data.error;
      // 分类只在这里读一次：市场八类失败要在界面上给出八种不同的动作提示，
      // 在每处调用点各解一遍错误体是「两处口径漂移」的标准起因。
      if (typeof data?.kind === 'string' && data.kind !== '') kind = data.kind;
    } catch {
      // 非 JSON 错误体，保留状态文本
    }
    throw new ApiError(res.status, message, kind);
  }

  if (opts.text) return (await res.text()) as unknown as T;
  if (res.status === 204) return undefined as unknown as T;
  return (await res.json()) as T;
}

/* ─────────────────────────────── 项目 ─────────────────────────────── */

export async function listProjects(): Promise<Project[]> {
  const data = await request<{ projects: Project[] }>('/projects');
  return data.projects;
}

/* ─────────────────────────────── 任务 ─────────────────────────────── */

export interface TaskListParams {
  project_id?: string | null;
  status?: TaskStatus | null;
  include_archived?: boolean;
}

export async function listTasks(params: TaskListParams = {}): Promise<TaskListItem[]> {
  const q = new URLSearchParams();
  if (params.project_id) q.set('project_id', params.project_id);
  if (params.status) q.set('status', params.status);
  q.set('include_archived', params.include_archived ? 'true' : 'false');
  const data = await request<{ tasks: TaskListItem[] }>(`/tasks?${q.toString()}`);
  return data.tasks;
}

export function getTask(id: string, signal?: AbortSignal): Promise<TaskDetail> {
  return request<TaskDetail>(`/tasks/${encodeURIComponent(id)}`, { signal });
}

export function createTask(payload: CreateTaskPayload): Promise<{ task: Task }> {
  return request<{ task: Task }>('/tasks', { method: 'POST', body: payload });
}

export function getFlow(id: string, signal?: AbortSignal): Promise<FlowResponse> {
  return request<FlowResponse>(`/tasks/${encodeURIComponent(id)}/flow`, { signal });
}

export interface ConversationListParams {
  /** 取回被重试归档的旧 attempt（§12.2）；缺省只看未归档。 */
  includeArchived?: boolean;
  /**
   * **批量取正文**（决策 361，票 03）：命中时服务端一次返回该任务全部轮的完整会话。
   *
   * 返回的元素形状随之变成 `NodeConversation`（与 `GET /conversations/{run_id}`
   * 的单条读法同形）——它带 `messages_json`，但没有 `status`（台账那一列只贴给摘要态）。
   * 调用方要状态时读摘要那一份。
   */
  includeMessages?: boolean;
  /**
   * **限定轮**（现场页签增量拉取）：只取这些 run_id 的会话，缺省 = 全部轮。
   * 配合批量正文态用——缓存里已有的轮不重发，载荷从「全任务 MB 级」降到缺失的增量。
   */
  runIds?: number[];
}

/** 摘要态（缺省，`includeMessages` 未开）的返回类型。 */
export function getConversations(
  id: string,
  params?: ConversationListParams & { includeMessages?: false },
  signal?: AbortSignal,
): Promise<ConversationSummary[]>;
/** 批量正文态：一次拿回（或按 `runIds` 限定）任务会话的完整正文。 */
export function getConversations(
  id: string,
  params: ConversationListParams & { includeMessages: true },
  signal?: AbortSignal,
): Promise<NodeConversation[]>;
export function getConversations(
  id: string,
  params: ConversationListParams = {},
  signal?: AbortSignal,
): Promise<ConversationSummary[] | NodeConversation[]> {
  const q = new URLSearchParams();
  if (params.includeArchived) q.set('include_archived', 'true');
  if (params.includeMessages) q.set('include_messages', 'true');
  if (params.runIds?.length) q.set('run_ids', params.runIds.join(','));
  const suffix = q.toString() ? `?${q.toString()}` : '';
  return request<{ conversations: ConversationSummary[] | NodeConversation[] }>(
    `/tasks/${encodeURIComponent(id)}/conversations${suffix}`,
    { signal },
  ).then((d) => d.conversations);
}

export function getConversation(
  id: string,
  runId: number,
  signal?: AbortSignal,
): Promise<NodeConversation> {
  return request<{ conversation: NodeConversation }>(
    `/tasks/${encodeURIComponent(id)}/conversations/${runId}`,
    { signal },
  ).then((d) => d.conversation);
}

export interface CommandListParams {
  stage?: Stage;
  node?: PipelineNode;
}

export function getCommands(
  id: string,
  params: CommandListParams = {},
  signal?: AbortSignal,
): Promise<NodeCommand[]> {
  const q = new URLSearchParams();
  if (params.stage) q.set('stage', params.stage);
  if (params.node) q.set('node', params.node);
  const suffix = q.toString() ? `?${q.toString()}` : '';
  return request<{ commands: NodeCommand[] }>(
    `/tasks/${encodeURIComponent(id)}/commands${suffix}`,
    { signal },
  ).then((d) => d.commands);
}

export function getCommandOutput(id: string, cmdId: number, signal?: AbortSignal): Promise<string> {
  return request<string>(`/tasks/${encodeURIComponent(id)}/commands/${cmdId}/output`, {
    text: true,
    signal,
  });
}

/**
 * 读取任务产出文件。403 → 路径越界（api 层 forbidden），404 → 不存在；
 * 调用方据此做降级提示（ticket 21）。
 */
export function getTaskFile(id: string, relPath: string, signal?: AbortSignal): Promise<string> {
  const clean = relPath.replace(/^\/+/, '');
  return request<string>(
    `/tasks/${encodeURIComponent(id)}/files/${clean.split('/').map(encodeURIComponent).join('/')}`,
    { text: true, signal },
  );
}

/* ─────────────────────────────── resume / 旁路动作 ─────────────────────────────── */

export interface ResumeResponse {
  ok: boolean;
  action: string;
  cursor_id: string;
  spawned: boolean;
}

/** resume 类动作（决策 91：cursor_id 必带于多游标场景）。 */
export function resumeTask(id: string, payload: ResumePayload): Promise<ResumeResponse> {
  return request<ResumeResponse>(`/tasks/${encodeURIComponent(id)}/resume`, {
    method: 'POST',
    body: payload,
  });
}

export function cancelTask(id: string): Promise<unknown> {
  return request(`/tasks/${encodeURIComponent(id)}/cancel`, { method: 'POST' });
}

/**
 * 打开 / 关掉**这一个任务**的托管（决策 210①，票 08 的端点 / 票 14 的界面）。
 *
 * 关掉走的是同一个端点 + `enabled: false`（后端语义是**清空那一列**，不是写一个 false）。
 * 终态任务（400）与值班长未接线（503）都会被后端拒掉，故界面先不摆那颗钮
 * （见 `lib/stewardship.ts::stewardshipFace`）。
 */
export function setStewardship(id: string, enabled: boolean): Promise<{ ok: boolean; task: Task }> {
  return request<{ ok: boolean; task: Task }>(`/tasks/${encodeURIComponent(id)}/stewardship`, {
    method: 'POST',
    body: { enabled },
  });
}

export function retryTask(id: string): Promise<unknown> {
  return request(`/tasks/${encodeURIComponent(id)}/retry`, { method: 'POST' });
}

/**
 * 把**在跑**的任务按住（决策 276）：中止在飞的那一轮、位置保留。
 *
 * `message` 是后端写好的那一句（含「有没有通知到在跑的执行体」这件事实）——界面直接显示，
 * 不自己拼一句「应该成功了」的话。续跑 / 重跑本阶段那两颗钮不在这里：它们由暂停之后
 * 后端下发的 `allowed_actions` 给（`continue` / 带落点的 `goto`），走 `resume` 那条路。
 */
export function pauseTask(id: string): Promise<{ ok: boolean; message: string }> {
  return request(`/tasks/${encodeURIComponent(id)}/pause`, { method: 'POST' });
}

/** 从**本阶段入口**重跑一遍（决策 276）：那一轮不算。整条任务回 init 重跑是 `retryTask`。 */
export function rerunTask(id: string): Promise<{ ok: boolean; message: string }> {
  return request(`/tasks/${encodeURIComponent(id)}/rerun`, { method: 'POST' });
}

export function archiveTask(id: string): Promise<unknown> {
  return request(`/tasks/${encodeURIComponent(id)}/archive`, { method: 'POST' });
}

export interface SplitTaskSpec {
  title: string;
  description?: string;
  depends_on?: string[];
}

export function splitTask(id: string, tasks: SplitTaskSpec[]): Promise<{ created: string[] }> {
  return request(`/tasks/${encodeURIComponent(id)}/split`, {
    method: 'POST',
    body: { tasks },
  });
}

export function modelOverrideTask(id: string, providerId: string): Promise<unknown> {
  return request(`/tasks/${encodeURIComponent(id)}/model-override`, {
    method: 'POST',
    body: { provider_id: providerId },
  });
}

/** 人工评审（review_mode = human，决策 2 / 124）。 */
export function reviewTask(id: string, approved: boolean, comments?: string): Promise<unknown> {
  return request(`/tasks/${encodeURIComponent(id)}/review`, {
    method: 'POST',
    body: { approved, comments: comments ?? null },
  });
}

/** merge 审批（决策 23 / 119）：approve = 合入，return = 返回修改。**无"拒绝"**。
 *  `push`（决策 393）：approve 时合入后是否推远端（缺省 false；无 remote 服务端自动跳过）。 */
export function mergeDecision(
  id: string,
  decision: 'approve' | 'return',
  push = false,
): Promise<unknown> {
  return request(`/tasks/${encodeURIComponent(id)}/merge/decision`, {
    method: 'POST',
    body: { decision, push },
  });
}

/* ─────────────────────────────── 其它（票 22 消费）─────────────────────────────── */

/* providers（决策 111 / 112）：读接口 api_key 恒为 `***`。 */

export function listProviders(): Promise<Provider[]> {
  return request<{ providers: Provider[] }>('/providers').then((d) => d.providers);
}

export function createProvider(payload: ProviderCreatePayload): Promise<Provider> {
  return request<{ provider: Provider }>('/providers', { method: 'POST', body: payload }).then(
    (d) => d.provider,
  );
}

/** patch 中**不要**带 `***` 掩码（调用方用 buildProviderPatch 保证）。 */
export function updateProvider(id: string, patch: ProviderPatchPayload): Promise<Provider> {
  return request<{ provider: Provider }>(`/providers/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: patch,
  }).then((d) => d.provider);
}

export function deleteProvider(id: string): Promise<void> {
  return request<void>(`/providers/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

/** 连通性探针（决策 160）：探测成功/失败都返回 200，结论在结果体里。 */
export function testProvider(payload: ProviderTestPayload): Promise<ConnectionTestResult> {
  return request<{ test: ConnectionTestResult }>('/providers/test', {
    method: 'POST',
    body: payload,
  }).then((d) => d.test);
}

/* projects（决策 24 / 29 / 61 / 78 / 101 / 130）。 */

export function createProject(payload: ProjectCreatePayload): Promise<Project> {
  return request<{ project: Project }>('/projects', { method: 'POST', body: payload }).then(
    (d) => d.project,
  );
}

export function updateProject(id: string, patch: ProjectPatchPayload): Promise<Project> {
  return request<{ project: Project }>(`/projects/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: patch,
  }).then((d) => d.project);
}

/** 有活跃任务时后端返回 409，message 即拒绝原因（决策 101）。 */
export function deleteProject(id: string): Promise<void> {
  return request<void>(`/projects/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

/** `POST /projects/analyze` → 202 `{ analysis_id }`，随后轮询 analysis。 */
export function startProjectAnalysis(projectId: string): Promise<AnalyzeResponse> {
  return request<AnalyzeResponse>('/projects/analyze', {
    method: 'POST',
    body: { project_id: projectId },
  });
}

/** `GET /projects/{id}/analysis`：项目尚无分析记录时后端返回 404。 */
export function getProjectAnalysis(projectId: string): Promise<ProjectAnalysis> {
  return request<ProjectAnalysis>(`/projects/${encodeURIComponent(projectId)}/analysis`);
}

/* metrics（决策 130）。 */

export function getGlobalMetrics(): Promise<GlobalMetrics> {
  return request<GlobalMetrics>('/metrics');
}

export function getTaskMetrics(id: string): Promise<TaskMetrics> {
  return request<TaskMetrics>(`/tasks/${encodeURIComponent(id)}/metrics`);
}

/* stage_configs（票 22）：GET 列表 / PUT 整条替换 / DELETE 撤销覆盖。 */

export function listStageConfigs(): Promise<StageConfig[]> {
  return request<{ stage_configs: StageConfig[] }>('/stage-configs').then((d) => d.stage_configs);
}

/** PUT 是**整条替换**：缺省字段清空为默认；4xx 的 `{ error }` 由 request 抛出 ApiError。 */
export function putStageConfig(
  stage: string,
  payload: StageConfigPutPayload,
): Promise<StageConfig> {
  return request<{ stage_config: StageConfig }>(
    `/stage-configs/${encodeURIComponent(stage)}`,
    { method: 'PUT', body: payload },
  ).then((d) => d.stage_config);
}

export function deleteStageConfig(stage: string): Promise<void> {
  return request<void>(`/stage-configs/${encodeURIComponent(stage)}`, { method: 'DELETE' });
}

/* 技能（决策 172④⑤，票 09 / 11 / 16）：清单 / 预览 / 信任转换 / 推荐与一键安装。 */

export function listSkills(): Promise<SkillSummary[]> {
  return request<{ skills: SkillSummary[] }>('/skills').then((d) => d.skills);
}

/** 已安装技能的三项预览（推荐去向 / 注入模式与信任态 / 正文特征命中）。 */
export function previewSkill(name: string): Promise<SkillPreview> {
  return request<SkillPreview>(`/skills/${encodeURIComponent(name)}/preview`);
}

/**
 * 显式信任 / 撤销信任：**按技能名**改写引用它的每一条声明（信任态不另存一份账）。
 *
 * 撤销信任撞上全文模式时后端会 400 并给出可操作提示（不静默降级），原样回显即可。
 *
 * 与技能控件里的「信任此技能」不是重复：控件一次改**一行**声明（那个阶段 / 节点的），
 * 本端点一次改**全部**引用（阶段级 + 各节点级），是「整个技能的口径」这个动作。
 */
export function setSkillTrust(
  name: string,
  trusted: boolean,
): Promise<{ name: string; trusted: boolean; changed: number; updated_stages: string[]; note: string | null }> {
  return request(`/skills/${encodeURIComponent(name)}/trust`, {
    method: 'PUT',
    body: { trusted },
  });
}

/** 各阶段的推荐技能清单（含「装没装」），未安装的项由界面显示「未安装」。 */
export function listRecommendedSkills(): Promise<RecommendedStage[]> {
  return request<{ stages: RecommendedStage[] }>('/skills/recommendations').then((d) => d.stages);
}

/**
 * 一键安装：技能落到技能根 **且** 写进该阶段配置（一步完成）。
 *
 * 失败可归因：技能不存在 → 404、来源未放行 → 400、摘要不符 → 400（带 `detail`）、
 * 网络失败 → 502。响应里的 `preview` 是票 11 的三项预览，界面据此把特征命中摆给用户看。
 */
export function installSkillForStage(
  stage: string,
  name: string,
  overwrite = false,
): Promise<OneClickInstallResult> {
  return request<OneClickInstallResult>('/skills/install', {
    method: 'POST',
    body: { stage, name, overwrite },
  });
}

/* 技能市场（决策 194）：仓名单的读写 + 列表 + 从一个钉住的 commit 安装。 */

/** 当前生效的仓名单、它的来源级别（界面 / 配置文件）与冷启动推荐名单。 */
export function getMarketRepos(): Promise<MarketRepoConfig> {
  return request<MarketRepoConfig>('/market/repos');
}

/**
 * 保存界面上的仓名单（决策 194，继承 187 的两级结构）：**保存完当场生效**，不必重启。
 *
 * 空数组是合法且显式的输入（= 一个仓都不放行，看不到也装不上），与「没保存过」不是一回事
 * ——后者读 `config.toml` 的 `[market] github_repos`。非法项由后端 400 并说明是哪一项、
 * 为什么（校验与配置解析共用同一个函数）。
 */
export function saveMarketRepos(repos: string[]): Promise<MarketRepoConfig> {
  return request<MarketRepoConfig>('/market/repos', { method: 'PUT', body: { repos } });
}

/** 清掉界面那份仓名单，回到 `config.toml` 的 `[market] github_repos`（决策 194 / 187）。 */
export function clearMarketRepos(): Promise<MarketRepoConfig> {
  return request<MarketRepoConfig>('/market/repos', { method: 'DELETE' });
}

/* 离线通知（决策 272）：读数 / 总开关 / 通道单元 / 探针。 */

/** 值守轮的读数：开关 + provenance + 只读的节奏五个数。 */
export function getForemanWatch(): Promise<ForemanWatchSettings> {
  return request<ForemanWatchSettings>('/foreman-watch');
}

/**
 * 拨**值守开关**（决策 287）：保存即活——值守循环每 10s 读一次库里的这一行，
 * 下一趟按新值走，不必重启；在飞的那一轮不受影响。
 */
export function setForemanWatch(enabled: boolean): Promise<{ enabled: boolean; origin: string }> {
  return request<{ enabled: boolean; origin: string }>('/foreman-watch', {
    method: 'PUT',
    body: { enabled },
  });
}


/* 重活外发（票 runner-offload/05）：开关 + 活体探测；保存时探测失败也照存。 */

/** 读**重活外发**设置：存的状态 + 每次现做一次的活体探测（gh 登录态/工作流在场）。 */
export function getOffload(): Promise<OffloadSettings> {
  return request<OffloadSettings>('/offload');
}

/** 拨**重活外发开关**：保存即活——外发工具每条命令现读这一行，下一条就按新值走。 */
export function setOffload(payload: { enabled: boolean }): Promise<OffloadSettings> {
  return request<OffloadSettings>('/offload', { method: 'PUT', body: payload });
}

/* 命令执行（决策 297）：开关 + 活体探测；保存时探测失败也照存。 */

/**
 * 读**命令执行**设置（票 05）：存的状态 + **每次现做一次**的活体探测。
 *
 * 「现做」不是实现细节而是契约的一部分：这台机器上 rtk 装没装、还灵不灵，只有在读的
 * 那一刻问一遍才算数（决策 257 的「重读目标态」）。
 */
export function getRtk(): Promise<RtkSettings> {
  return request<RtkSettings>('/rtk');
}

/**
 * 拨**命令执行开关**（票 05）：保存即活——每条命令现读库里的这一行，下一条就按新值走。
 *
 * 启用时后端先探测，但**探测失败不拦**（决策 297）：照样存下来，把 `probe` 摆出来给
 * 用户看。`path` 传空串或省略 = 回到自动解析。
 */
export function setRtk(payload: {
  enabled: boolean;
  path?: string | null;
}): Promise<RtkSettings> {
  return request<RtkSettings>('/rtk', { method: 'PUT', body: payload });
}


/** 生效的通道读数（单元 > `config.toml`；秘密只回掩码）。 */
export function getNotifySettings(): Promise<NotifySettings> {
  return request<NotifySettings>('/notify/settings');
}

/**
 * 拨**总开关**（决策 272⑧）：开启时后端先解析生效通道、BlueBubbles 先 ping——
 * **够不着不当成功**（400），一个字节都不落库；关闭直接摘出口（`clear_notifier`）。
 */
export function setNotifyEnabled(enabled: boolean): Promise<{ ok: boolean; enabled: boolean }> {
  return request<{ ok: boolean; enabled: boolean }>('/notify/settings', {
    method: 'PUT',
    body: { enabled },
  });
}

/** 保存通道单元（**整体覆盖** `config.toml`，272⑥；掩码或留空 = 沿用已存值）。 */
export function saveNotifyChannel(unit: NotifyChannelPayload): Promise<{ ok: boolean }> {
  return request<{ ok: boolean }>('/notify/channel', { method: 'PUT', body: unit });
}

/** 交还 `config.toml` 那一级（照 `DELETE /market/repos`）；开关不动。 */
export function clearNotifyChannel(): Promise<{ ok: boolean }> {
  return request<{ ok: boolean }>('/notify/channel', { method: 'DELETE' });
}

/**
 * 保存**礼貌单元**（决策 284②：节流 + 免打扰整体覆盖 `config.toml`；越界由后端 400
 * 点名，报错不静默）。开关开着时保存即生效——后端按新值重建出口。
 */
export function saveNotifyPoliteness(
  unit: NotifyPolitenessPayload,
): Promise<{ ok: boolean }> {
  return request<{ ok: boolean }>('/notify/politeness', { method: 'PUT', body: unit });
}

/** 交还 `config.toml` 的 `[notify]` 那一份；开关与通道单元都不动（284⑦）。 */
export function clearNotifyPoliteness(): Promise<{ ok: boolean }> {
  return request<{ ok: boolean }>('/notify/politeness', { method: 'DELETE' });
}

/**
 * 订阅此设备（`POST /notify/push/subscriptions`，pwa-webpush 02）：按 `endpoint`
 * upsert（同设备两次订阅落一行），返回那一行的 id（本机退订时按它删）。
 *
 * **过配对令牌守卫**：局域网来源要带令牌（`request` 自动带上），回环豁免。
 */
export function subscribePushDevice(
  payload: PushSubscriptionPayload,
): Promise<{ ok: boolean; id: number }> {
  return request<{ ok: boolean; id: number }>('/notify/push/subscriptions', {
    method: 'POST',
    body: payload,
  });
}

/** 已订阅设备清单（`GET`，**读也过配对守卫**——它是外泄管道，不是公开读数）。 */
export function listPushSubscriptions(): Promise<PushSubscriptionList> {
  return request<PushSubscriptionList>('/notify/push/subscriptions');
}

/** 撤销一台设备（单个撤销）。 */
export function deletePushSubscription(id: number): Promise<{ ok: boolean; removed: boolean }> {
  return request<{ ok: boolean; removed: boolean }>(`/notify/push/subscriptions/${id}`, {
    method: 'DELETE',
  });
}

/** 一键清空订阅（换手机 / 怀疑被订阅过时的收回动作）。 */
export function clearPushSubscriptions(): Promise<{ ok: boolean; removed: number }> {
  return request<{ ok: boolean; removed: number }>('/notify/push/subscriptions', {
    method: 'DELETE',
  });
}

/** BlueBubbles 连通性探针（照 `POST /providers/test`，决策 160：成功失败都 200）。 */
export function testNotifyChannel(body: {
  bluebubbles_url: string;
  bluebubbles_password?: string;
}): Promise<{ test: NotifyChannelTest }> {
  return request<{ test: NotifyChannelTest }>('/notify/test', { method: 'POST', body });
}

/**
 * 列出某个仓里的技能（按父路径分组）。
 *
 * `refresh = true` → 重新 `head()` 取 tip；否则用缓存里那个 commit（列表因此**钉住**了
 * 浏览时那一份，直到用户显式刷新）。`q` 只过滤已 fetch 的那一份，不会去打 GitHub 的搜索
 * API（票 03 的取舍：跨仓搜索只覆盖已拉下来的仓）——界面因此可以纯本地过滤，见页面组件。
 */
export function listMarketSkills(
  repo: string,
  q = '',
  refresh = false,
): Promise<MarketSkillList> {
  const params = new URLSearchParams({ repo });
  if (q !== '') params.set('q', q);
  if (refresh) params.set('refresh', '1');
  return request<MarketSkillList>(`/market/skills?${params.toString()}`);
}

/**
 * 从一个**钉住的 commit** 装一个技能目录到技能根（**不**写任何阶段配置——启用是另一件事）。
 *
 * `commit` 必须由调用方从列表上原样透传，**不得在这里或后端中途「取最新」**：用户看到的是
 * 某一份，装到的就必须是那一份（决策 194 裁决 ⑤）。
 *
 * 八类失败分得开（`kind` 字段，见 {@link ApiError}）：`market_network` / `repo_not_found` /
 * `commit_not_found` / `skill_not_found` / `repo_unreadable` / `digest_mismatch` /
 * `repo_not_allowed` / `download_too_large`；同名已存在是 409（`overwrite` 为显式确认）。
 * 落盘之后用 {@link previewSkill} 取三项预览给用户看（正文特征命中要摆在眼前）。
 */
export function installFromRepo(payload: {
  owner: string;
  repo: string;
  commit: string;
  subpath: string;
  overwrite?: boolean;
}): Promise<{ skill: { name: string; description: string | null; sibling_count: number } }> {
  return request('/market/install', { method: 'POST', body: payload });
}

/* server-info（决策 167）：局域网分享地址枚举与二维码。 */

/** 服务自述：候选局域网地址 + 当前是否仅回环绑定。 */
export function getServerInfo(): Promise<ServerInfo> {
  return request<ServerInfo>('/server-info');
}

/**
 * 打开 / 关闭局域网访问（决策 186）：把绑定切成全网卡或只回环，**当场改绑**并记住选择。
 *
 * **应答可能读不到**：改绑会切断当前所有连接，包括发出这次请求的那条。调用方必须把
 * 「传输失败」与「真的失败」分开——正确读法是重读 [`getServerInfo`]，读到目标状态
 * 就算成功（`changeLanMode` 就是这么做的）。
 *
 * 只有本机可以调（局域网来源 403）：这是全站唯一能把服务暴露到局域网的入口。
 */
export function setServerLan(enabled: boolean): Promise<ServerInfo> {
  // `body` 传**对象**：`request()` 负责 JSON.stringify 与 Content-Type（传字符串会被
  // 再序列化一次，服务端解不出来——422）
  return request<ServerInfo>('/server/lan', { method: 'POST', body: { enabled } });
}

/** 清掉界面上的绑定选择，回到启动参数 / 配置文件那一级（决策 186）。 */
export function clearServerLan(): Promise<ServerInfo> {
  return request<ServerInfo>('/server/lan', { method: 'DELETE' });
}

/**
 * 二维码 SVG 的 `<img src>` 地址。
 *
 * 用 `apiUrl` 而非裸相对路径：桌面壳 / 跨源注入形态下 base 非空，裸路径会指错。
 * 服务端只接受本服务自己的地址（决策 167），故 `url` 必须来自 `getServerInfo`。
 */
export function qrSvgUrl(url: string): string {
  return apiUrl(`/server-info/qr.svg?url=${encodeURIComponent(url)}`);
}

/* 配对令牌（决策 182㉙，票 07）。 */

/**
 * 读取本服务的配对令牌。
 *
 * **只在回环可读**（服务端强制）：这一步是「在跑服务的这台电脑上生成给手机的链接」，
 * 手机自己打开分享页、或在电脑上用局域网地址打开分享页，这里都会 403——那是设计如此，
 * 不是故障。**调用方不据此降级为裸地址**（决策 189）：没有令牌就不画二维码，改给
 * 「去那台电脑本机打开本页」的指引——那张裸地址的码扫了配不上，却与正常的那张一样。
 */
export function fetchPairingToken(): Promise<{ token: string }> {
  return request<{ token: string }>('/pairing/token');
}

/** 一键重置配对：服务端换新令牌，旧令牌立即失效（调用方须同步清掉本地那份）。 */
export function resetPairing(): Promise<{ token: string }> {
  return request<{ token: string }>('/pairing/reset', { method: 'POST' });
}

/**
 * 把令牌拼进地址（与后端 `server_info.rs::pairing_url` 同一约定）。
 *
 * 参数名 `pair` 是前后端**唯一**的约定，改一处就得改另一处；后端的单测钉着它。
 */
export function pairedUrl(base: string, token: string): string {
  return `${base}/?${PAIRING_QUERY}=${encodeURIComponent(token)}`;
}

/* 值班长 / 对讲台（决策 182，票 01 / 03 / 04）：任务无关的两个读写口。 */

/**
 * 本会话台账（按 id 升序）。**这是对讲台唯一的权威状态入口**：界面重取它来对齐
 * 「值班经理说了什么 / 值班长回了什么 / 合计烧了多少 token」，不自己攒一份账。
 * 工头未接线时后端回 503，由 `request` 抛出 `ApiError`。
 *
 * `beforeId`（票 05，向上游标）：只取**更早的一段**（`id < beforeId`，段内升序，
 * 到头回空）。不给 = 缺省最近一页（多少条由应答里的 `page_limit` 回显，决策 354④），
 * 与从前逐字一致。
 */
export function getForemanSession(
  sessionId?: string | null,
  signal?: AbortSignal,
  kind?: string,
  beforeId?: number | null,
): Promise<ForemanSession> {
  // `?kind=`（决策 286 / 票 01）：不指定 id 时缺省落点按它取各自的「最近」。
  const params = new URLSearchParams();
  if (sessionId) params.set('session', sessionId);
  if (kind) params.set('kind', kind);
  if (beforeId != null) params.set('before_id', String(beforeId));
  const q = params.size > 0 ? `?${params.toString()}` : '';
  return request<ForemanSession>(`/foreman/session${q}`, { signal });
}

/** 班次列表，按最近活动倒序（决策 204⑦）。`includeArchived`（票 06）：true 时含归档。 */
export function getForemanSessions(
  signal?: AbortSignal,
  kind?: string,
  includeArchived?: boolean,
): Promise<ForemanSessionList> {
  // `?kind=watch` 取值守台账的列表（决策 286 / 票 01）；缺省只回人的班次——
  // 「对讲台的班次列表」与「值守的独立入口」是两个列表。`?include_archived=true`
  // 是「显示已归档」那颗开关的后端一半（票 06）；不给 = 现状，只列未归档。
  const params = new URLSearchParams();
  if (kind) params.set('kind', kind);
  if (includeArchived) params.set('include_archived', 'true');
  const q = params.size > 0 ? `?${params.toString()}` : '';
  return request<ForemanSessionList>(`/foreman/sessions${q}`, { signal });
}

/**
 * 全量工具清单的回执标签（决策 247⑤）。
 *
 * 静态清单，前端**取数一次缓存**（`lib/toolLabels.ts`）；不带档位参数——回执标的是历史，
 * 端点自己不滤。
 */
export function getForemanToolLabels(signal?: AbortSignal): Promise<ForemanToolLabelList> {
  return request<ForemanToolLabelList>('/foreman/tools', { signal });
}

/**
 * 未消费待办的只读计数（决策 307，票 06）。
 *
 * **只读**：不建行、不改行、不消费——消费归值守轮。页头那枚读数读它，
 * 与 `turn_in_flight` 无关（值守轮排队时也要看得见，那正是它存在的理由）。
 */
export function getForemanAttention(signal?: AbortSignal): Promise<ForemanAttention> {
  return request<ForemanAttention>('/foreman/attention', { signal });
}

/** 新开一个班次。标题留空 = 中性标题，第一句话说出来时按它命名（决策 204②）。 */
export function createForemanSession(title?: string): Promise<{ session: ForemanSessionMeta }> {
  return request<{ session: ForemanSessionMeta }>('/foreman/sessions', {
    method: 'POST',
    body: { title: title ?? null },
  });
}

export function renameForemanSession(
  sessionId: string,
  title: string,
): Promise<{ session: ForemanSessionMeta }> {
  return request<{ session: ForemanSessionMeta }>(`/foreman/sessions/${sessionId}`, {
    method: 'PATCH',
    body: { title },
  });
}

/** 归档 = 从列表里收起来（不删行、不删消息）。 */
export function archiveForemanSession(sessionId: string): Promise<{ session: ForemanSessionMeta }> {
  return request<{ session: ForemanSessionMeta }>(`/foreman/sessions/${sessionId}/archive`, {
    method: 'POST',
    body: {},
  });
}

/**
 * 停钮：请这一班**正在跑的那个人这一轮**停下（决策 294 / 票 09）。
 *
 * `cancelled: false` 不是错误——「没有一轮在跑」本身就是答案（值守轮压根不设停钮，
 * 裁决 10：它归开关），界面据此把那颗钮收回去。`true` 也只说**请求送到了**：收口是
 * 协作的（决策 226 的同一句话），它停没停由那一轮自己落的那一行（`【已停】`）回答。
 */
export function cancelForemanTurn(sessionId: string): Promise<{ cancelled: boolean }> {
  return request<{ cancelled: boolean }>(`/foreman/sessions/${sessionId}/cancel`, {
    method: 'POST',
    body: {},
  });
}

/**
 * 说一句话并拿回一次回话。
 *
 * **失败时不要清空输入框**：后端在叫模型之前就把 user 行落了库，所以失败是「这句话
 * 没被答上」而不是「这句话没说」——人应当能改几个字重发（决策 182㉓）。
 */
export function sendForemanMessage(
  text: string,
  sessionId?: string | null,
): Promise<ForemanSendResult> {
  return request<ForemanSendResult>('/foreman/messages', {
    method: 'POST',
    body: { text, session_id: sessionId ?? null },
    // 这一轮要等模型回话：默认 30s 兜底对它偏紧，显式放宽到 3 分钟（票 12 的口径）。
    // 2026-09-18 抬到 5 分钟：实测一次**成功**的回话约 150s，而它带着 9 万 token 的
    // prompt——3 分钟对它是常态而不是异常，于是本地一放弃就报一句「失败」，让人以为
    // 话没发出去。本地放弃**不再掐死这一轮**（决策 223：服务端那一轮跑在自己的任务里，
    // 回话照旧落库），故这个数只决定本地等多久，不影响这一轮的成败。
    timeoutMs: 300_000,
  });
}

/* ─────────── 确认钮（决策 188 / 207，票 02 / 03）：按下走既有端点 ─────────── */

/**
 * 按下「执行」。**执行不是一条新路**：后端按提议里的 `(工具, 参数)` 重走既有端点
 * ——同一套校验、同一套闸门，故这里的失败报文与在别处点同一颗钮时看到的是同一句。
 */
export function executeForemanProposal(id: string): Promise<ForemanProposalResult> {
  return request<ForemanProposalResult>(`/foreman/proposals/${encodeURIComponent(id)}/execute`, {
    method: 'POST',
    body: {},
  });
}

/**
 * 按下「拒绝」。它记的是「值班长提过、值班经理没让做」——**没有执行任何动作**，
 * 与「执行失败」是两件事（后者只是没成功，提议仍是未决态）。
 */
export function rejectForemanProposal(id: string): Promise<ForemanProposalResult> {
  return request<ForemanProposalResult>(`/foreman/proposals/${encodeURIComponent(id)}/reject`, {
    method: 'POST',
    body: {},
  });
}

/** 该班次的命令台账（含被出口策略拒掉的——审计面要看得见那次尝试）。 */
export function getForemanCommands(
  sessionId?: string | null,
  signal?: AbortSignal,
): Promise<{ session: ForemanSessionMeta | null; commands: ForemanCommand[] }> {
  const q = sessionId ? `?session=${encodeURIComponent(sessionId)}` : '';
  return request<{ session: ForemanSessionMeta | null; commands: ForemanCommand[] }>(
    `/foreman/commands${q}`,
    { signal },
  );
}

/* 「管线压缩」设置（long-run-budget 票 02）：硬底线 + 压缩保留轮数。 */

/** 读数：两个旋钮的有效值 + 逐字段 provenance（default / settings）。 */
export function getCompaction(): Promise<CompactionSettings> {
  return request<CompactionSettings>('/compaction');
}

/**
 * 保存两个旋钮：保存即活——流水线每 attempt、值班长每轮懒读 DB 覆盖层，
 * 下一轮就按新值走，不必重启。
 */
export function setCompaction(
  conversation_max_tokens: number,
  keep_recent_rounds: number,
): Promise<CompactionSettings> {
  return request<CompactionSettings>('/compaction', {
    method: 'PUT',
    body: { conversation_max_tokens, keep_recent_rounds },
  });
}
