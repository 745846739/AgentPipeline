import { CLIENT_HEADER, apiUrl } from './config';
import type {
  AnalyzeResponse,
  CreateTaskPayload,
  FlowResponse,
  GlobalMetrics,
  NodeCommand,
  NodeConversation,
  Project,
  ProjectAnalysis,
  ProjectCreatePayload,
  ProjectPatchPayload,
  Provider,
  ProviderCreatePayload,
  ProviderPatchPayload,
  ResumePayload,
  StageConfig,
  StageConfigPutPayload,
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
  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

interface RequestOptions {
  method?: string;
  body?: unknown;
  /** 期望纯文本（命令输出 / 产出文件）。 */
  text?: boolean;
  signal?: AbortSignal;
}

async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const method = opts.method ?? 'GET';
  const headers: Record<string, string> = { Accept: 'application/json, text/plain, */*' };
  // 决策 153③：所有写请求恒携带 X-AgentPipeline（128 旁路）。
  if (method !== 'GET' && method !== 'HEAD') {
    headers[CLIENT_HEADER] = '1';
  }
  if (opts.body !== undefined) {
    headers['Content-Type'] = 'application/json';
  }

  let res: Response;
  try {
    res = await fetch(apiUrl(path), {
      method,
      headers,
      body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
      signal: opts.signal,
    });
  } catch (err) {
    throw new ApiError(0, `网络请求失败：${(err as Error).message}`);
  }

  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      const data = (await res.json()) as { error?: string };
      if (data?.error) message = data.error;
    } catch {
      // 非 JSON 错误体，保留状态文本
    }
    throw new ApiError(res.status, message);
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

export function createTask(payload: CreateTaskPayload): Promise<{ task: unknown }> {
  return request('/tasks', { method: 'POST', body: payload });
}

export function getFlow(id: string, signal?: AbortSignal): Promise<FlowResponse> {
  return request<FlowResponse>(`/tasks/${encodeURIComponent(id)}/flow`, { signal });
}

export function getConversations(id: string, signal?: AbortSignal): Promise<ConversationSummary[]> {
  return request<{ conversations: ConversationSummary[] }>(
    `/tasks/${encodeURIComponent(id)}/conversations`,
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

export function retryTask(id: string): Promise<unknown> {
  return request(`/tasks/${encodeURIComponent(id)}/retry`, { method: 'POST' });
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

/** merge 审批（决策 23 / 119）：approve = 合入，return = 返回修改。**无"拒绝"**。 */
export function mergeDecision(id: string, decision: 'approve' | 'return'): Promise<unknown> {
  return request(`/tasks/${encodeURIComponent(id)}/merge/decision`, {
    method: 'POST',
    body: { decision },
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
