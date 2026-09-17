import { CLIENT_HEADER, PAIRING_HEADER, PAIRING_QUERY, apiUrl, getPairingToken } from './config';
import type {
  AnalyzeResponse,
  MarketRepoConfig,
  MarketSkillList,
  CreateTaskPayload,
  FlowResponse,
  ForemanSendResult,
  ForemanSession,
  ForemanSessionList,
  ForemanSessionMeta,
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
 */
export function getForemanSession(
  sessionId?: string | null,
  signal?: AbortSignal,
): Promise<ForemanSession> {
  const q = sessionId ? `?session=${encodeURIComponent(sessionId)}` : '';
  return request<ForemanSession>(`/foreman/session${q}`, { signal });
}

/** 未归档的班次，按最近活动倒序（决策 204⑦）。 */
export function getForemanSessions(signal?: AbortSignal): Promise<ForemanSessionList> {
  return request<ForemanSessionList>('/foreman/sessions', { signal });
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
  });
}
