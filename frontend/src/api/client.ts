import { CLIENT_HEADER, PAIRING_HEADER, PAIRING_QUERY, apiUrl, getPairingToken } from './config';
import type {
  AnalyzeResponse,
  MarketConfig,
  MarketEntry,
  MarketInstallResult,
  CreateTaskPayload,
  FlowResponse,
  ForemanSendResult,
  ForemanSession,
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
  // 决策 182㉙（票 07）：配对令牌压在**所有**方法上，不只写请求——非回环形态下
  // 对讲台的读接口（`/foreman/session`、`/foreman/stream`）同样凭它通行，
  // 而「只写请求带头」会让手机上看得到看板、对讲台却一律 403，那是更难排查的形态。
  // 未配对时（回环形态的常态）这个头根本不带，本地使用零摩擦。
  const token = getPairingToken();
  if (token) headers[PAIRING_HEADER] = token;
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

/* 技能市场（决策 172⑤ / 177 / 187）：来源白名单的读写 + 搜索 + 安装。 */

/** 当前生效的来源白名单与它的来源级别（界面 / 配置文件）。 */
export function getMarketConfig(): Promise<MarketConfig> {
  return request<MarketConfig>('/market/config');
}

/**
 * 保存界面上的来源白名单（决策 187）：**保存完当场生效**，不必重启。
 *
 * 空数组是合法且显式的输入（= 不允许远程安装），与「没保存过」不是一回事——后者读配置文件。
 * 非法来源由后端 400 并说明是哪一项、为什么（校验与 `config.toml` 共用同一个函数）。
 */
export function saveMarketConfig(sources: string[]): Promise<MarketConfig> {
  return request<MarketConfig>('/market/config', { method: 'PUT', body: { sources } });
}

/** 清掉界面那份来源，回到 `config.toml` 的 `[market] allowed_sources`。 */
export function clearMarketConfig(): Promise<MarketConfig> {
  return request<MarketConfig>('/market/config', { method: 'DELETE' });
}

/** 查 registry（省略 `q` = 列出全部已放行来源的技能）。 */
export function searchMarket(q = ''): Promise<{ skills: MarketEntry[]; sources: string[] }> {
  return request<{ skills: MarketEntry[]; sources: string[] }>(
    `/market/search?q=${encodeURIComponent(q)}`,
  );
}

/**
 * 从市场装一个技能到技能根（**不**写任何阶段配置——启用是另一件事）。
 *
 * 四类失败分得开：技能不存在 → 404、来源未放行 / 摘要不符 → 400（带 `detail`）、
 * 网络失败 → 502、同名已存在 → 409（`overwrite` 为显式确认）。
 * 落盘之后用 {@link previewSkill} 取三项预览给用户看（正文特征命中要摆在眼前）。
 */
export function installFromMarket(name: string, overwrite = false): Promise<MarketInstallResult> {
  return request<MarketInstallResult>('/market/install', {
    method: 'POST',
    body: { name, overwrite },
  });
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
 * **只在回环可读**（服务端强制）：这一步是「在一台已配对的设备上生成给手机的链接」，
 * 手机自己打开分享页时这里会 403——那是设计如此，不是故障。调用方据此降级为
 * 不带令牌的裸地址（手机能看只读页，只是动手与对话还得先配对）。
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
 * 「值班员说了什么 / 值班长回了什么 / 合计烧了多少 token」，不自己攒一份账。
 * 工头未接线时后端回 503，由 `request` 抛出 `ApiError`。
 */
export function getForemanSession(signal?: AbortSignal): Promise<ForemanSession> {
  return request<ForemanSession>('/foreman/session', { signal });
}

/**
 * 说一句话并拿回一次回话。
 *
 * **失败时不要清空输入框**：后端在叫模型之前就把 user 行落了库，所以失败是「这句话
 * 没被答上」而不是「这句话没说」——人应当能改几个字重发（决策 182㉓）。
 */
export function sendForemanMessage(text: string): Promise<ForemanSendResult> {
  return request<ForemanSendResult>('/foreman/messages', {
    method: 'POST',
    body: { text },
  });
}
